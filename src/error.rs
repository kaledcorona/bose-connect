//! What can go wrong.
//!
//! Kept as its own type because this protocol answers three ways where most
//! answer two: a value, a refusal, or nothing at all. The reference is explicit
//! that a client must hold them apart — reading silence as "busy" invites a
//! retry loop against an opcode that will never answer, and reading it as
//! "unsupported" throws away the refusal codes, which are the only signal for
//! whether a function is worth probing further.

use std::fmt;
use std::io;

use crate::wire::{self, hex, Addr, Refusal};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Frame(wire::Error),
    /// The device refused, and said why.
    Refused { addr: Addr, why: Refusal },
    /// No reply and no refusal. Not a timeout to retry: the opcode does not
    /// answer, and the channel stays usable.
    Silent(Addr),
    /// This model does not carry it. Established before asking, from the
    /// device's own surface.
    Absent(Addr),
    /// The device answered in a shape the codec does not know.
    Malformed { addr: Addr, got: Vec<u8> },
    /// Real, but not understood well enough to use. The reason comes from the
    /// catalog entry, so it says what is missing rather than merely refusing.
    NotUnderstood { addr: Addr, why: &'static str },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Frame(e) => write!(f, "{e}"),
            Error::Refused { addr, why } => write!(f, "{addr}: {why}"),
            Error::Silent(addr) => write!(f, "{addr}: no answer, and no refusal"),
            Error::Absent(addr) => write!(f, "{addr}: this model does not have it"),
            Error::Malformed { addr, got } => {
                write!(f, "{addr}: unexpected payload [{}]", hex(got))
            }
            Error::NotUnderstood { addr, why } => write!(f, "{addr}: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<wire::Error> for Error {
    fn from(e: wire::Error) -> Self {
        Error::Frame(e)
    }
}

/// For callers still on `io::Result`, and for `main`.
///
/// The kind is what a shell-facing caller branches on: `Unsupported` for
/// anything this model or this crate cannot do, `InvalidData` for a device that
/// answered wrongly.
impl From<Error> for io::Error {
    fn from(e: Error) -> io::Error {
        use io::ErrorKind::*;
        match e {
            Error::Io(e) => e,
            Error::Frame(e) => io::Error::new(InvalidData, e.to_string()),
            ref e @ (Error::Absent(_) | Error::NotUnderstood { .. }) => {
                io::Error::new(Unsupported, e.to_string())
            }
            ref e @ Error::Refused { .. } => io::Error::new(InvalidInput, e.to_string()),
            ref e @ Error::Silent(_) => io::Error::new(TimedOut, e.to_string()),
            ref e @ Error::Malformed { .. } => io::Error::new(InvalidData, e.to_string()),
        }
    }
}
