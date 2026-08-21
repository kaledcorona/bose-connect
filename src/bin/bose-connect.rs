//! A thin command line over the library.
//!
//! Deliberately thin: it exists to prove the API is usable by something other
//! than its own tests, and to be the first thing that complains if it is not.

use std::time::Duration;

use bose_connect::api::{Anc, Toggle};
use bose_connect::catalog::{self, CATALOG};
use bose_connect::codec::{AutoOff, Immersive, Language, Level};
use bose_connect::device::Device;
use bose_connect::error::{Error, Result};
use bose_connect::transport::{probe_channel, Address, Rfcomm};
use bose_connect::wire::{hex, Addr};

const TIMEOUT: Duration = Duration::from_secs(4);

fn bad(s: &str) -> Error {
    Error::Io(std::io::Error::other(format!("not a number: {s}")))
}

fn num(s: &str) -> Result<u8> {
    let s = s.strip_prefix("0x").map_or((s, 10), |h| (h, 16));
    u8::from_str_radix(s.0, s.1).map_err(|_| bad(s.0))
}

fn usage() -> ! {
    eprintln!(
        "usage: bose-connect <address> <command>

  info              identity and what this model supports
  json              the same, machine-readable
  anc               read noise cancelling
  anc off|high|low  set it, where the model offers named levels
  eq                read the equaliser, with each band's range
  eq <band> <val>   set one band
  modes             list stored modes
  mode              read the active mode index (selecting is not understood)

  language [en|es] [on|off]
      read or set the voice-prompt language; the second word is the
      battery announcement, which the record forces you to write too
  name [new]        read or set the device name
  battery           charge, one line per cell
  volume [level]    read or set, clamped to the device's own scale
  auto-off [min|never]
  toggle <what> [on|off]
      multipoint | head-detection | auto-answer | remember-mode

  catalog           every record this build knows, and how well
  raw <fn> <op>     read one address, decoded by nobody
  scan [first last] which functions answer; defaults to 00-3f"
    );
    std::process::exit(2)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("bose-connect: {e}");
        std::process::exit(1);
    }
}

