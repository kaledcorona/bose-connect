//! Getting bytes to and from a device.
//!
//! Everything above this is written against [`Transport`], so the rest of the
//! crate can be tested by replaying recorded traffic — which matters here,
//! because exercising the whole protocol needs four headphones from two
//! generations and most people have one.

use std::io;

pub mod mock;
pub mod probe;
pub mod rfcomm;

pub use mock::{Replay, Scripted};
pub use probe::probe_channel;
pub use rfcomm::{Address, Rfcomm};


/// A byte channel to a device.
pub trait Transport {
    fn send(&mut self, data: &[u8]) -> io::Result<()>;
    /// Returns however many bytes arrived. A timeout is `ErrorKind::TimedOut`,
    /// which callers must distinguish from a refusal: the device answering
    /// nothing at all is a third case, not a slow refusal.
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// A transport that keeps what was sent, so a test can assert on the wire.
///
/// Separate from [`Transport`] because a socket has nothing to record, and a
/// default returning an empty slice puts test scaffolding in the trait every
/// real implementation has to carry.
pub trait Recording: Transport {
    fn sent(&self) -> &[Vec<u8>];
}
