//! Transports that answer from a script instead of a device.
//!
//! This is the reason [`Transport`] is a trait. Recorded
//! exchanges from four models exercise code paths nobody could otherwise reach
//! without owning all four; see the `fixtures` module.

use std::io::{self, ErrorKind};

use super::{Recording, Transport};

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

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let reply = self.replies.get(self.next).cloned().flatten();
        self.next += 1;
        match reply {
            // Hand over what fits and keep the rest, the way a stream socket
            // does. Copying the whole reply panics when it is longer than the
            // caller's buffer, which is how a real device answers `1f 01 05`.
            Some(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() {
                    self.replies.insert(self.next, Some(bytes[n..].to_vec()));
                }
                Ok(n)
            }
            None => Err(io::Error::new(ErrorKind::TimedOut, "scripted silence")),
        }
    }
}

impl Recording for Replay {
    fn sent(&self) -> &[Vec<u8>] {
        &self.sent
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
    /// The reply owed for the last request, drained as the caller reads. A
    /// device says a thing once; a double that repeats it forever turns any
    /// read-until-terminator loop into a spin.
    owed: Option<Vec<u8>>,
    log: Vec<Vec<u8>>,
}

impl Scripted {
    pub fn new() -> Self {
        Scripted {
            table: std::collections::HashMap::new(),
            last: Vec::new(),
            owed: None,
            log: Vec::new(),
        }
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
        self.owed = match self.table.get(data) {
            Some(reply) => reply.clone(),
            // Unscripted: refuse as a device does for a function it lacks.
            None if data.len() >= 2 => Some(vec![data[0], data[1], 0x04, 0x01, 0x03]),
            None => None,
        };
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(reply) = self.owed.take() else {
            return Err(io::Error::new(ErrorKind::TimedOut, "scripted silence"));
        };
        // Hand over what fits and keep the tail, the way a stream socket does.
        let n = reply.len().min(buf.len());
        buf[..n].copy_from_slice(&reply[..n]);
        if n < reply.len() {
            self.owed = Some(reply[n..].to_vec());
        }
        Ok(n)
    }
}

impl Recording for Scripted {
    fn sent(&self) -> &[Vec<u8>] {
        &self.log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripted_reply_is_given_once_not_forever() {
        // `enumerate` reads until a terminator arrives. A double that repeats
        // its reply on every read turns that into a spin — which is how this
        // was found, by a test that never returned.
        let mut t = Scripted::new().on(&[0x1f, 0x01, 0x05, 0x00], &[0x1f, 0x01, 0x04, 0x01, 0x03]);
        t.send(&[0x1f, 0x01, 0x05, 0x00]).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(t.recv(&mut buf).unwrap(), 5);
        assert_eq!(t.recv(&mut buf).unwrap_err().kind(), ErrorKind::TimedOut);
    }

    #[test]
    fn a_reply_longer_than_the_buffer_is_served_in_pieces_not_a_panic() {
        let long: Vec<u8> = (0..300).map(|i| i as u8).collect();
        let mut t = Replay::new(vec![Some(long.clone())]);
        let mut buf = [0u8; 128];
        assert_eq!(t.recv(&mut buf).unwrap(), 128);
        assert_eq!(t.recv(&mut buf).unwrap(), 128);
        assert_eq!(t.recv(&mut buf).unwrap(), 44);
    }
}
