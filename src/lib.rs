//! A client for the RFCOMM control protocol used by Bose headphones.
//!
//! The protocol description this implements lives at
//! <https://github.com/kaledcorona/bose-rfcomm>. Where the two disagree, the
//! description is the one backed by observation.
//!
//! Layers run one way, each knowing only the one below it:
//!
//! ```text
//! transport   bytes
//! session     bytes → records, one exchange
//! wire        records ↔ payloads
//! codec       payloads ↔ values
//! catalog     which value lives where, and how well we know it
//! surface     which of those this device answers
//! device      get / set — the only two verbs
//! api         names for the verbs
//! ```

pub mod wire;
pub mod error;
pub mod session;
pub mod codec;
pub mod catalog;
pub mod surface;
pub mod device;
pub mod api;
pub mod transport;

#[cfg(any(test, feature = "mock"))]
pub mod fixtures;
