//! A client for the RFCOMM control protocol used by Bose headphones.
//!
//! The protocol description this implements lives at
//! <https://github.com/kaledcorona/bose-rfcomm>. Where the two disagree, the
//! description is the one backed by observation.

pub mod fields;
pub mod framing;
pub mod device;
pub mod settings;
pub mod transport;
