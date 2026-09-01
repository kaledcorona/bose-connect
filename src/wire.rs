//! Message framing.
//!
//! Every exchange is `<function> <opcode> <operator> <length> <payload…>`, and a
//! reply may pack several records back to back. Length is a single octet, so a
//! record never exceeds 259 bytes.
//!
//! Nothing here talks to a device. Framing is pure, which is what makes it
//! testable against recorded traffic rather than against hardware nobody has.

use std::fmt;

/// Where a value lives: a function and an opcode.
///
/// The pair is the protocol's only address, and it is what errors, the catalog
/// and the device surface are all keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr {
    pub function: u8,
    pub opcode: u8,
}

impl Addr {
    pub const fn at(function: u8, opcode: u8) -> Self {
        Addr { function, opcode }
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02x} {:02x}", self.function, self.opcode)
    }
}

/// What a message is asking for, or answering with.
///
/// A refusal is a well-formed record — it parses like any other — so a caller
/// that tests only for "did I get bytes back" treats errors as data. That
/// mistake is easy enough to make that [`Record::refusal`] exists to name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// Wire name `Set`: a write with no reply. This client never sends one — it
    /// writes with [`Operator::SetGet`] to have the change confirmed — so every
    /// `0x00` it sees arrives *from* the device, unsolicited, which is a
    /// notification. Named for that inbound use.
    Notify,
    Get,
    /// Wire name `SetGet`: write, and return the new value. This is why a
    /// confirmed write echoes back the record it set.
    SetGet,
    /// A reply carrying state.
    Status,
    /// Refused. The payload says why; see [`Refusal`].
    Error,
    /// Begins an enumeration, and starts some operations.
    Start,
    /// `Result`: the record that closes an enumeration, and the reply to an
    /// operation. Zero length at the end of a listing.
    Result,
    /// `Processing`: accepted, working. Acknowledges an enumeration before its
    /// records arrive, and a long operation before its outcome.
    Processing,
    Unknown(u8),
}

impl From<u8> for Operator {
    fn from(b: u8) -> Self {
        match b {
            0x00 => Operator::Notify,
            0x01 => Operator::Get,
            0x02 => Operator::SetGet,
            0x03 => Operator::Status,
            0x04 => Operator::Error,
            0x05 => Operator::Start,
            0x06 => Operator::Result,
            0x07 => Operator::Processing,
            other => Operator::Unknown(other),
        }
    }
}

impl From<Operator> for u8 {
    fn from(o: Operator) -> u8 {
        match o {
            Operator::Notify => 0x00,
            Operator::Get => 0x01,
            Operator::SetGet => 0x02,
            Operator::Status => 0x03,
            Operator::Error => 0x04,
            Operator::Start => 0x05,
            Operator::Result => 0x06,
            Operator::Processing => 0x07,
            Operator::Unknown(b) => b,
        }
    }
}

/// Why a request was refused.
///
/// The distinction matters: `FunctionAbsent` means stop probing that function
/// entirely, `OpcodeAbsent` means the function is real and worth exploring
/// further.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    FunctionAbsent,
    OpcodeAbsent,
    /// The operator is not valid for this opcode.
    BadOperator,
    /// The value is outside what the device accepts.
    BadArgument,
    Other(u8),
    /// A refusal with no reason byte at all.
    Unspecified,
}

impl From<u8> for Refusal {
    fn from(b: u8) -> Self {
        match b {
            0x03 => Refusal::FunctionAbsent,
            0x04 => Refusal::OpcodeAbsent,
            0x05 => Refusal::BadOperator,
            0x06 => Refusal::BadArgument,
            other => Refusal::Other(other),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::FunctionAbsent => "this device has no such function",
            Refusal::OpcodeAbsent => "this device has no such setting",
            Refusal::BadOperator => "that operation is not allowed on this setting",
            Refusal::BadArgument => "this device does not accept that value",
            Refusal::Unspecified => "refused, with no reason given",
            Refusal::Other(_) => "refused, for an unrecognised reason",
        })
    }
}

