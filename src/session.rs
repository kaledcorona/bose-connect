//! One exchange with a device.
//!
//! RFCOMM is a stream, not a datagram channel: a reply can arrive split across
//! reads, and two replies can arrive in one. Everything above this layer sees
//! whole records addressed to what it asked for.

use std::io::ErrorKind;

use crate::error::{Error, Result};
use crate::wire::{decode, Addr, Message, Operator, Refusal};
use crate::transport::{Recording, Transport};

/// Ten 47-byte mode records plus framing is the largest answer observed; this
/// is an order of magnitude above it.
const MAX_ENUMERATION: usize = 8192;

/// What a function said when asked to list itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// The opcodes it listed. An index, not an inventory: opcode `00` is never
    /// listed, and opcodes holding only zeros appear to be omitted.
    Records(Vec<u8>),
    /// It exists and will not list itself.
    Refused(Refusal),
    /// Nothing came back.
    Silent,
}

/// A conversation with a device.
pub struct Session<T: Transport> {
    transport: T,
    /// Bytes read but not yet consumed.
    pending: Vec<u8>,
}

impl<T: Transport> Session<T> {
    /// Open a session, sending the handshake first.
    ///
    /// Some devices answer nothing at all until `00 01 01 00` has been sent, so
    /// this is not optional politeness.
    pub fn open(mut transport: T) -> Result<Self> {
        transport.send(&Message::get(Addr::at(0x00, 0x01)).encode()?)?;
        let mut buf = [0u8; 512];
        // Wanted only for its side effect; a device that stays quiet here may
        // still answer everything afterwards.
        let _ = transport.recv(&mut buf);
        Ok(Session { transport, pending: Vec::new() })
    }

    /// Read one record. The payload, or the reason there is none.
    pub fn read(&mut self, addr: Addr) -> Result<Vec<u8>> {
        self.exchange(&Message::get(addr), addr)
    }

