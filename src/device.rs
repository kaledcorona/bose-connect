//! The two verbs.
//!
//! `get` and `set` carry every access in the crate. What differs between
//! records lives in [`crate::catalog`]; what differs between models lives in
//! [`crate::surface`]. Neither is a match arm here.

use crate::catalog::{self, Field};
use crate::error::{Error, Result};
use crate::session::Session;
use crate::surface::{Known, Surface};
use crate::transport::Transport;
use crate::wire::{Addr, Refusal};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identity {
    /// `0x400c`, `0x402D`, `0x4064`, `0x4066` so far. Equal to the product id
    /// in bluez's `Modalias`, so it can also be read without connecting.
    pub id: u16,
    pub index: u8,
    /// `00 05`. **Not always the firmware version**: a QuietComfort 35 returns
    /// `3.0.3`, which is its firmware, while an Ultra returns a build string
    /// like `1.6.7+g6ebabd2`, which is not. Present it as a version string and
    /// let the reader decide what it means.
    pub version: Option<String>,
    pub serial: Option<String>,
    /// `00 0f`. Only the Ultra generation answers; older models refuse, so a
    /// name table is still needed for them.
    pub model: Option<String>,
}

pub struct Device<T: Transport> {
    session: Session<T>,
    pub identity: Identity,
    pub surface: Surface,
}

impl<T: Transport> Device<T> {
    /// Interrogate a device over an already-open session.
    pub fn open(mut session: Session<T>) -> Result<Self> {
        let surface = Surface::discover(&mut session)?;
        let mut dev = Device { session, identity: Identity::default(), surface };
        dev.identity = dev.read_identity();
        Ok(dev)
    }

    /// Read one record and decode it.
    pub fn get<R, W>(&mut self, f: &Field<R, W>) -> Result<R> {
        let addr = f.meta.addr;
        if self.surface.state(addr) == Known::Absent {
            return Err(Error::Absent(addr));
        }
        match self.session.read(addr) {
            Ok(bytes) => {
                self.surface.settle(addr, true);
                (f.decode)(&bytes).ok_or(Error::Malformed { addr, got: bytes })
            }
            Err(e) => {
                if structural(&e) {
                    self.surface.settle(addr, false);
                }
                Err(e)
            }
        }
    }

    /// Write one record, if the format is confirmed.
    ///
    /// The gate is the catalog's, not this function's: a format seen on the
    /// wire but never shown to change anything carries its own refusal text and
    /// cannot be sent. That is the reference's own lesson — **a capture
    /// establishes the syntax, not the semantics** — held by the data rather
    /// than by a match arm somebody has to remember to write.
    pub fn set<R, W>(&mut self, f: &Field<R, W>, value: W) -> Result<()> {
        let addr = f.meta.addr;
        let Some(encode) = f.encode.filter(|_| f.meta.write.usable()) else {
            return Err(Error::NotUnderstood { addr, why: f.meta.write.why() });
        };
        if self.surface.state(addr) == Known::Absent {
            return Err(Error::Absent(addr));
        }
        self.session.write(addr, encode(value))
    }

    /// Whether the device is known to hold this record, without asking it.
    ///
    /// `false` for anything unproven, so a caller needing certainty reads
    /// instead of guessing.
    pub fn has<R, W>(&self, f: &Field<R, W>) -> bool {
        self.surface.state(f.meta.addr) == Known::Live
    }

    fn read_identity(&mut self) -> Identity {
        let (id, index) = self.get(&catalog::DEVICE_ID).map_or((0, 0), |d| (d.id, d.index));
        Identity {
            id,
            index,
            version: self.get(&catalog::VERSION).ok(),
            serial: self.get(&catalog::SERIAL).ok(),
            model: self.get(&catalog::MODEL).ok(),
        }
    }

    /// Read an address the catalog has no entry for.
    ///
    /// The exploration path. Everything the catalog knows started here.
    pub fn raw(&mut self, addr: Addr) -> Result<Vec<u8>> {
        self.session.read(addr)
    }

