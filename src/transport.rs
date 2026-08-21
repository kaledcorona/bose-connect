//! Getting bytes to and from a device.
//!
//! The only part that needs hardware. Everything above it is written against
//! [`Transport`], so the rest of the crate can be tested by replaying recorded
//! traffic — which matters here, because exercising the whole protocol needs
//! four headphones from two generations and most people have one.

use std::io::{self, ErrorKind};
use std::time::Duration;

use crate::framing::{decode, Message};

/// A byte channel to a device.
pub trait Transport {
    fn send(&mut self, data: &[u8]) -> io::Result<()>;
    /// Everything sent so far. Only test transports record it; a real socket
    /// returns an empty slice.
    fn sent(&self) -> &[Vec<u8>] {
        &[]
    }
    /// Returns however many bytes arrived. A timeout is `ErrorKind::TimedOut`,
    /// which callers must distinguish from a refusal: the device answering
    /// nothing at all is a third case, not a slow refusal.
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

// ---------------------------------------------------------------------------
// RFCOMM
// ---------------------------------------------------------------------------

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
        // SAFETY: plain socket syscalls; the fd is closed in Drop.
        let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_STREAM, BTPROTO_RFCOMM) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let me = Rfcomm { fd };
        me.set_timeout(timeout)?;

        // The kernel wants the address in reverse octet order.
        let mut bd = addr.0;
        bd.reverse();
        let sa = SockaddrRc {
            rc_family: AF_BLUETOOTH as libc::sa_family_t,
            rc_bdaddr: bd,
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

/// Find the channel a device answers control traffic on.
///
/// It differs per model — 8 on a QuietComfort 35 and Sport Earbuds, 1 on the
/// Ultra family — and neither the SDP record nor the device id predicts it. The
/// only reliable method is to ask, with a read that writes nothing.
///
/// Scan once and keep the answer. Repeated full scans appeared to leave a
/// device unresponsive until it was power-cycled; unproven, but cheap to avoid.
pub fn probe_channel(addr: Address, timeout: Duration) -> Option<(u8, Session<Rfcomm>)> {
    for channel in 1..=30u8 {
        let Ok(sock) = Rfcomm::connect(addr, channel, timeout) else {
            continue;
        };
        let Ok(mut session) = Session::open(sock) else {
            continue;
        };
        // Device id: four bytes out, seven back, nothing changed.
        if let Ok(Some(msgs)) = session.request(&Message::get(0x00, 0x03)) {
            if msgs
                .iter()
                .any(|m| m.function == 0x00 && m.opcode == 0x03 && m.refusal().is_none())
            {
                // Hand back the open session rather than the number. Closing and
                // reconnecting races the kernel releasing the channel, which
                // surfaces as EBUSY on the very next connect.
                return Some((channel, session));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A conversation with a device.
pub struct Session<T: Transport> {
    transport: T,
}

impl<T: Transport> Session<T> {
    /// Open a session, sending the handshake first.
    ///
    /// Some devices answer nothing at all until `00 01 01 00` has been sent, so
    /// this is not optional politeness.
    pub fn open(mut transport: T) -> io::Result<Self> {
        transport.send(&Message::get(0x00, 0x01).encode().expect("fixed length"))?;
        let mut buf = [0u8; 512];
        // The reply is wanted only for its side effect; a device that stays
        // quiet here may still answer everything afterwards.
        let _ = transport.recv(&mut buf);
        Ok(Session { transport })
    }

    /// Send one message and decode whatever comes back.
    ///
    /// Returns `Ok(None)` on silence, which is a real answer and not an error:
    /// some opcodes neither reply nor refuse.
    /// What has been sent, for tests that assert on the wire.
    pub fn transport_sent(&self) -> &[Vec<u8>] {
        self.transport.sent()
    }

    pub fn request(&mut self, m: &Message) -> io::Result<Option<Vec<Message>>> {
        self.transport.send(&m.encode().map_err(to_io)?)?;
        let mut buf = [0u8; 2048];
        match self.transport.recv(&mut buf) {
            Ok(n) => Ok(Some(decode(&buf[..n]).map_err(to_io)?)),
            Err(e) if e.kind() == ErrorKind::TimedOut => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl<T: Transport> Session<T> {
    /// Ask a function to list itself, reading until it says it has finished.
    ///
    /// An enumeration does not fit one read: the mode table alone is 555 bytes
    /// against an RFCOMM MTU far smaller. A single `recv` returns a prefix that
    /// may well parse cleanly — ending on a record boundary — and silently omit
    /// most of the answer, which is how this first appeared: an empty list of
    /// modes on a device with five.
    pub fn enumerate(&mut self, function: u8) -> io::Result<Vec<Message>> {
        self.transport
            .send(&Message::enumerate(function).encode().map_err(to_io)?)?;
        let terminator = [function, 0x01, 0x06, 0x00];
        let mut acc: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match self.transport.recv(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    acc.extend_from_slice(&chunk[..n]);
                    if acc.ends_with(&terminator) {
                        break;
                    }
                }
                // Silence after something arrived means the device has said all
                // it intends to, terminator or not.
                Err(e) if e.kind() == ErrorKind::TimedOut => break,
                Err(e) => return Err(e),
            }
        }
        if acc.is_empty() {
            return Ok(Vec::new());
        }
        decode(&acc).map_err(to_io)
    }
}

fn to_io(e: crate::framing::Error) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, e.to_string())
}

// ---------------------------------------------------------------------------
// Replay, for tests
// ---------------------------------------------------------------------------

/// A transport that answers from a script instead of a device.
///
/// This is the reason [`Transport`] is a trait. Recorded exchanges from four
/// models exercise code paths nobody could otherwise reach without owning all
/// four.
pub struct Replay {
    pub replies: Vec<Option<Vec<u8>>>,
    pub sent: Vec<Vec<u8>>,
    next: usize,
}

impl Replay {
    /// Each entry is the reply to the next request; `None` means silence.
    pub fn new(replies: Vec<Option<Vec<u8>>>) -> Self {
        Replay { replies, sent: Vec::new(), next: 0 }
    }
}

impl Transport for Replay {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.sent.push(data.to_vec());
        Ok(())
    }

    fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let reply = self.replies.get(self.next).cloned().flatten();
        self.next += 1;
        match reply {
            Some(bytes) => {
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            None => Err(io::Error::new(ErrorKind::TimedOut, "scripted silence")),
        }
    }
}

/// A transport that answers by request rather than by turn.
///
/// [`Replay`] is sequential, which is unreadable once a test drives sixty-four
/// capability probes. Here a test scripts only the records a model actually
/// has; anything unscripted is refused with "function absent", which is exactly
/// how a device behaves for the functions it does not implement.
pub struct Scripted {
    table: std::collections::HashMap<Vec<u8>, Option<Vec<u8>>>,
    last: Vec<u8>,
    log: Vec<Vec<u8>>,
}

impl Scripted {
    pub fn new() -> Self {
        Scripted { table: std::collections::HashMap::new(), last: Vec::new(), log: Vec::new() }
    }

    /// Answer `request` with `reply`. Both are raw bytes.
    pub fn on(mut self, request: &[u8], reply: &[u8]) -> Self {
        self.table.insert(request.to_vec(), Some(reply.to_vec()));
        self
    }

    /// Answer `request` with nothing at all — the third case.
    pub fn silent(mut self, request: &[u8]) -> Self {
        self.table.insert(request.to_vec(), None);
        self
    }
}

impl Default for Scripted {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for Scripted {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.last = data.to_vec();
        self.log.push(data.to_vec());
        Ok(())
    }

    fn sent(&self) -> &[Vec<u8>] {
        &self.log
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let reply = match self.table.get(&self.last) {
            Some(Some(bytes)) => bytes.clone(),
            Some(None) => return Err(io::Error::new(ErrorKind::TimedOut, "scripted silence")),
            // Unscripted: refuse as a device does for a function it lacks.
            None if self.last.len() >= 2 => {
                vec![self.last[0], self.last[1], 0x04, 0x01, 0x03]
            }
            None => return Err(io::Error::new(ErrorKind::TimedOut, "no script")),
        };
        buf[..reply.len()].copy_from_slice(&reply);
        Ok(reply.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::{Operator, Refusal};

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

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
    fn opening_a_session_sends_the_handshake_first() {
        let t = Replay::new(vec![Some(hex("00010305312e302e34"))]);
        let s = Session::open(t).unwrap();
        assert_eq!(s.transport.sent[0], hex("00010100"));
    }

    #[test]
    fn replays_a_qc35_noise_cancelling_read() {
        let t = Replay::new(vec![
            Some(hex("00010305312e302e34")),   // handshake
            Some(hex("01060302010b")),          // 01 06 -> level 01, 11 positions
        ]);
        let mut s = Session::open(t).unwrap();
        let reply = s.request(&crate::framing::Message::get(0x01, 0x06)).unwrap().unwrap();
        assert_eq!(reply[0].operator, Operator::Status);
        assert_eq!(reply[0].payload, vec![0x01, 0x0b]);
    }

    #[test]
    fn a_refused_opcode_is_reported_not_mistaken_for_data() {
        // An Ultra asked for the QC35's noise-cancelling opcode.
        let t = Replay::new(vec![Some(hex("00010305312e322e30")), Some(hex("0106040104"))]);
        let mut s = Session::open(t).unwrap();
        let reply = s.request(&crate::framing::Message::get(0x01, 0x06)).unwrap().unwrap();
        assert_eq!(reply[0].refusal(), Some(Refusal::OpcodeAbsent));
    }

    #[test]
    fn silence_is_none_not_an_error() {
        // `01 07` on a QC35: no reply, no refusal, and the channel stays usable.
        let t = Replay::new(vec![Some(hex("00010305312e302e34")), None, Some(hex("01060302010b"))]);
        let mut s = Session::open(t).unwrap();
        assert!(s.request(&crate::framing::Message::get(0x01, 0x07)).unwrap().is_none());
        assert!(s.request(&crate::framing::Message::get(0x01, 0x06)).unwrap().is_some());
    }
}