/// Arity and number parsing before the channel probe, which at four seconds a
/// channel costs up to two minutes on an unreachable device.
fn check(args: &[String]) -> Result<()> {
    let need = |n: usize| if args.len() < n { usage() };
    match args[1].as_str() {
        "eq" if args.len() > 2 => {
            need(4);
            num(&args[2])?;
            args[3].parse::<i8>().map_err(|_| bad(&args[3]))?;
        }
        "toggle" => need(3),
        "raw" => {
            need(4);
            num(&args[2])?;
            num(&args[3])?;
        }
        "scan" if args.len() > 2 => {
            need(4);
            num(&args[2])?;
            num(&args[3])?;
        }
        "volume" | "mode" if args.len() > 2 => {
            num(&args[2])?;
        }
        "auto-off" if args.len() > 2 && args[2] != "never" => {
            num(&args[2])?;
        }
        _ => {}
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    check(&args)?;

    // Needs no device, so it answers before anything is probed.
    if args[1] == "catalog" {
        print_catalog();
        return Ok(());
    }

    let addr: Address = args[0].parse()?;
    let (channel, session) = probe_channel(addr, TIMEOUT)?;
    let mut dev = Device::open(session)?;

    match args[1].as_str() {
        "info" => info(&mut dev, channel),
        "json" => json(&mut dev, channel),
        "anc" if args.len() == 2 => anc(&mut dev)?,
        "anc" => {
            let level = match args[2].as_str() {
                "off" => Level::Off,
                "high" => Level::High,
                "low" => Level::Low,
                _ => usage(),
            };
            dev.set_level(level)?;
        }
        "eq" if args.len() == 2 => {
            for b in dev.equaliser()? {
                println!("band {}  {:>3}  [{}..{}]", b.index, b.value, b.min, b.max);
            }
        }
        "eq" => dev.set_band(num(&args[2])?, args[3].parse().map_err(|_| bad(&args[3]))?)?,
        "language" if args.len() == 2 => {
            let p = dev.prompts()?;
            println!(
                "{:?}  voice prompts {}  battery announcement {}  (record {})",
                p.language,
                on_off(p.voice_prompts),
                p.battery_announcement.map_or("?", on_off),
                hex(&p.raw),
            );
        }
        "language" => {
            let l = match args[2].as_str() {
                "en" => Language::English,
                "es" => Language::Spanish,
                _ => usage(),
            };
            match args.get(3).map(String::as_str) {
                None => dev.set_language(l)?,
                Some(v @ ("on" | "off")) => dev.set_prompts(l, v == "on")?,
                Some(_) => usage(),
            }
        }
        "name" if args.len() == 2 => println!("{}", dev.name()?),
        "name" => dev.set_name(&args[2])?,
        "battery" => {
            for c in dev.battery()? {
                println!("cell {}  {}%", c.index, c.percent);
            }
        }
        "volume" if args.len() == 2 => {
            let v = dev.volume()?;
            println!("{} of {}", v.current, v.max());
        }
        "volume" => dev.set_volume(num(&args[2])?)?,
        "auto-off" if args.len() == 2 => println!("{}", dev.auto_off()?),
        "auto-off" => {
            let v = if args[2] == "never" {
                AutoOff::Never
            } else {
                AutoOff::from_minutes(num(&args[2])?)
            };
            dev.set_auto_off(v)?;
        }
        "toggle" => {
            let t = Toggle::ALL
                .into_iter()
                .find(|t| t.name() == args[2])
                .unwrap_or_else(|| usage());
            match args.get(3).map(String::as_str) {
                None => println!("{}", on_off(dev.toggle(t)?)),
                Some(v) => dev.set_toggle(t, v == "on")?,
            }
        }
        "modes" => {
            for m in dev.modes()? {
                let wind = if m.wind_block { "  wind block" } else { "" };
                println!("{}  {:<20} awareness {}{}", m.index, m.name, m.awareness, wind);
            }
        }
        "mode" if args.len() == 2 => println!("{}", dev.current_mode()?),
        "mode" => dev.select_mode(num(&args[2])?)?,
        "raw" => {
            let a = Addr::at(num(&args[2])?, num(&args[3])?);
            let bytes = dev.raw(a)?;
            let known = catalog::find(a).map_or(String::new(), |m| format!("  ({})", m.label));
            println!("{a}  {}{known}", hex(&bytes));
        }
        "scan" => {
            let (first, last) = match args.len() {
                2 => (0x00, 0x3f),
                _ => (num(&args[2])?, num(&args[3])?),
            };
            let found = dev.scan(first, last)?;
            println!("{}", found.iter().map(|f| format!("{f:02x}")).collect::<Vec<_>>().join(" "));
        }
        _ => usage(),
    }
    Ok(())
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

fn anc(dev: &mut Device<Rfcomm>) -> Result<()> {
    match dev.noise_cancelling()? {
        Anc::Named(a) => {
            let names: Vec<String> =
                a.levels().map(|l| format!("{l:?}").to_lowercase()).collect();
            let now = a.level.map_or("unknown".into(), |l| format!("{l:?}").to_lowercase());
            println!("{now}  (accepts {})", names.join(", "));
        }
        Anc::Graded(a) => {
            println!("cancelling {} of {}  (awareness {})", a.cancelling(), a.top(), a.awareness);
        }
    }
    Ok(())
}

fn info(dev: &mut Device<Rfcomm>, channel: u8) {
    let id = dev.identity.clone();
    println!("channel   {channel}");
    println!("id        0x{:04x} index {}", id.id, id.index);
    println!("model     {}", id.model.as_deref().unwrap_or("(not reported)"));
    println!("version   {}", id.version.as_deref().unwrap_or("-"));
    println!("serial    {}", id.serial.as_deref().unwrap_or("-"));
    let s = dev.supports();
    println!(
        "supports  anc:{} eq:{} immersive:{} modes:{}",
        s.anc.map_or("no", |k| k.as_str()),
        s.equaliser,
        s.immersive,
        s.modes
    );
    println!(
        "functions {}",
        dev.surface.functions().map(|f| format!("{f:02x}")).collect::<Vec<_>>().join(" ")
    );
    if s.immersive && let Ok(i) = dev.immersive() {
        println!("immersive {}", immersive_name(i));
    }
}

fn immersive_name(i: Immersive) -> &'static str {
    match i {
        Immersive::Off => "off",
        Immersive::Still => "still",
        Immersive::Motion => "motion",
    }
}

/// Hand-rolled: a handful of fields do not justify a dependency, and a
/// dependency in a binary becomes one for anyone vendoring the crate.
fn json(dev: &mut Device<Rfcomm>, channel: u8) {
    let q = |o: &Option<String>| match o {
        Some(v) => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "null".into(),
    };
    let id = dev.identity.clone();
    let s = dev.supports();
    let battery: Vec<String> = dev
        .battery()
        .unwrap_or_default()
        .iter()
        .map(|c| format!("{{\"cell\": {}, \"percent\": {}}}", c.index, c.percent))
        .collect();
    println!("{{");
    println!("  \"channel\": {channel},");
    println!("  \"id\": \"0x{:04x}\",", id.id);
    println!("  \"model\": {},", q(&id.model));
    println!("  \"version\": {},", q(&id.version));
    println!("  \"serial\": {},", q(&id.serial));
    println!("  \"anc\": {},", s.anc.map_or("null".into(), |k| format!("\"{}\"", k.as_str())));
    println!("  \"equaliser\": {},", s.equaliser);
    println!("  \"immersive\": {},", s.immersive);
    println!("  \"modes\": {},", s.modes);
    println!("  \"battery\": [{}]", battery.join(", "));
    println!("}}");
}

/// The catalog, as a table. `cargo doc` renders the same facts; this is for
/// when the question is what a *build* knows, in a terminal, next to a device.
fn print_catalog() {
    println!("{:<6} {:<34} {:<10} {:<10} note", "addr", "record", "read", "write");
    for m in CATALOG {
        println!(
            "{:<6} {:<34} {:<10} {:<10} {}",
            m.addr.to_string(),
            m.label,
            evidence(m.read),
            evidence(m.write),
            m.note
        );
    }
}

fn evidence(e: bose_connect::catalog::Evidence) -> &'static str {
    use bose_connect::catalog::Evidence::*;
    match e {
        Confirmed => "confirmed",
        Syntax(_) => "syntax",
        Ineffective(_) => "no effect",
        Unknown => "-",
    }
}