    /// Every function from `first` to `last` that answers, for mapping a model
    /// this crate does not know.
    ///
    /// Slow by nature — a round trip each, and a silent function costs the whole
    /// receive timeout. Every sweep in the reference stopped at `0x0f`, which is
    /// why the mode table at `0x1f` went unfound through twenty-nine labelled
    /// observations. There is no boundary in the protocol; pick one deliberately.
    pub fn scan(&mut self, first: u8, last: u8) -> Result<Vec<u8>> {
        (first..=last)
            .filter_map(|f| match self.session.read(Addr::at(f, 0x00)) {
                Ok(_) => Some(Ok(f)),
                // `03` says the function is missing. `04` says the opcode is,
                // which means the function is real and worth probing further.
                Err(Error::Refused { why: Refusal::FunctionAbsent, .. } | Error::Silent(_)) => None,
                Err(Error::Refused { .. }) => Some(Ok(f)),
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    pub fn session(&mut self) -> &mut Session<T> {
        &mut self.session
    }
}

/// Whether a failure says something permanent about the device.
///
/// A transient one must not be cached. `01 05` answers operator `04` around
/// immersive-audio transitions and settles afterwards; remembering that would
/// report an Ultra as having no noise cancelling for the rest of the session.
fn structural(e: &Error) -> bool {
    matches!(
        e,
        Error::Refused { why: Refusal::FunctionAbsent | Refusal::OpcodeAbsent, .. }
            | Error::Silent(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::*;
    use crate::fixtures;
    use crate::transport::Scripted;

    fn open(t: Scripted) -> Device<Scripted> {
        Device::open(Session::open(t).unwrap()).unwrap()
    }

    #[test]
    fn identifies_a_qc35() {
        let d = open(fixtures::qc35());
        assert_eq!(d.identity.id, 0x400c);
        assert_eq!(d.identity.version.as_deref(), Some("3.0.3"));
        // The model name opcode is Ultra-only; older devices refuse it, so a
        // client still needs a table for them.
        assert_eq!(d.identity.model, None);
    }

    #[test]
    fn identifies_an_ultra_by_asking_rather_than_by_table() {
        let d = open(fixtures::ultra_hp());
        assert_eq!(d.identity.id, 0x4066);
        assert_eq!(d.identity.model.as_deref(), Some("Bose QC Ultra Headphones"));
        // The same opcode carries a different kind of version per generation.
        assert_eq!(d.identity.version.as_deref(), Some("1.6.7+g6ebabd2"));
    }

    #[test]
    fn an_absent_record_costs_no_round_trip() {
        // A QC35's function 01 enumerates and does not mention 07, so there is
        // no equaliser — established without asking, where a direct probe would
        // have waited out the whole receive timeout on a silent opcode.
        let mut d = open(fixtures::qc35());
        let before = d.session().transport_sent().len();
        assert!(matches!(d.get(&EQUALISER), Err(Error::Absent(_))));
        assert_eq!(d.session().transport_sent().len(), before);
    }

    #[test]
    fn a_function_that_refuses_to_list_is_probed_rather_than_written_off() {
        // Function 05 refuses the sweep on both generations and holds the
        // volume on one and immersive audio on the other.
        let mut d = open(fixtures::qc35());
        assert!(d.get(&VOLUME).is_ok());
    }

    #[test]
    fn silence_and_refusal_are_different_answers() {
        let mut d = open(fixtures::qc35());
        // `05 0f` refuses: the opcode is not on this model.
        assert!(matches!(d.get(&IMMERSIVE), Err(Error::Refused { .. })));
        // `01 07` says nothing at all, and the channel stays usable.
        let mut d = open(fixtures::qc35());
        d.surface.settle(Addr::at(0x01, 0x07), true); // bypass the listing
        assert!(matches!(d.get(&EQUALISER), Err(Error::Silent(_))));
        assert!(d.get(&ANC_NAMED).is_ok());
    }

    #[test]
    fn a_transient_refusal_is_not_remembered() {
        // `01 05` answers operator 04 around immersive-audio transitions and
        // settles afterwards. Caching that reports an Ultra as having no noise
        // cancelling for the rest of the session.
        let mut d = open(fixtures::ultra_hp());
        let addr = ANC_GRADED.meta.addr;
        d.surface.settle(addr, false);
        assert!(structural(&Error::Refused { addr, why: Refusal::OpcodeAbsent }));
        assert!(!structural(&Error::Refused { addr, why: Refusal::Other(0x08) }));
    }

    #[test]
    fn a_write_with_no_confirmed_format_is_refused_by_the_catalog() {
        // Not by a match arm here. `1f 03` is accepted by the device and
        // changes nothing, and the reason travels with the record.
        let mut d = open(fixtures::ultra_hp());
        let Err(Error::NotUnderstood { why, .. }) = d.set(&CURRENT_MODE, 1) else {
            panic!("a write with no confirmed format was sent");
        };
        assert!(why.contains("changes nothing"));
        // And `01 05` was never captured at all.
        assert!(matches!(d.set(&ANC_GRADED, 5), Err(Error::NotUnderstood { .. })));
    }

    #[test]
    fn reports_the_functions_a_model_actually_has() {
        let d = open(fixtures::qc35());
        let f: Vec<u8> = d.surface.functions().collect();
        assert!(f.contains(&0x04));
        assert!(!f.contains(&0x1f)); // no modes on a 2016 device
    }
}