/// One record, borrowing the buffer it was read from.
///
/// Parsing borrows and building owns: a reply is scanned in place and only the
/// one record a caller wanted is copied out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub addr: Addr,
    pub operator: Operator,
    pub payload: &'a [u8],
}

impl<'a> Record<'a> {
    /// The refusal reason, if this is one.
    pub fn refusal(&self) -> Option<Refusal> {
        match (self.operator, self.payload.first()) {
            (Operator::Error, Some(&b)) => Some(Refusal::from(b)),
            (Operator::Error, None) => Some(Refusal::Unspecified),
            _ => None,
        }
    }

    /// Whether this closes an enumeration. The terminator is a zero-length
    /// `Result`; the `Processing` that opens the listing is not it.
    pub fn is_terminator(&self) -> bool {
        self.operator == Operator::Result && self.payload.is_empty()
    }

    pub fn to_message(self) -> Message {
        Message {
            addr: self.addr,
            operator: self.operator,
            payload: self.payload.to_vec(),
        }
    }
}

/// One record, owned. What a request is built as, and what crosses an API
/// boundary once the buffer it came from is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub addr: Addr,
    pub operator: Operator,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn new(addr: Addr, operator: Operator, payload: Vec<u8>) -> Self {
        Message { addr, operator, payload }
    }

    /// A read with no arguments, which is most reads.
    pub fn get(addr: Addr) -> Self {
        Message::new(addr, Operator::Get, Vec::new())
    }

    /// A write. Uses `SetGet`, so the device echoes the record it set — which is
    /// how a write is confirmed rather than assumed.
    pub fn set(addr: Addr, payload: Vec<u8>) -> Self {
        Message::new(addr, Operator::SetGet, payload)
    }

    /// Asks a function to list itself.
    pub fn enumerate(function: u8) -> Self {
        Message::new(Addr::at(function, 0x01), Operator::Start, Vec::new())
    }

    pub fn as_record(&self) -> Record<'_> {
        Record { addr: self.addr, operator: self.operator, payload: &self.payload }
    }

    pub fn refusal(&self) -> Option<Refusal> {
        self.as_record().refusal()
    }

    pub fn is_terminator(&self) -> bool {
        self.as_record().is_terminator()
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let len = u8::try_from(self.payload.len())
            .map_err(|_| Error::PayloadTooLong(self.payload.len()))?;
        let mut out = Vec::with_capacity(4 + self.payload.len());
        out.extend_from_slice(&[self.addr.function, self.addr.opcode, self.operator.into(), len]);
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} [{}]", self.addr, self.operator, hex(&self.payload))
    }
}

/// Space-separated hex, the way every capture in the reference is written.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Fewer than four bytes remain: not even a header.
    Truncated { at: usize },
    /// A record claims more payload than the buffer holds.
    ShortPayload { at: usize, want: usize, have: usize },
    PayloadTooLong(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { at } => write!(f, "truncated header at offset {at}"),
            Error::ShortPayload { at, want, have } => {
                write!(f, "record at {at} wants {want} payload bytes, {have} available")
            }
            Error::PayloadTooLong(n) => write!(f, "payload of {n} bytes exceeds 255"),
        }
    }
}

impl std::error::Error for Error {}

/// Walk a reply record by record, without copying any of it.
pub fn records(buf: &[u8]) -> Records<'_> {
    Records { buf, at: 0 }
}

pub struct Records<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Record<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at >= self.buf.len() {
            return None;
        }
        let (i, len) = (self.at, self.buf.len());
        // A malformed buffer yields its error once and then ends. Leaving the
        // cursor where it was would hand the same error out forever, and any
        // `collect` over it would never return.
        self.at = len;
        if len - i < 4 {
            return Some(Err(Error::Truncated { at: i }));
        }
        let want = self.buf[i + 3] as usize;
        let start = i + 4;
        if len < start + want {
            return Some(Err(Error::ShortPayload { at: i, want, have: len - start }));
        }
        self.at = start + want;
        Some(Ok(Record {
            addr: Addr::at(self.buf[i], self.buf[i + 1]),
            operator: Operator::from(self.buf[i + 2]),
            payload: &self.buf[start..start + want],
        }))
    }
}

