//! Identity, and what a given device can actually do.
//!
//! The protocol is shared across generations; the vocabulary is not. Noise
//! cancelling lives at `01 06` on a QuietComfort 35 and at `01 05` on the Ultra
//! family, which refuses the older opcode. Four of nine fields found on an Ultra
//! do not exist on a 2016 device at all.
//!
//! Every implementation of this protocol so far has answered that with a table
//! of device ids maintained by hand, and been wrong about every model its author
//! did not own. This module asks the device instead.

use std::collections::BTreeSet;
use std::io;

use crate::framing::{Message, Refusal};
use crate::transport::{Session, Transport};

/// Where a model keeps its noise-cancelling control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anc {
    /// `01 06`, payload `<level> <positions>`. QuietComfort 35.
    Legacy,
    /// `01 05`, payload `<positions> <awareness> <unknown>`. Ultra family.
    ///
    /// The value counts **awareness**, not cancellation: `0` is maximum
    /// cancelling and `10` lets everything through.
    Modern,
    /// The Sport Earbuds have none, and say so.
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Functions that exist, whether or not they enumerate.
    pub functions: BTreeSet<u8>,
    pub anc: Anc,
    pub equaliser: bool,
    pub immersive: bool,
    pub modes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `0x400c`, `0x402D`, `0x4064`, `0x4066` so far. Equal to the product id
    /// in bluez's `Modalias`, so it can also be read without connecting.
    pub id: u16,
    pub index: u8,
    /// `00 05`. **Not always the firmware version**: a QuietComfort 35 returns
    /// `3.0.3`, which is its firmware, while an Ultra returns a build string
    /// like `1.6.7+g6ebabd2`, which is not. Present it as a version string and
    /// let the user decide what it means.
    pub version: Option<String>,
    pub serial: Option<String>,
    /// `00 0f`. Only the Ultra generation answers; older models refuse, so a
    /// name table is still needed for them.
    pub model: Option<String>,
}

pub struct Device<T: Transport> {
    session: Session<T>,
    pub identity: Identity,
    pub capabilities: Capabilities,
}

impl<T: Transport> Device<T> {
    /// Interrogate a device over an already-open session.
    pub fn open(mut session: Session<T>) -> io::Result<Self> {
        let identity = Self::read_identity(&mut session)?;
        let capabilities = Self::probe_capabilities(&mut session)?;
        Ok(Device { session, identity, capabilities })
    }

    fn text(session: &mut Session<T>, function: u8, opcode: u8) -> io::Result<Option<String>> {
        Ok(Self::payload(session, function, opcode)?
            .map(|p| String::from_utf8_lossy(&p).trim_matches('\0').to_string()))
    }

    /// The payload of a successful read, or `None` for a refusal or silence.
    fn payload(session: &mut Session<T>, function: u8, opcode: u8) -> io::Result<Option<Vec<u8>>> {
        match session.request(&Message::get(function, opcode))? {
            Some(msgs) => Ok(msgs
                .into_iter()
                .find(|m| m.function == function && m.opcode == opcode && m.refusal().is_none())
                .map(|m| m.payload)),
            None => Ok(None),
        }
    }

    fn read_identity(session: &mut Session<T>) -> io::Result<Identity> {
        let (id, index) = match Self::payload(session, 0x00, 0x03)? {
            Some(p) if p.len() >= 3 => (u16::from(p[0]) << 8 | u16::from(p[1]), p[2]),
            _ => (0, 0),
        };
        Ok(Identity {
            id,
            index,
            version: Self::text(session, 0x00, 0x05)?,
            serial: Self::text(session, 0x00, 0x07)?,
            model: Self::text(session, 0x00, 0x0f)?,
        })
    }

    /// Ask which functions exist, then which of the interesting opcodes answer.
    ///
    /// Opcode `00` is every function's version and is never listed by an
    /// enumeration, but it always answers — which makes it the cheapest probe
    /// for whether a function is there at all.
    fn probe_capabilities(session: &mut Session<T>) -> io::Result<Capabilities> {
        let mut functions = BTreeSet::new();
        for f in 0x00u8..=0x3f {
            if Self::function_exists(session, f)? {
                functions.insert(f);
            }
        }
        let anc = if Self::answers(session, 0x01, 0x06)? {
            Anc::Legacy
        } else if Self::answers(session, 0x01, 0x05)? {
            Anc::Modern
        } else {
            Anc::Absent
        };
        Ok(Capabilities {
            equaliser: Self::answers(session, 0x01, 0x07)?,
            immersive: Self::answers(session, 0x05, 0x0f)?,
            modes: functions.contains(&0x1f),
            functions,
            anc,
        })
    }

