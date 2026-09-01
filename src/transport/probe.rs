//! Finding the channel.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::rfcomm::{Address, Rfcomm};
use crate::session::Session;
use crate::wire::Addr;


/// Find the channel a device answers control traffic on.
///
/// It differs per model — 8 on a QuietComfort 35 and Sport Earbuds, 1 on the
/// Ultra family — and neither the SDP record nor the device id predicts it. The
/// only reliable method is to ask, with a read that writes nothing.
///
/// Scan once and keep the answer. Repeated full scans appeared to leave a
/// device unresponsive until it was power-cycled; unproven, but cheap to avoid.
///
/// A device that is switched off or out of range answers `EHOSTDOWN` on the
/// first channel and on all thirty, at five seconds each. Scanning through it
/// takes over a minute to conclude what the first attempt already said, so that
/// error ends the scan and is reported as itself.
pub fn probe_channel(addr: Address, timeout: Duration) -> io::Result<(u8, Session<Rfcomm>)> {
    // The two channels every model seen so far has used, first; then the
    // sweep. A first connection is then one or two attempts rather than up to
    // twenty-nine on an ACL link bluez may be bringing A2DP up on at the same
    // moment — which answers `Device or resource busy` and leaves no audio.
    let order = [1u8, 8].into_iter().chain((1..=30).filter(|c| ![1, 8, 24].contains(c)));
    for channel in order {
        // 24 is `SRfcomm`, silent, present on every model and most plausibly
        // firmware update. The protocol description says not to probe it; this
        // scan was writing a handshake there on every run.
        debug_assert_ne!(channel, 24);
        let sock = match Rfcomm::connect(addr, channel, timeout) {
            Ok(sock) => sock,
            Err(e) => match e.raw_os_error() {
                // Not a channel matter: nothing is listening at that address.
                Some(libc::EHOSTDOWN) | Some(libc::EHOSTUNREACH) | Some(libc::ETIMEDOUT) => {
                    return Err(io::Error::new(
                        ErrorKind::NotFound,
                        "device not reachable; switched off, out of range, or not connected",
                    ))
                }
                // `ECONNREFUSED` means the channel is there but taken — the
                // phone app holds one and keeps it. Try the next.
                _ => continue,
            },
        };
        let Ok(mut session) = Session::open(sock) else {
            continue;
        };
        // Device id: four bytes out, seven back, nothing changed.
        if session.read(Addr::at(0x00, 0x03)).is_ok() {
            // Hand back the open session rather than the number. Closing and
            // reconnecting races the kernel releasing the channel, which
            // surfaces as EBUSY on the very next connect.
            return Ok((channel, session));
        }
    }
    Err(io::Error::new(
        ErrorKind::NotFound,
        "no control channel answered; the device is connected but not speaking this protocol",
    ))
}



/// Where the channel for `addr` is remembered between runs.
///
/// `$XDG_CACHE_HOME/bose-connect/<address>`, one decimal number. A file per
/// device rather than one table, so two processes never race on a rewrite.
/// `None` when no cache directory can be named, which is not an error — the
/// probe still works, it is just slow.
pub fn cache_path(addr: Address) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("bose-connect").join(addr.to_string()))
}

/// The channel remembered for `addr`, if any. A damaged file reads as none.
pub fn cached_channel(addr: Address) -> Option<u8> {
    read_channel(&cache_path(addr)?)
}

fn read_channel(path: &Path) -> Option<u8> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Remember the channel `addr` answered on. Failure to write is swallowed:
/// the connection already succeeded, and a read-only home should not turn
/// that into an error.
pub fn remember_channel(addr: Address, channel: u8) {
    let Some(path) = cache_path(addr) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, format!("{channel}\n"));
}

/// Open a session, remembering the channel.
///
/// A cached channel is tried first and discarded if it no longer answers —
/// firmware updates have moved it. Otherwise the probe runs and its answer is
/// kept. What a client should call unless the user named the channel.
pub fn connect(addr: Address, timeout: Duration) -> io::Result<(u8, Session<Rfcomm>)> {
    if let Some(channel) = cached_channel(addr)
        && let Ok(sock) = Rfcomm::connect(addr, channel, timeout)
        && let Ok(mut session) = Session::open(sock)
        && session.read(Addr::at(0x00, 0x03)).is_ok()
    {
        return Ok((channel, session));
    }
    let (channel, session) = probe_channel(addr, timeout)?;
    remember_channel(addr, channel);
    Ok((channel, session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_cache_is_one_file_per_device_under_xdg() {
        // Read through the environment rather than set it: tests run in
        // parallel and `set_var` is unsound across threads.
        let addr: Address = "AA:BB:CC:00:00:01".parse().unwrap();
        let path = cache_path(addr).expect("a home directory");
        assert!(path.ends_with("bose-connect/AA:BB:CC:00:00:01"));
        assert!(path.is_absolute());
    }

    #[test]
    fn a_damaged_or_missing_cache_file_reads_as_no_cache() {
        // The probe is the fallback; a stray byte must not become a channel.
        let dir = std::env::temp_dir().join(format!("bose-connect-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good");
        let bad = dir.join("bad");
        fs::write(&good, "8\n").unwrap();
        fs::write(&bad, "eight").unwrap();
        assert_eq!(read_channel(&good), Some(8));
        assert_eq!(read_channel(&bad), None);
        assert_eq!(read_channel(&dir.join("missing")), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_firmware_channel_is_never_probed() {
        // 24 is SRfcomm: silent, on every model, most plausibly firmware
        // update. The protocol description says not to touch it.
        let src = include_str!("probe.rs");
        assert!(src.contains("![1, 8, 24].contains"));
        let order: Vec<u8> = [1u8, 8].into_iter().chain((1..=30).filter(|c| ![1, 8, 24].contains(c))).collect();
        assert!(!order.contains(&24));
        assert_eq!(&order[..2], &[1, 8]);
        assert_eq!(order.len(), 29);
    }
}
