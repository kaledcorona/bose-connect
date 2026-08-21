//! Message framing.
//!
//! Every exchange is `<function> <opcode> <operator> <length> <payload…>`, and a
//! reply may pack several records back to back. Length is a single octet, so a
//! record never exceeds 259 bytes.
//!
//! Nothing here talks to a device. Framing is pure, which is what makes it
//! testable against recorded traffic rather than against hardware nobody has.

use std::fmt;

/// What a message is asking for, or answering with.
///
/// A refusal is a well-formed record — it parses like any other — so a caller
/// that tests only for "did I get bytes back" treats errors as data. That
/// mistake is easy enough to make that [`Message::refusal`] exists to name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// Unsolicited; the device volunteered this.
    Notify,
    Get,
    Set,
    /// A reply carrying state.
    Status,
    /// Refused. The payload says why; see [`Refusal`].
    Error,
    /// Begins an enumeration, and starts some operations.
    Start,
    /// Ends an enumeration. Zero length.
    End,
    Result,
    Unknown(u8),
}

impl From<u8> for Operator {
    fn from(b: u8) -> Self {
        match b {
            0x00 => Operator::Notify,
            0x01 => Operator::Get,
            0x02 => Operator::Set,
            0x03 => Operator::Status,
            0x04 => Operator::Error,
            0x05 => Operator::Start,
            0x06 => Operator::End,
            0x07 => Operator::Result,
            other => Operator::Unknown(other),
        }
    }
}

impl From<Operator> for u8 {
    fn from(o: Operator) -> u8 {
        match o {
            Operator::Notify => 0x00,
            Operator::Get => 0x01,
            Operator::Set => 0x02,
            Operator::Status => 0x03,
            Operator::Error => 0x04,
            Operator::Start => 0x05,
            Operator::End => 0x06,
            Operator::Result => 0x07,
            Operator::Unknown(b) => b,
        }
    }
}

/// Why a request was refused.
///
/// Only two of the observed payloads are understood. The distinction matters:
/// `FunctionAbsent` means stop probing that function entirely, `OpcodeAbsent`
/// means the function is real and worth exploring further.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    FunctionAbsent,
    OpcodeAbsent,
    Other(u8),
}

impl From<u8> for Refusal {
    fn from(b: u8) -> Self {
        match b {
            0x03 => Refusal::FunctionAbsent,
            0x04 => Refusal::OpcodeAbsent,
            other => Refusal::Other(other),
        }
    }
}

/// One record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub function: u8,
    pub opcode: u8,
    pub operator: Operator,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn new(function: u8, opcode: u8, operator: Operator, payload: Vec<u8>) -> Self {
        Message { function, opcode, operator, payload }
    }

    /// A read with no arguments, which is most reads.
    pub fn get(function: u8, opcode: u8) -> Self {
        Message::new(function, opcode, Operator::Get, Vec::new())
    }

    pub fn set(function: u8, opcode: u8, payload: Vec<u8>) -> Self {
        Message::new(function, opcode, Operator::Set, payload)
    }

    /// Asks a function to list itself.
    pub fn enumerate(function: u8) -> Self {
        Message::new(function, 0x01, Operator::Start, Vec::new())
    }

    /// The refusal reason, if this is one.
    pub fn refusal(&self) -> Option<Refusal> {
        match (self.operator, self.payload.first()) {
            (Operator::Error, Some(&b)) => Some(Refusal::from(b)),
            (Operator::Error, None) => Some(Refusal::Other(0)),
            _ => None,
        }
    }

    /// Whether this closes an enumeration.
    pub fn is_terminator(&self) -> bool {
        self.operator == Operator::End && self.payload.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let len = u8::try_from(self.payload.len())
            .map_err(|_| Error::PayloadTooLong(self.payload.len()))?;
        let mut out = Vec::with_capacity(4 + self.payload.len());
        out.extend_from_slice(&[self.function, self.opcode, self.operator.into(), len]);
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x} {:02x} {:?} [{}]",
            self.function,
            self.opcode,
            self.operator,
            self.payload.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        )
    }
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