    fn function_exists(session: &mut Session<T>, function: u8) -> io::Result<bool> {
        match session.request(&Message::get(function, 0x00))? {
            // Silence is not absence. It is its own case, seen on a QuietComfort
            // 35 for a contiguous block of opcodes, and treating it as "missing"
            // throws away the only signal that says whether to keep looking.
            None => Ok(false),
            Some(msgs) => Ok(!msgs
                .iter()
                .any(|m| m.refusal() == Some(Refusal::FunctionAbsent))),
        }
    }

    fn answers(session: &mut Session<T>, function: u8, opcode: u8) -> io::Result<bool> {
        Ok(Self::payload(session, function, opcode)?.is_some())
    }

    pub fn session(&mut self) -> &mut Session<T> {
        &mut self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Scripted;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// A QuietComfort 35, as observed. Functions 00 01 02 03 04 05 08 09;
    /// noise cancelling at `01 06`; no equaliser, no immersive, no modes.
    fn qc35() -> Scripted {
        let mut s = Scripted::new()
            .on(&hex("00010100"), &hex("00010305312e302e34"))
            .on(&hex("00030100"), &hex("00030303400c02"))
            .on(&hex("00050100"), &hex("00050305332e302e33"))
            .on(&hex("00070100"), &hex("0007030430303031"))
            .on(&hex("01060100"), &hex("01060302010b"))
            // The equaliser opcode does not refuse here — it says nothing.
            .silent(&hex("01070100"));
        for f in [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x08, 0x09] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x05, 0x31, 0x2e, 0x30, 0x2e, 0x31]);
        }
        s
    }

    /// A QuietComfort Ultra Headphones. Noise cancelling moved to `01 05`, the
    /// old opcode refuses, and function 0x1f carries the modes.
    fn ultra() -> Scripted {
        let mut s = Scripted::new()
            .on(&hex("00010100"), &hex("00010305312e322e30"))
            .on(&hex("00030100"), &hex("00030303406601"))
            .on(&hex("00050100"), &hex("0005030e312e362e372b6736656261626432"))
            .on(&hex("000f0100"), &hex("000f0318426f736520514320556c747261204865616470686f6e6573"))
            .on(&hex("01060100"), &hex("0106040104"))
            .on(&hex("01050100"), &hex("010503030b0a03"))
            .on(&hex("01070100"), &hex("0107030cf60a0200f60a0101f60a0202"))
            .on(&hex("050f0100"), &hex("050f030100"));
        for f in [0x00u8, 0x01, 0x02, 0x05, 0x06, 0x07, 0x1f] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x05, 0x31, 0x2e, 0x30, 0x2e, 0x30]);
        }
        s
    }

    fn open(t: Scripted) -> Device<Scripted> {
        Device::open(Session::open(t).unwrap()).unwrap()
    }

    #[test]
    fn identifies_a_qc35() {
        let d = open(qc35());
        assert_eq!(d.identity.id, 0x400c);
        assert_eq!(d.identity.version.as_deref(), Some("3.0.3"));
        // The model name opcode is Ultra-only; older devices refuse it, so a
        // client still needs a table for them.
        assert_eq!(d.identity.model, None);
    }

    #[test]
    fn identifies_an_ultra_by_asking_rather_than_by_table() {
        let d = open(ultra());
        assert_eq!(d.identity.id, 0x4066);
        assert_eq!(d.identity.model.as_deref(), Some("Bose QC Ultra Headphones"));
    }

    #[test]
    fn finds_noise_cancelling_where_each_generation_keeps_it() {
        assert_eq!(open(qc35()).capabilities.anc, Anc::Legacy);
        assert_eq!(open(ultra()).capabilities.anc, Anc::Modern);
    }

    #[test]
    fn silence_on_the_equaliser_reads_as_absent_not_present() {
        // A QC35 neither answers nor refuses `01 07`. Either misreading is a bug:
        // "present" offers a control that will never respond.
        assert!(!open(qc35()).capabilities.equaliser);
        assert!(open(ultra()).capabilities.equaliser);
    }

    #[test]
    fn features_that_arrived_with_the_ultra_are_not_claimed_for_older_models() {
        let old = open(qc35()).capabilities;
        assert!(!old.immersive);
        assert!(!old.modes);
        let new = open(ultra()).capabilities;
        assert!(new.immersive);
        assert!(new.modes);
    }

    #[test]
    fn reports_the_functions_a_model_actually_has() {
        let f = open(qc35()).capabilities.functions;
        assert!(f.contains(&0x08));   // exists on the QC35
        assert!(!f.contains(&0x1f));  // and not the modes function
    }
}
