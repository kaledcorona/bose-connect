//! A thin command line over the library.
//!
//! Deliberately thin: it exists to prove the API is usable by something other
//! than its own tests, and to be the first thing that complains if it is not.
//!
//! Everything the arguments can say is settled in [`parse`] before a socket is
//! opened. The channel probe costs up to four seconds per channel on an
//! unreachable device, so a typo must fail before it, not after.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::time::Duration;

use bose_connect::api::{Anc, Toggle};
use bose_connect::catalog::{self, Evidence, ACTIVE_DEVICE, CATALOG, PAIRED};
use bose_connect::codec::{AutoOff, Immersive, Language, Level};
use bose_connect::device::Device;
use bose_connect::error::Result;
use bose_connect::session::Session;
use bose_connect::transport::{self, Address, Rfcomm, Transport};
use bose_connect::wire::{hex, Addr};

const TIMEOUT: Duration = Duration::from_secs(4);

const USAGE: &str = "usage: bose-connect [--channel N] <address> <command>
       bose-connect catalog | devices | --help | --version

  info              identity and what this model supports
  json              the same, machine-readable
  anc               read noise cancelling
  anc off|high|low  set it, where the model offers named levels
  eq                read the equaliser, with each band's range
  eq <band> <val>   set one band
  modes             list stored modes
  mode              read the active mode index (selecting is not understood)
  immersive         read immersive audio

  language [en|es] [on|off]
      read or set the voice-prompt language; the second word is the
      battery announcement, which the record forces you to write too
  name [new]        read or set the device name
  battery           charge, one line per cell
  volume [level]    read or set, clamped to the device's own scale
  auto-off [min|never]
  toggle <what> [on|off]
      multipoint | head-detection | auto-answer | remember-mode
  paired            addresses the device remembers
  active            the host holding the link

  catalog           every record this build knows, and how well
  devices           paired Bose devices, from bluez (needs the bluez feature)
  raw <fn> <op>     read one address, decoded by nobody; hex
  scan [first last] which functions answer; hex, defaults to 00-3f

  --channel N       use this channel; otherwise the last one that answered,
                    or a probe";

/// One invocation, fully decided.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmd {
    Help,
    Version,
    Catalog,
    Devices,
    Info,
    Json,
    Anc(Option<Level>),
    Eq(Option<(u8, i8)>),
    Modes,
    Mode,
    Immersive,
    Language(Option<(Language, Option<bool>)>),
    Name(Option<String>),
    Battery,
    Volume(Option<u8>),
    AutoOff(Option<AutoOff>),
    Toggle(Toggle, Option<bool>),
    Paired,
    Active,
    Raw(Addr),
    Scan(u8, u8),
}