/// Split a reply into its records.
///
/// A reply that does not consume exactly is a sign the framing assumption is
/// wrong for that traffic — worth knowing immediately rather than after
/// building on a partial parse, so this errors instead of returning what it
/// managed.
pub fn decode(buf: &[u8]) -> Result<Vec<Message>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if buf.len() - i < 4 {
            return Err(Error::Truncated { at: i });
        }
        let len = buf[i + 3] as usize;
        let start = i + 4;
        if buf.len() < start + len {
            return Err(Error::ShortPayload {
                at: i,
                want: len,
                have: buf.len() - start,
            });
        }
        out.push(Message::new(
            buf[i],
            buf[i + 1],
            Operator::from(buf[i + 2]),
            buf[start..start + len].to_vec(),
        ));
        i = start + len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn encodes_a_read() {
        // The session handshake: every device answers this, and some answer
        // nothing until it has been sent.
        assert_eq!(Message::get(0x00, 0x01).encode().unwrap(), hex("00010100"));
    }

    #[test]
    fn encodes_a_write() {
        // Noise cancelling to low on a QC35.
        let m = Message::set(0x01, 0x06, vec![0x03]);
        assert_eq!(m.encode().unwrap(), hex("0106020103"));
    }

    #[test]
    fn decodes_a_version_reply() {
        let msgs = decode(&hex("00010305312e302e34")).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].operator, Operator::Status);
        assert_eq!(String::from_utf8_lossy(&msgs[0].payload), "1.0.4");
    }

    #[test]
    fn decodes_a_multi_record_enumeration() {
        // A QC35 answering `01 01 05 00`: five records and a terminator, with
        // the name redacted to keep a real device out of the fixtures.
        let stream = hex(concat!(
            "01010700",              // result, zero length
            "010203050041424344",    // name: a leading 00, then "ABCD"
            "01030305a10004c3de",    // language and flags
            "010403013c",            // auto-off, 60 minutes
            "01060302010b",          // noise cancelling
            "01010600",              // terminator
        ));
        let msgs = decode(&stream).unwrap();
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[1].payload, hex("0041424344"));
        assert!(msgs.last().unwrap().is_terminator());
    }

    #[test]
    fn a_refusal_parses_like_any_record_and_is_named() {
        // 04 with payload 03: the function does not exist on this model.
        let m = &decode(&hex("1f03040103")).unwrap()[0];
        assert_eq!(m.refusal(), Some(Refusal::FunctionAbsent));
        // 04 with payload 04: the function exists, the opcode does not.
        let m = &decode(&hex("000f040104")).unwrap()[0];
        assert_eq!(m.refusal(), Some(Refusal::OpcodeAbsent));
        // Anything else is not a refusal, however it looks.
        assert_eq!(decode(&hex("01060302010b")).unwrap()[0].refusal(), None);
    }

    #[test]
    fn selecting_a_mode_is_a_write_that_does_not_use_set() {
        // `1f 03` takes operator 0x05, not 0x02. Filtering captures for writes
        // by operator 0x02 alone misses it, which is how it was nearly missed.
        let m = Message::new(0x1f, 0x03, Operator::Start, vec![0x00, 0x01]);
        assert_eq!(m.encode().unwrap(), hex("1f0305020001"));
    }

    #[test]
    fn a_short_payload_is_an_error_not_a_partial_parse() {
        // Claims five payload bytes, supplies two.
        assert!(matches!(
            decode(&hex("000103050102")),
            Err(Error::ShortPayload { at: 0, want: 5, have: 2 })
        ));
    }

    #[test]
    fn a_stray_trailing_byte_is_an_error() {
        assert!(matches!(decode(&hex("0001010000")), Err(Error::Truncated { at: 4 })));
    }
}
