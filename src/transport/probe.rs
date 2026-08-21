//! Finding the channel.

use std::io::{self, ErrorKind};
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
    for channel in 1..=30u8 {
        // 24 is `SRfcomm`, silent, present on every model and most plausibly
        // firmware update. The protocol description says not to probe it; this
        // scan was writing a handshake there on every run.
        if channel == 24 {
            continue;
        }
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



#[cfg(test)]
mod tests {

    #[test]
    fn the_firmware_channel_is_never_probed() {
        // 24 is SRfcomm: silent, on every model, most plausibly firmware
        // update. The protocol description says not to touch it.
        let src = include_str!("probe.rs");
        assert!(src.contains("if channel == 24"));
    }
}