impl Cmd {
    fn needs_device(&self) -> bool {
        !matches!(self, Cmd::Help | Cmd::Version | Cmd::Catalog | Cmd::Devices)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    addr: Option<Address>,
    channel: Option<u8>,
    cmd: Cmd,
}

/// What an argument failed to be. The message is the whole error.
type Usage = String;

/// Arguments to an invocation. Pure, so a typo costs nothing and a test covers
/// it without a device.
fn parse(args: &[String]) -> std::result::Result<Invocation, Usage> {
    let mut channel = None;
    let mut rest: &[String] = args;
    while let [flag, value, tail @ ..] = rest
        && flag == "--channel"
    {
        channel = Some(dec(value)?);
        rest = tail;
    }
    let [first, ..] = rest else { return Err(USAGE.into()) };
    match first.as_str() {
        "--help" | "-h" | "help" => return Ok(Invocation { addr: None, channel, cmd: Cmd::Help }),
        "--version" | "-V" => return Ok(Invocation { addr: None, channel, cmd: Cmd::Version }),
        "catalog" => return Ok(Invocation { addr: None, channel, cmd: Cmd::Catalog }),
        "devices" => return Ok(Invocation { addr: None, channel, cmd: Cmd::Devices }),
        _ => {}
    }
    let [addr, cmd, rest @ ..] = rest else { return Err(USAGE.into()) };
    let addr = addr.parse::<Address>().map_err(|e| format!("{addr}: {e}"))?;
    let cmd = command(cmd, rest)?;
    Ok(Invocation { addr: Some(addr), channel, cmd })
}

fn command(name: &str, rest: &[String]) -> std::result::Result<Cmd, Usage> {
    let r: Vec<&str> = rest.iter().map(String::as_str).collect();
    let cmd = match (name, r.as_slice()) {
        ("catalog", []) => Cmd::Catalog,
        ("info", []) => Cmd::Info,
        ("json", []) => Cmd::Json,
        ("anc", []) => Cmd::Anc(None),
        ("anc", [l]) => Cmd::Anc(Some(level(l)?)),
        ("eq", []) => Cmd::Eq(None),
        ("eq", [band, value]) => Cmd::Eq(Some((dec(band)?, signed(value)?))),
        ("modes", []) => Cmd::Modes,
        ("mode", []) => Cmd::Mode,
        ("immersive", []) => Cmd::Immersive,
        ("language", []) => Cmd::Language(None),
        ("language", [l]) => Cmd::Language(Some((language(l)?, None))),
        ("language", [l, a]) => Cmd::Language(Some((language(l)?, Some(on_off(a)?)))),
        ("name", []) => Cmd::Name(None),
        ("name", [n]) => Cmd::Name(Some((*n).to_string())),
        ("battery", []) => Cmd::Battery,
        ("volume", []) => Cmd::Volume(None),
        ("volume", [v]) => Cmd::Volume(Some(dec(v)?)),
        ("auto-off", []) => Cmd::AutoOff(None),
        ("auto-off", ["never"]) => Cmd::AutoOff(Some(AutoOff::Never)),
        ("auto-off", [m]) => Cmd::AutoOff(Some(AutoOff::from_minutes(dec(m)?))),
        ("toggle", [what]) => Cmd::Toggle(toggle(what)?, None),
        ("toggle", [what, v]) => Cmd::Toggle(toggle(what)?, Some(on_off(v)?)),
        ("paired", []) => Cmd::Paired,
        ("active", []) => Cmd::Active,
        ("raw", [f, o]) => Cmd::Raw(Addr::at(hex_byte(f)?, hex_byte(o)?)),
        ("scan", []) => Cmd::Scan(0x00, 0x3f),
        ("scan", [a, b]) => Cmd::Scan(hex_byte(a)?, hex_byte(b)?),
        _ => return Err(USAGE.into()),
    };
    Ok(cmd)
}

/// Addresses are hex everywhere — in the reference, in `catalog`'s output, in
/// every error message — so `raw` and `scan` take them that way, with or
/// without `0x`.
fn hex_byte(s: &str) -> std::result::Result<u8, Usage> {
    let digits = s.strip_prefix("0x").unwrap_or(s);
    u8::from_str_radix(digits, 16).map_err(|_| format!("not a hex byte: {s}"))
}

/// Quantities — a volume, a band, minutes — are decimal, as a person says them.
fn dec(s: &str) -> std::result::Result<u8, Usage> {
    s.parse().map_err(|_| format!("not a number 0-255: {s}"))
}

fn signed(s: &str) -> std::result::Result<i8, Usage> {
    s.parse().map_err(|_| format!("not a number -128..127: {s}"))
}

fn level(s: &str) -> std::result::Result<Level, Usage> {
    match s {
        "off" => Ok(Level::Off),
        "high" => Ok(Level::High),
        "low" => Ok(Level::Low),
        _ => Err(format!("not a level (off, high, low): {s}")),
    }
}

fn language(s: &str) -> std::result::Result<Language, Usage> {
    match s {
        "en" => Ok(Language::English),
        "es" => Ok(Language::Spanish),
        _ => Err(format!("not a language (en, es): {s}")),
    }
}

fn on_off(s: &str) -> std::result::Result<bool, Usage> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("not on or off: {s}")),
    }
}

