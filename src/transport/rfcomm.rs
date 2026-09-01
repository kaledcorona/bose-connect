//! The Linux socket.
//!
//! The only part that needs hardware, and the only part that is unsafe. Every
//! comment below records something that cost an afternoon.

use std::io::{self, ErrorKind};
use std::time::Duration;

use super::Transport;

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_RFCOMM: libc::c_int = 3;

// Not packed. `sa_family_t` is a u16, so the struct aligns to two and C pads it
// to ten bytes; declaring it packed yields nine and the kernel rejects the
// address as malformed — with ENOTCONN or EINVAL rather than anything that
// points at the cause.
#[repr(C)]
struct SockaddrRc {
    rc_family: libc::sa_family_t,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

/// A Bluetooth address, written the way people write it: `AA:BB:CC:DD:EE:FF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address(pub [u8; 6]);

impl std::str::FromStr for Address {
    type Err = io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 6];
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            return Err(io::Error::new(ErrorKind::InvalidInput, "expected six octets"));
        }
        for (i, p) in parts.iter().enumerate() {
            out[i] = u8::from_str_radix(p, 16)
                .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "bad octet"))?;
        }
        Ok(Address(out))
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s: Vec<String> = self.0.iter().map(|b| format!("{b:02X}")).collect();
        write!(f, "{}", s.join(":"))
    }
}

pub struct Rfcomm {
    fd: libc::c_int,
}

impl Rfcomm {
    /// Connect to one channel.
    ///
    /// `ECONNREFUSED` here usually means the channel exists but something else
    /// holds it — the official phone app takes one and keeps it. Try another.
    pub fn connect(addr: Address, channel: u8, timeout: Duration) -> io::Result<Self> {
        // A zero timeout is not "no timeout" to the kernel — `SO_RCVTIMEO` of
        // zero means block forever, so a caller asking for the fastest possible
        // scan would get one that never returns.
        if timeout.is_zero() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "a zero timeout blocks forever; pass a duration",
            ));
        }
        // SAFETY: plain socket syscalls; the fd is closed in Drop.
        // CLOEXEC so an exec in another thread does not inherit the channel —
        // the device allows one holder and a leaked fd keeps it after we exit.
        let fd = unsafe {
            libc::socket(
                AF_BLUETOOTH,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                BTPROTO_RFCOMM,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let me = Rfcomm { fd };
        me.set_timeout(timeout)?;

        let sa = SockaddrRc {
            rc_family: AF_BLUETOOTH as libc::sa_family_t,
            rc_bdaddr: bdaddr(addr),
            rc_channel: channel,
        };
        let rc = unsafe {
            libc::connect(
                me.fd,
                &sa as *const SockaddrRc as *const libc::sockaddr,
                std::mem::size_of::<SockaddrRc>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(me)
    }

    fn set_timeout(&self, d: Duration) -> io::Result<()> {
        let tv = libc::timeval {
            tv_sec: d.as_secs() as libc::time_t,
            tv_usec: d.subsec_micros() as libc::suseconds_t,
        };
        for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            let rc = unsafe {
                libc::setsockopt(
                    self.fd,
                    libc::SOL_SOCKET,
                    opt,
                    &tv as *const libc::timeval as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for Rfcomm {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

impl Transport for Rfcomm {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        let n = unsafe {
            libc::send(self.fd, data.as_ptr() as *const libc::c_void, data.len(), 0)
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else if n as usize != data.len() {
            Err(io::Error::new(ErrorKind::WriteZero, "short write"))
        } else {
            Ok(())
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe {
            libc::recv(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0)
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            // A socket timeout arrives as EAGAIN; name it for what it is.
            if e.raw_os_error() == Some(libc::EAGAIN) || e.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Err(io::Error::new(ErrorKind::TimedOut, "no reply"));
            }
            Err(e)
        } else {
            Ok(n as usize)
        }
    }
}

/// The kernel wants the six octets reversed: the opposite of how an address is
/// written and of how bluez prints it. Getting this backwards connects to
/// nothing, with no error that says why.
pub(super) fn bdaddr(addr: Address) -> [u8; 6] {
    let mut bd = addr.0;
    bd.reverse();
    bd
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_round_trips() {
        let a: Address = "AA:BB:CC:00:00:01".parse().unwrap();
        assert_eq!(a.0, [0xaa, 0xbb, 0xcc, 0x00, 0x00, 0x01]);
        assert_eq!(a.to_string(), "AA:BB:CC:00:00:01");
    }

    #[test]
    fn a_bad_address_is_rejected_not_guessed() {
        assert!("AA:BB:CC".parse::<Address>().is_err());
        assert!("AA:BB:CC:00:00:ZZ".parse::<Address>().is_err());
    }

    #[test]
    fn the_kernel_wants_the_address_reversed() {
        let a: Address = "AA:BB:CC:00:00:01".parse().unwrap();
        assert_eq!(bdaddr(a), [0x01, 0x00, 0x00, 0xcc, 0xbb, 0xaa]);
    }

    #[test]
    fn a_zero_timeout_is_rejected_because_it_means_block_forever() {
        let a: Address = "AA:BB:CC:00:00:01".parse().unwrap();
        let Err(e) = Rfcomm::connect(a, 1, Duration::ZERO) else {
            panic!("a zero timeout was accepted");
        };
        assert_eq!(e.kind(), ErrorKind::InvalidInput);
    }
}