    /// Write one record, and report what the device answered.
    ///
    /// Discarding the reply reports a refusal as success — the caller sets a
    /// value, sees no error, and the device is unchanged. Silence is accepted:
    /// several writes acknowledge with nothing.
    pub fn write(&mut self, addr: Addr, payload: Vec<u8>) -> Result<()> {
        match self.exchange(&Message::set(addr, payload), addr) {
            Ok(_) | Err(Error::Silent(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Send one message and read until the record answering it arrives.
    ///
    /// A single `recv` is not a reply. Reading once and decoding whatever came
    /// back fails two ways on a stream socket — a reply split mid-record decodes
    /// as truncated, and one split on a record boundary leaves its tail to be
    /// served as the answer to the next request, which desynchronises the
    /// session silently and permanently.
    ///
    /// Matching on the address is the other half of that. A device volunteers
    /// notifications with operator `00`, and stopping at the first thing that
    /// parses hands one of those back as the answer to a question it never saw.
    pub fn exchange(&mut self, m: &Message, addr: Addr) -> Result<Vec<u8>> {
        self.transport.send(&m.encode()?)?;
        let mut chunk = [0u8; 1024];
        loop {
            if let Some(found) = self.take_matching(addr) {
                return found;
            }
            match self.transport.recv(&mut chunk) {
                // The peer closed. An empty buffer is not an empty reply.
                Ok(0) => {
                    return Err(Error::Io(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "device closed the connection",
                    )))
                }
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    if self.pending.is_empty() {
                        return Err(Error::Silent(addr));
                    }
                    // Something arrived but never completed, or never mentioned
                    // what was asked. Say so rather than returning a partial
                    // parse or another record's payload.
                    let leftover = std::mem::take(&mut self.pending);
                    return match decode(&leftover) {
                        Err(e) => Err(Error::Frame(e)),
                        Ok(_) => Err(Error::Silent(addr)),
                    };
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    /// The record for `addr` in what has been buffered so far, if the buffer
    /// parses whole. `None` means keep reading.
    fn take_matching(&mut self, addr: Addr) -> Option<Result<Vec<u8>>> {
        if self.pending.is_empty() {
            return None;
        }
        let msgs = decode(&self.pending).ok()?;
        let found = msgs.iter().find(|m| m.addr == addr).map(|m| match m.refusal() {
            Some(why) => Err(Error::Refused { addr, why }),
            None => Ok(m.payload.clone()),
        });
        // Consumed either way. Records for other addresses are notifications or
        // leftovers: keeping them would serve one as the next request's answer,
        // and giving up on them would abandon a reply still on its way.
        self.pending.clear();
        found
    }

    /// Ask a function to list itself, reading until it says it has finished.
    ///
    /// An enumeration does not fit one read: the mode table alone is 555 bytes
    /// against a much smaller MTU, and a prefix may parse cleanly while omitting
    /// most of the answer.
    ///
    /// A refusal is returned as itself rather than as an empty list — five
    /// functions refuse to enumerate while holding data, so "refuses" and "has
    /// nothing" must stay distinguishable.
    pub fn enumerate(&mut self, function: u8) -> Result<Vec<Message>> {
        self.transport.send(&Message::enumerate(function).encode()?)?;
        let terminator = [function, 0x01, 0x06, 0x00];
        let mut acc = std::mem::take(&mut self.pending);
        let mut chunk = [0u8; 1024];
        loop {
            // A device that answers without ever terminating would otherwise be
            // read until memory runs out; real hardware times out, but a
            // misbehaving one — or a test double that repeats its reply — does
            // not.
            if acc.len() > MAX_ENUMERATION {
                return Err(Error::Io(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "enumeration never terminated",
                )));
            }
            if acc.ends_with(&terminator) {
                break;
            }
            // A refusal is short, complete and has no terminator; stop on it
            // rather than waiting out the timeout.
            if acc.len() >= 5 && acc[..3] == [function, 0x01, 0x04] {
                break;
            }
            match self.transport.recv(&mut chunk) {
                Ok(0) => break,
                Ok(n) => acc.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::TimedOut => break,
                Err(e) => return Err(Error::Io(e)),
            }
        }
        if acc.is_empty() {
            return Ok(Vec::new());
        }
        Ok(decode(&acc)?)
    }

    /// What a function said when asked to list itself.
    ///
    /// The three outcomes are not interchangeable: a function that refuses to
    /// enumerate may still be full of data — five on the Ultra are — so a
    /// refusal must not read as an empty inventory.
    pub fn list(&mut self, function: u8) -> Result<Listing> {
        let reply = self.enumerate(function)?;
        if reply.is_empty() {
            return Ok(Listing::Silent);
        }
        if let Some(why) = reply.iter().find_map(|m| m.refusal()) {
            return Ok(Listing::Refused(why));
        }
        // Status records only. An enumeration opens with `<fn> 01 07 00`, a
        // zero-length result, and closes with the terminator; counting either as
        // content lists opcode 01 as a record that does not exist.
        Ok(Listing::Records(
            reply
                .iter()
                .filter(|m| m.operator == Operator::Status && m.addr.function == function)
                .map(|m| m.addr.opcode)
                .collect(),
        ))
    }
}

impl<T: Recording> Session<T> {
    /// What has been sent, for tests that assert on the wire.
    pub fn transport_sent(&self) -> &[Vec<u8>] {
        self.transport.sent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Replay;
    use crate::wire::Refusal;
    use std::io;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    const ANC: Addr = Addr::at(0x01, 0x06);

    #[test]
    fn opening_a_session_sends_the_handshake_first() {
        let t = Replay::new(vec![Some(h("00010305312e302e34"))]);
        let s = Session::open(t).unwrap();
        assert_eq!(s.transport_sent()[0], h("00010100"));
    }

    #[test]
    fn replays_a_qc35_noise_cancelling_read() {
        let t = Replay::new(vec![
            Some(h("00010305312e302e34")),   // handshake
            Some(h("01060302010b")),          // 01 06 -> level 01, 11 positions
        ]);
        let mut s = Session::open(t).unwrap();
        assert_eq!(s.read(ANC).unwrap(), vec![0x01, 0x0b]);
    }

    #[test]
    fn a_refused_opcode_is_reported_not_mistaken_for_data() {
        // An Ultra asked for the QC35's noise-cancelling opcode.
        let t = Replay::new(vec![Some(h("00010305312e322e30")), Some(h("0106040104"))]);
        let mut s = Session::open(t).unwrap();
        assert!(matches!(
            s.read(ANC),
            Err(Error::Refused { why: Refusal::OpcodeAbsent, .. })
        ));
    }

    #[test]
    fn silence_is_its_own_error_not_a_refusal() {
        // `01 07` on a QC35: no reply, no refusal, and the channel stays usable.
        let t = Replay::new(vec![Some(h("00010305312e302e34")), None, Some(h("01060302010b"))]);
        let mut s = Session::open(t).unwrap();
        assert!(matches!(s.read(Addr::at(0x01, 0x07)), Err(Error::Silent(_))));
        assert!(s.read(ANC).is_ok());
    }

    /// A transport that hands back one byte at a time.
    struct Dribble {
        reply: Vec<u8>,
        at: usize,
    }

    impl Transport for Dribble {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.reply.len() {
                return Err(io::Error::new(ErrorKind::TimedOut, "done"));
            }
            buf[0] = self.reply[self.at];
            self.at += 1;
            Ok(1)
        }
    }

    fn dribbling(reply: &str) -> Session<Dribble> {
        Session { transport: Dribble { reply: h(reply), at: 0 }, pending: Vec::new() }
    }

    #[test]
    fn a_reply_split_across_reads_is_still_one_reply() {
        // RFCOMM is a stream. Reading once and decoding whatever arrived fails
        // on any reply the kernel hands over in pieces.
        assert_eq!(dribbling("01060302010b").read(ANC).unwrap(), vec![0x01, 0x0b]);
    }

    #[test]
    fn an_unsolicited_record_is_not_served_as_the_answer() {
        // Operator 00 arrives without a request. Taking the first thing that
        // parses answers a question the device was never asked.
        let mut s = dribbling("030100030fbfe401060302010b");
        assert_eq!(s.read(ANC).unwrap(), vec![0x01, 0x0b]);
    }

    struct Closed;
    impl Transport for Closed {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn a_closed_connection_is_an_error_not_an_empty_reply() {
        // Ok(0) decoded to an empty message list, which contains no refusal,
        // which made every capability probe answer "present". Headphones that
        // power off mid-probe produced a plausible-looking device.
        let mut s = Session { transport: Closed, pending: Vec::new() };
        let Err(Error::Io(e)) = s.read(Addr::at(0x00, 0x03)) else {
            panic!("expected an io error");
        };
        assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn an_enumeration_arriving_in_pieces_is_reassembled() {
        // Ten mode records are 555 bytes against an MTU well below that, so an
        // enumeration always arrives split. A prefix parses cleanly while
        // omitting most of the answer, so a single read looks like success.
        let mut whole = String::new();
        for i in 0..3u8 {
            whole.push_str(&format!("1f060302{i:02x}01"));
        }
        whole.push_str("1f010600");
        let msgs = dribbling(&whole).enumerate(0x1f).unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].payload, vec![0x00, 0x01]);
    }

    /// A device that answers and answers and never says it has finished.
    struct Flood;
    impl Transport for Flood {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            buf[..6].copy_from_slice(&[0x1f, 0x06, 0x03, 0x02, 0x00, 0x01]);
            Ok(6)
        }
    }

    #[test]
    fn an_enumeration_that_never_ends_is_an_error_not_a_hang() {
        let mut s = Session { transport: Flood, pending: Vec::new() };
        let Err(Error::Io(e)) = s.enumerate(0x1f) else {
            panic!("expected an io error");
        };
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn an_enumeration_lists_opcodes_and_drops_its_terminator() {
        let mut s = dribbling("010203050041424344010403013c01010600");
        assert_eq!(
            s.list(0x01).unwrap(),
            Listing::Records(vec![0x02, 0x04])
        );
    }

    #[test]
    fn a_write_reports_a_refusal_rather_than_succeeding_quietly() {
        let t = Replay::new(vec![Some(h("00010305312e302e34")), Some(h("0106040106"))]);
        let mut s = Session::open(t).unwrap();
        assert!(matches!(
            s.write(ANC, vec![0x02]),
            Err(Error::Refused { why: Refusal::BadArgument, .. })
        ));
    }

    #[test]
    fn a_write_that_is_acknowledged_with_nothing_still_succeeds() {
        let t = Replay::new(vec![Some(h("00010305312e302e34")), None]);
        let mut s = Session::open(t).unwrap();
        assert!(s.write(ANC, vec![0x01]).is_ok());
    }

}