fn toggle(s: &str) -> std::result::Result<Toggle, Usage> {
    Toggle::ALL.into_iter().find(|t| t.name() == s).ok_or_else(|| {
        let names: Vec<&str> = Toggle::ALL.iter().map(|t| t.name()).collect();
        format!("not a toggle ({}): {s}", names.join(", "))
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let inv = match parse(&args) {
        Ok(inv) => inv,
        Err(msg) if msg == USAGE => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
        Err(msg) => {
            eprintln!("bose-connect: {msg}");
            std::process::exit(2);
        }
    };
    let mut out = io::stdout().lock();
    let done = match inv.cmd {
        Cmd::Help => writeln!(out, "{USAGE}").map_err(Into::into),
        Cmd::Version => writeln!(out, "bose-connect {}", env!("CARGO_PKG_VERSION")).map_err(Into::into),
        Cmd::Catalog => print_catalog(&mut out).map_err(Into::into),
        Cmd::Devices => devices(&mut out),
        _ => connect_and_run(&inv, &mut out),
    };
    match done {
        // `catalog | head` closes stdout early; that is the reader's choice,
        // not a failure.
        Err(bose_connect::error::Error::Io(e)) if e.kind() == io::ErrorKind::BrokenPipe => {}
        Err(e) => {
            eprintln!("bose-connect: {e}");
            std::process::exit(1);
        }
        Ok(()) => {}
    }
}

fn connect_and_run(inv: &Invocation, out: &mut impl Write) -> Result<()> {
    debug_assert!(inv.cmd.needs_device());
    let addr = inv.addr.expect("a device command carries an address");
    let (channel, session) = match inv.channel {
        Some(c) => (c, Session::open(Rfcomm::connect(addr, c, TIMEOUT)?)?),
        None => transport::connect(addr, TIMEOUT)?,
    };
    let mut dev = Device::open(session)?;
    run(&inv.cmd, &mut dev, channel, out)
}

/// Dispatch. Generic over the transport so the fixtures can drive it.
fn run<T: Transport>(cmd: &Cmd, dev: &mut Device<T>, channel: u8, out: &mut impl Write) -> Result<()> {
    match cmd {
        Cmd::Help | Cmd::Version | Cmd::Catalog | Cmd::Devices => {
            unreachable!("answered without a device")
        }
        Cmd::Info => info(dev, channel, out)?,
        Cmd::Json => json(dev, channel, out)?,
        Cmd::Anc(None) => anc(dev, out)?,
        Cmd::Anc(Some(level)) => dev.set_level(*level)?,
        Cmd::Eq(None) => {
            for b in dev.equaliser()? {
                writeln!(out, "band {}  {:>3}  [{}..{}]", b.index, b.value, b.min, b.max)?;
            }
        }
        Cmd::Eq(Some((band, value))) => dev.set_band(*band, *value)?,
        Cmd::Modes => {
            for m in dev.modes()? {
                let wind = if m.wind_block { "  wind block" } else { "" };
                writeln!(out, "{}  {:<20} awareness {}{}", m.index, m.name, m.awareness, wind)?;
            }
        }
        Cmd::Mode => writeln!(out, "{}", dev.current_mode()?)?,
        Cmd::Immersive => writeln!(out, "{}", immersive_name(dev.immersive()?))?,
        Cmd::Language(None) => {
            let p = dev.prompts()?;
            writeln!(
                out,
                "{}  voice prompts {}  battery announcement {}  (record {})",
                language_name(p.language),
                on_off_name(p.voice_prompts),
                p.battery_announcement.map_or("?", on_off_name),
                hex(&p.raw),
            )?;
        }
        Cmd::Language(Some((l, None))) => dev.set_language(*l)?,
        Cmd::Language(Some((l, Some(a)))) => dev.set_prompts(*l, *a)?,
        Cmd::Name(None) => writeln!(out, "{}", dev.name()?)?,
        Cmd::Name(Some(n)) => dev.set_name(n)?,
        Cmd::Battery => {
            for c in dev.battery()? {
                writeln!(out, "cell {}  {}%", c.index, c.percent)?;
            }
        }
        Cmd::Volume(None) => {
            let v = dev.volume()?;
            writeln!(out, "{} of {}", v.current, v.max())?;
        }
        Cmd::Volume(Some(v)) => dev.set_volume(*v)?,
        Cmd::AutoOff(None) => writeln!(out, "{}", dev.auto_off()?)?,
        Cmd::AutoOff(Some(a)) => dev.set_auto_off(*a)?,
        Cmd::Toggle(t, None) => writeln!(out, "{}", on_off_name(dev.toggle(*t)?))?,
        Cmd::Toggle(t, Some(v)) => dev.set_toggle(*t, *v)?,
        Cmd::Paired => {
            for a in dev.get(&PAIRED)? {
                writeln!(out, "{a}")?;
            }
        }
        Cmd::Active => writeln!(out, "{}", dev.get(&ACTIVE_DEVICE)?)?,
        Cmd::Raw(a) => {
            let bytes = dev.raw(*a)?;
            let known = catalog::find(*a).map_or(String::new(), |m| format!("  ({})", m.label));
            writeln!(out, "{a}  {}{known}", hex(&bytes))?;
        }
        Cmd::Scan(first, last) => {
            let found = dev.scan(*first, *last)?;
            writeln!(out, "{}", found.iter().map(|f| format!("{f:02x}")).collect::<Vec<_>>().join(" "))?;
        }
    }
    Ok(())
}

fn on_off_name(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

fn language_name(l: Language) -> String {
    match l {
        Language::English => "en".into(),
        Language::Spanish => "es".into(),
        Language::Other(c) => format!("language {c:#04x}"),
    }
}

fn immersive_name(i: Immersive) -> &'static str {
    match i {
        Immersive::Off => "off",
        Immersive::Still => "still",
        Immersive::Motion => "motion",
    }
}

fn anc<T: Transport>(dev: &mut Device<T>, out: &mut impl Write) -> Result<()> {
    match dev.noise_cancelling()? {
        Anc::Named(a) => {
            let names: Vec<String> =
                a.levels().map(|l| format!("{l:?}").to_lowercase()).collect();
            let now = a.level.map_or("unknown".into(), |l| format!("{l:?}").to_lowercase());
            writeln!(out, "{now}  (accepts {})", names.join(", "))?;
        }
        Anc::Graded(a) => {
            writeln!(out, "cancelling {} of {}  (awareness {})", a.cancelling(), a.top(), a.awareness)?;
        }
    }
    Ok(())
}

fn info<T: Transport>(dev: &mut Device<T>, channel: u8, out: &mut impl Write) -> Result<()> {
    let id = dev.identity.clone();
    writeln!(out, "channel   {channel}")?;
    // Zero is no device's id; it is what `Device::open` leaves when `00 03`
    // did not answer.
    match id.id {
        0 => writeln!(out, "id        -")?,
        n => writeln!(out, "id        0x{n:04x} index {}", id.index)?,
    }
    writeln!(out, "model     {}", id.model.as_deref().unwrap_or("(not reported)"))?;
    writeln!(out, "version   {}", id.version.as_deref().unwrap_or("-"))?;
    writeln!(out, "serial    {}", id.serial.as_deref().unwrap_or("-"))?;
    let s = dev.supports();
    writeln!(
        out,
        "supports  anc:{} eq:{} immersive:{} modes:{}",
        s.anc.map_or("no", |k| k.as_str()),
        s.equaliser,
        s.immersive,
        s.modes
    )?;
    writeln!(
        out,
        "functions {}",
        dev.surface.functions().map(|f| format!("{f:02x}")).collect::<Vec<_>>().join(" ")
    )?;
    if s.immersive && let Ok(i) = dev.immersive() {
        writeln!(out, "immersive {}", immersive_name(i))?;
    }
    Ok(())
}

/// A JSON string, escaped per RFC 8259. Device names are whatever the firmware
/// holds, passed through `from_utf8_lossy`, so control bytes are possible.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).expect("string"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Hand-rolled: a handful of fields do not justify a dependency, and a
/// dependency in a binary becomes one for anyone vendoring the crate.
///
/// `battery` is `null` when the read failed, as distinct from `[]`: a script
/// polling it should not take an error for a device with no cells.
fn json<T: Transport>(dev: &mut Device<T>, channel: u8, out: &mut impl Write) -> Result<()> {
    let q = |o: &Option<String>| o.as_deref().map_or("null".into(), json_string);
    let id = dev.identity.clone();
    let s = dev.supports();
    let battery = match dev.battery() {
        Ok(cells) => {
            let cells: Vec<String> = cells
                .iter()
                .map(|c| format!("{{\"cell\": {}, \"percent\": {}}}", c.index, c.percent))
                .collect();
            format!("[{}]", cells.join(", "))
        }
        Err(_) => "null".into(),
    };
    writeln!(out, "{{")?;
    writeln!(out, "  \"channel\": {channel},")?;
    match id.id {
        0 => writeln!(out, "  \"id\": null,")?,
        n => writeln!(out, "  \"id\": \"0x{n:04x}\",")?,
    }
    writeln!(out, "  \"model\": {},", q(&id.model))?;
    writeln!(out, "  \"version\": {},", q(&id.version))?;
    writeln!(out, "  \"serial\": {},", q(&id.serial))?;
    writeln!(out, "  \"anc\": {},", s.anc.map_or("null".into(), |k| json_string(k.as_str())))?;
    writeln!(out, "  \"equaliser\": {},", s.equaliser)?;
    writeln!(out, "  \"immersive\": {},", s.immersive)?;
    writeln!(out, "  \"modes\": {},", s.modes)?;
    writeln!(out, "  \"battery\": {battery}")?;
    writeln!(out, "}}")?;
    Ok(())
}

#[cfg(feature = "bluez")]
fn devices(out: &mut impl Write) -> Result<()> {
    for p in transport::bluez::bose()? {
        let ids = match (p.vendor, p.product) {
            (Some(v), Some(pr)) => format!("{v:04x}:{pr:04x}"),
            _ => "-".into(),
        };
        let state = if p.connected { "connected" } else { "" };
        writeln!(out, "{}  {ids}  {:<28} {state}", p.address, p.name)?;
    }
    Ok(())
}

#[cfg(not(feature = "bluez"))]
fn devices(_: &mut impl Write) -> Result<()> {
    Err(bose_connect::error::Error::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "built without the bluez feature; pass the address",
    )))
}