/// Split a reply into its records.
///
/// A reply that does not consume exactly is a sign the framing assumption is
/// wrong for that traffic — worth knowing immediately rather than after
/// building on a partial parse, so this errors instead of returning what it
/// managed.
pub fn decode(buf: &[u8]) -> Result<Vec<Message>, Error> {
    records(buf).map(|r| r.map(Record::to_message)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn encodes_a_read() {
        // The session handshake: every device answers this, and some answer
        // nothing until it has been sent.
        assert_eq!(Message::get(Addr::at(0x00, 0x01)).encode().unwrap(), h("00010100"));
    }

    #[test]
    fn encodes_a_write() {
        // Noise cancelling to low on a QC35.
        let m = Message::set(Addr::at(0x01, 0x06), vec![0x03]);
        assert_eq!(m.encode().unwrap(), h("0106020103"));
    }

    #[test]
    fn decodes_a_version_reply() {
        let msgs = decode(&h("00010305312e302e34")).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].operator, Operator::Status);
        assert_eq!(String::from_utf8_lossy(&msgs[0].payload), "1.0.4");
    }

    #[test]
    fn decodes_a_multi_record_enumeration() {
        // A QC35 answering `01 01 05 00`: five records and a terminator, with
        // the name redacted to keep a real device out of the fixtures.
        let stream = h(concat!(
            "01010700",              // result, zero length
            "010203050041424344",    // name: a leading 00, then "ABCD"
            "01030305a10004c3de",    // language and flags
            "010403013c",            // auto-off, 60 minutes
            "01060302010b",          // noise cancelling
            "01010600",              // terminator
        ));
        let msgs = decode(&stream).unwrap();
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[1].payload, h("0041424344"));
        assert!(msgs.last().unwrap().is_terminator());
    }

    #[test]
    fn a_refusal_parses_like_any_record_and_is_named() {
        // 04 with payload 03: the function does not exist on this model.
        let m = &decode(&h("1f03040103")).unwrap()[0];
        assert_eq!(m.refusal(), Some(Refusal::FunctionAbsent));
        // 04 with payload 04: the function exists, the opcode does not.
        let m = &decode(&h("000f040104")).unwrap()[0];
        assert_eq!(m.refusal(), Some(Refusal::OpcodeAbsent));
        // Anything else is not a refusal, however it looks.
        assert_eq!(decode(&h("01060302010b")).unwrap()[0].refusal(), None);
    }

    #[test]
    fn selecting_a_mode_is_a_write_that_does_not_use_set() {
        // `1f 03` takes operator 0x05, not 0x02. Filtering captures for writes
        // by operator 0x02 alone misses it, which is how it was nearly missed.
        let m = Message::new(Addr::at(0x1f, 0x03), Operator::Start, vec![0x00, 0x01]);
        assert_eq!(m.encode().unwrap(), h("1f0305020001"));
    }

    #[test]
    fn a_short_payload_is_an_error_not_a_partial_parse() {
        // Claims five payload bytes, supplies two.
        assert!(matches!(
            decode(&h("000103050102")),
            Err(Error::ShortPayload { at: 0, want: 5, have: 2 })
        ));
    }

    #[test]
    fn a_stray_trailing_byte_is_an_error() {
        assert!(matches!(decode(&h("0001010000")), Err(Error::Truncated { at: 4 })));
    }

    #[test]
    fn records_are_walked_without_copying_the_buffer() {
        let buf = h("01060302010b010403013c");
        let got: Vec<&[u8]> = records(&buf).map(|r| r.unwrap().payload).collect();
        assert_eq!(got, vec![&[0x01u8, 0x0b][..], &[0x3c][..]]);
    }

    #[test]
    fn a_malformed_buffer_yields_its_error_once_and_stops() {
        // The cursor has to advance past a failure, or the iterator hands the
        // same error out forever and any collect over it never returns.
        let buf = h("000103050102");
        let mut it = records(&buf);
        assert!(it.next().unwrap().is_err());
        assert!(it.next().is_none());
    }
}
