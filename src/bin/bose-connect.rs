//! A thin command line over the library.
//!
//! Deliberately thin: it exists to prove the API is usable by something other
//! than its own tests, and to be the first thing that complains if it is not.

use std::time::Duration;

use bose_connect::device::{Anc, Device};
use bose_connect::fields::Toggle;
use bose_connect::settings::{AncState, Level};
use bose_connect::transport::{probe_channel, Address};

const TIMEOUT: Duration = Duration::from_secs(4);

fn usage() -> ! {
    eprintln!(
        "usage: bose-connect <address> <command>

  info              identity and what this model supports
  anc               read noise cancelling
  anc off|high|low  set it, where the model offers named levels
  eq                read the equaliser, with each band's range
  eq <band> <val>   set one band
  modes             list stored modes
  mode              read the active mode index (selecting is not understood)

  name [new]        read or set the device name
  battery           charge, one line per cell
  volume [level]    read or set, clamped to the device's own scale
  auto-off [min|never]
  toggle <what> [on|off]
      multipoint | head-detection | auto-answer | remember-mode"
    );
    std::process::exit(2)
}

fn main() {
    // `Result` from main prints the Debug form, which shows a user
    // `Custom { kind: Unsupported, error: Unsupported("modes") }`. Say it plainly.
    if let Err(e) = run() {
        eprintln!("bose-connect: {e}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    let addr: Address = args[0].parse()?;

    let (channel, session) = probe_channel(addr, TIMEOUT)
        .ok_or_else(|| std::io::Error::other("no control channel; is it connected?"))?;
    let mut dev = Device::open(session)?;

    match args[1].as_str() {
        "info" => {
            let id = &dev.identity;
            println!("channel   {channel}");
            println!("id        0x{:04x} index {}", id.id, id.index);
            println!("model     {}", id.model.as_deref().unwrap_or("(not reported)"));
            println!("version   {}", id.version.as_deref().unwrap_or("-"));
            println!("serial    {}", id.serial.as_deref().unwrap_or("-"));
            let c = &dev.capabilities;
            println!(
                "supports  anc:{} eq:{} immersive:{} modes:{}",
                match c.anc {
                    Anc::Legacy => "named",
                    Anc::Modern => "graded",
                    Anc::Absent => "no",
                },
                c.equaliser,
                c.immersive,
                c.modes
            );
            if c.immersive {
                if let Ok(Some(i)) = dev.immersive() {
                    println!("immersive {:?}", i);
                }
            }
        }
        "anc" if args.len() == 2 => match dev.noise_cancelling()? {
            Some(AncState::Named { level, accepted }) => {
                let names = [(0u8, "off"), (1, "high"), (3, "low")]
                    .iter()
                    .filter(|(bit, _)| accepted & (1 << bit) != 0)
                    .map(|(_, n)| *n)
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "{}  (accepts {names})",
                    match level {
                        Some(l) => format!("{l:?}").to_lowercase(),
                        None => "unknown".into(),
                    }
                );
            }
            Some(AncState::Graded { awareness, values }) => println!(
                "cancelling {} of {}  (awareness {awareness})",
                values - 1 - awareness,
                values - 1
            ),
            None => println!("no answer"),
        },
        "anc" => {
            let level = match args[2].as_str() {
                "off" => Level::Off,
                "high" => Level::High,
                "low" => Level::Low,
                _ => usage(),
            };
            dev.set_level(level)?;
            println!("{:?}", level);
        }
        "eq" if args.len() == 2 => {
            for b in dev.equaliser()? {
                println!("band {}  {:>3}  [{}..{}]", b.index, b.value, b.min, b.max);
            }
        }
        "eq" => {
            let (band, value) = (args[2].parse().unwrap_or(0), args[3].parse().unwrap_or(0));
            dev.set_band(band, value)?;
        }
        "name" if args.len() == 2 => {
            println!("{}", dev.name()?.unwrap_or_else(|| "-".into()))
        }
        "name" => dev.set_name(&args[2])?,
        "battery" => {
            for c in dev.battery()? {
                println!("cell {}  {}%", c.index, c.percent);
            }
        }
        "volume" if args.len() == 2 => match dev.volume()? {
            Some(v) => println!("{} of {}", v.current, v.steps - 1),
            None => println!("-"),
        },
        "volume" => dev.set_volume(args[2].parse().unwrap_or(0))?,
        "auto-off" if args.len() == 2 => match dev.auto_off()? {
            Some(Some(m)) => println!("{m} min"),
            Some(None) => println!("never"),
            None => println!("-"),
        },
        "auto-off" => {
            let v = if args[2] == "never" { None } else { args[2].parse().ok() };
            dev.set_auto_off(v)?
        }
        "toggle" => {
            let t = match args[2].as_str() {
                "multipoint" => Toggle::Multipoint,
                "head-detection" => Toggle::HeadDetection,
                "auto-answer" => Toggle::AutoAnswer,
                "remember-mode" => Toggle::RememberMode,
                _ => usage(),
            };
            match args.get(3).map(String::as_str) {
                None => println!(
                    "{}",
                    match dev.toggle(t)? {
                        Some(true) => "on",
                        Some(false) => "off",
                        None => "-",
                    }
                ),
                Some(v) => dev.set_toggle(t, v == "on")?,
            }
        }
        "modes" => {
            for (i, name) in dev.modes()? {
                println!("{i}  {name}");
            }
        }
        "mode" if args.len() == 2 => match dev.current_mode()? {
            Some(i) => println!("{i}"),
            None => println!("-"),
        },
        "mode" => dev.select_mode(args[2].parse().unwrap_or(0))?,
        _ => usage(),
    }
    Ok(())
}