/// The catalog, as a table. `cargo doc` renders the same facts; this is for
/// when the question is what a *build* knows, in a terminal, next to a device.
fn print_catalog(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "{:<6} {:<34} {:<10} {:<10} note", "addr", "record", "read", "write")?;
    for m in CATALOG {
        writeln!(
            out,
            "{:<6} {:<34} {:<10} {:<10} {}",
            m.addr.to_string(),
            m.label,
            evidence(m.read),
            evidence(m.write),
            m.note
        )?;
    }
    Ok(())
}

fn evidence(e: Evidence) -> &'static str {
    match e {
        Evidence::Confirmed => "confirmed",
        Evidence::Syntax(_) => "syntax",
        Evidence::Ineffective(_) => "no effect",
        Evidence::Unknown => "-",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bose_connect::fixtures;
    use bose_connect::transport::Scripted;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn cmd(s: &str) -> Cmd {
        parse(&args(s)).unwrap().cmd
    }

    fn open(t: Scripted) -> Device<Scripted> {
        Device::open(Session::open(t).unwrap()).unwrap()
    }

    /// What a command prints, on a fixture.
    fn output(c: &str, t: Scripted) -> String {
        let mut out = Vec::new();
        run(&cmd(&format!("{MAC} {c}")), &mut open(t), 8, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    const MAC: &str = "AA:BB:CC:00:00:01";

    #[test]
    fn addresses_are_hex_as_printed_by_the_catalog() {
        // `catalog` prints `1f 03`; pasting that back has to work.
        assert_eq!(cmd(&format!("{MAC} raw 1f 03")), Cmd::Raw(Addr::at(0x1f, 0x03)));
        assert_eq!(cmd(&format!("{MAC} raw 0x1f 0x03")), Cmd::Raw(Addr::at(0x1f, 0x03)));
        assert_eq!(cmd(&format!("{MAC} scan 00 3f")), Cmd::Scan(0x00, 0x3f));
        assert_eq!(cmd(&format!("{MAC} scan")), Cmd::Scan(0x00, 0x3f));
    }

    #[test]
    fn quantities_are_decimal_as_a_person_says_them() {
        assert_eq!(cmd(&format!("{MAC} volume 10")), Cmd::Volume(Some(10)));
        assert_eq!(cmd(&format!("{MAC} eq 0 -6")), Cmd::Eq(Some((0, -6))));
        assert_eq!(cmd(&format!("{MAC} auto-off 60")), Cmd::AutoOff(Some(AutoOff::After(60))));
        assert_eq!(cmd(&format!("{MAC} auto-off never")), Cmd::AutoOff(Some(AutoOff::Never)));
    }

    #[test]
    fn a_toggle_value_must_be_on_or_off() {
        // `toggle multipoint yes` used to switch multipoint *off*: anything
        // that was not the word "on" was taken as false.
        assert!(parse(&args(&format!("{MAC} toggle multipoint yes"))).is_err());
        assert_eq!(
            cmd(&format!("{MAC} toggle multipoint on")),
            Cmd::Toggle(Toggle::Multipoint, Some(true))
        );
        assert_eq!(cmd(&format!("{MAC} toggle remember-mode")), Cmd::Toggle(Toggle::RememberMode, None));
    }

    #[test]
    fn every_word_is_checked_before_a_socket_is_opened() {
        // Each of these used to connect, probe every channel, discover the
        // surface, and then print usage.
        for bad in ["anc medium", "language de", "language en maybe", "toggle nonsense", "eq 0", "raw 1f", "mode 3"] {
            let r = parse(&args(&format!("{MAC} {bad}")));
            assert!(r.is_err(), "{bad} was accepted");
        }
        assert!(parse(&args("not-a-mac info")).unwrap_err().contains("not-a-mac"));
    }

    #[test]
    fn commands_that_need_no_device_take_no_address() {
        assert_eq!(cmd("catalog"), Cmd::Catalog);
        assert_eq!(cmd(&format!("{MAC} catalog")), Cmd::Catalog);
        assert_eq!(cmd("--help"), Cmd::Help);
        assert_eq!(cmd("--version"), Cmd::Version);
        assert_eq!(cmd("devices"), Cmd::Devices);
        assert!(!Cmd::Catalog.needs_device() && Cmd::Info.needs_device());
    }

    #[test]
    fn a_known_channel_skips_the_probe() {
        let inv = parse(&args(&format!("--channel 8 {MAC} battery"))).unwrap();
        assert_eq!(inv.channel, Some(8));
        assert_eq!(inv.cmd, Cmd::Battery);
        assert!(parse(&args(&format!("--channel x {MAC} battery"))).is_err());
    }

    #[test]
    fn info_reports_what_each_generation_answers() {
        let old = output("info", fixtures::qc35());
        assert!(old.contains("id        0x400c index 2"));
        assert!(old.contains("model     (not reported)"));
        assert!(old.contains("supports  anc:named eq:false immersive:false modes:false"));
        let new = output("info", fixtures::ultra_hp());
        assert!(new.contains("model     Bose QC Ultra Headphones"));
        assert!(new.contains("anc:graded eq:true"));
        // Function 05 will not list itself, so `info` cannot claim immersive
        // audio before a read; the verb reaches it regardless.
        assert!(!new.contains("immersive off"));
        assert_eq!(output("immersive", fixtures::ultra_hp()), "off\n");
    }

    #[test]
    fn json_is_what_the_readme_promises() {
        let j = output("json", fixtures::qc35());
        assert!(j.contains("\"id\": \"0x400c\""));
        assert!(j.contains("\"model\": null"));
        assert!(j.contains("\"anc\": \"named\""));
        assert!(j.contains("\"battery\": [{\"cell\": 0, \"percent\": 70}]"));
        // A model with no noise cancelling says so with null, not a string.
        assert!(output("json", fixtures::sport()).contains("\"anc\": null"));
    }

    #[test]
    fn json_strings_escape_everything_a_firmware_might_hold() {
        assert_eq!(json_string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(json_string("tab\there\nnew\x01"), "\"tab\\there\\nnew\\u0001\"");
        assert_eq!(json_string("plain"), "\"plain\"");
    }

    #[test]
    fn reads_print_as_the_readme_shows() {
        assert_eq!(output("anc", fixtures::qc35()), "high  (accepts off, high, low)\n");
        assert_eq!(output("anc", fixtures::ultra_hp()), "cancelling 0 of 10  (awareness 10)\n");
        assert_eq!(output("battery", fixtures::qc35()), "cell 0  70%\n");
        assert_eq!(output("volume", fixtures::qc35()), "14 of 24\n");
        assert_eq!(output("auto-off", fixtures::qc35()), "60 min\n");
        assert_eq!(output("name", fixtures::qc35()), "qc35\n");
        assert_eq!(output("active", fixtures::qc35()), "AA:BB:CC:00:00:01\n");
        assert!(output("modes", fixtures::ultra_hp()).starts_with("0  Quiet"));
        assert!(output("eq", fixtures::ultra_hp()).contains("band 0    2  [-10..10]"));
    }

    #[test]
    fn writes_reach_the_wire_as_the_device_expects() {
        let mut d = open(fixtures::qc35());
        run(&cmd(&format!("{MAC} anc low")), &mut d, 8, &mut Vec::new()).unwrap();
        assert!(d.session().transport_sent().iter().any(|s| s == &[0x01, 0x06, 0x02, 0x01, 0x03]));
        let mut d = open(fixtures::ultra_hp());
        run(&cmd(&format!("{MAC} toggle multipoint off")), &mut d, 1, &mut Vec::new()).unwrap();
        assert!(d.session().transport_sent().iter().any(|s| s == &[0x01, 0x0a, 0x02, 0x01, 0x00]));
    }

    #[test]
    fn asking_for_what_a_model_lacks_is_an_error_with_the_address_in_it() {
        let mut out = Vec::new();
        let e = run(&cmd(&format!("{MAC} eq")), &mut open(fixtures::qc35()), 8, &mut out).unwrap_err();
        assert_eq!(e.to_string(), "01 07: this model does not have it");
        assert!(out.is_empty());
    }

    #[test]
    fn the_catalog_table_lists_every_record() {
        let mut out = Vec::new();
        print_catalog(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.lines().count(), CATALOG.len() + 1);
        assert!(s.contains("1f 03  current mode"));
    }
}
