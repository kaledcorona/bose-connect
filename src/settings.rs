//! Reading and writing the settings a device actually has.
//!
//! Every method here branches on [`Capabilities`], because the same setting
//! lives in different places across generations and a caller should not have to
//! know which.

use std::io;

use crate::device::{Anc, Device};
use crate::framing::Message;
use crate::transport::Transport;

/// A named noise-cancelling level, on models that offer a fixed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    High,
    Low,
}

impl Level {
    fn wire(self) -> u8 {
        match self {
            Level::Off => 0x00,
            Level::High => 0x01,
            Level::Low => 0x03,
        }
    }

    fn from_wire(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Level::Off),
            0x01 => Some(Level::High),
            0x03 => Some(Level::Low),
            _ => None,
        }
    }
}

/// The state of the noise-cancelling control.
///
/// The two generations do not share this field, so neither does this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncState {
    /// QuietComfort 35. Three named levels, and a bitmask saying which the
    /// model accepts — bit *n* set means level *n* is allowed. Writing anything
    /// else is refused with payload `06`, argument not accepted.
    Named { level: Option<Level>, accepted: u8 },
    /// Ultra family. `awareness` counts transparency, not cancellation: `0` is
    /// maximum cancelling and `values - 1` lets everything through.
    ///
    /// `values` is eleven on every model seen, `0x00`–`0x0a`. The app's live
    /// slider exposes ten of them; the eleventh, `0x0a`, is Aware mode and is
    /// also what a stored mode with cancellation `0` holds.
    Graded { awareness: u8, values: u8 },
}

impl AncState {
    /// Cancellation the way a person pictures it: `0` none, higher is more.
    /// `None` on models whose levels are named rather than graded.
    pub fn cancelling(&self) -> Option<u8> {
        match self {
            AncState::Graded { awareness, values } => {
                Some(values.saturating_sub(1).saturating_sub(*awareness))
            }
            AncState::Named { .. } => None,
        }
    }

    /// Whether a named level is offered, read from the device's own mask.
    pub fn accepts(&self, level: Level) -> bool {
        match self {
            AncState::Named { accepted, .. } => accepted & (1 << level.wire()) != 0,
            AncState::Graded { .. } => false,
        }
    }
}

/// One equaliser band, carrying its own limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub index: u8,
    pub min: i8,
    pub max: i8,
    pub value: i8,
}

/// Immersive audio, on the Ultra generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Immersive {
    Off,
    Still,
    Motion,
}

impl TryFrom<u8> for Immersive {
    type Error = u8;
    fn try_from(b: u8) -> Result<Self, u8> {
        match b {
            0x00 => Ok(Immersive::Off),
            0x01 => Ok(Immersive::Still),
            0x02 => Ok(Immersive::Motion),
            other => Err(other),
        }
    }
}

/// Raised when a caller asks for something this model does not have.
#[derive(Debug)]
pub struct Unsupported(pub &'static str);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The message is written where it is raised, so each one can be a
        // sentence that fits its case rather than a noun bolted onto a stem.
        f.write_str(self.0)
    }
}

impl std::error::Error for Unsupported {}

pub(crate) fn unsupported(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, Unsupported(what))
}

impl<T: Transport> Device<T> {
    pub fn noise_cancelling(&mut self) -> io::Result<Option<AncState>> {
        let flavour = self.capabilities.anc;
        let opcode = match flavour {
            Anc::Legacy => 0x06,
            Anc::Modern => 0x05,
            Anc::Absent => return Err(unsupported("this device has no noise cancelling")),
        };
        let Some(msgs) = self.session().request(&Message::get(0x01, opcode))? else {
            return Ok(None);
        };
        let Some(m) = msgs.into_iter().find(|m| m.refusal().is_none()) else {
            return Ok(None);
        };
        Ok(match (flavour, m.payload.as_slice()) {
            // `<level> <mask>`
            (Anc::Legacy, [value, mask, ..]) => Some(AncState::Named {
                level: Level::from_wire(*value),
                accepted: *mask,
            }),
            // `<values> <awareness> <unknown>`
            (Anc::Modern, [values, awareness, ..]) => Some(AncState::Graded {
                awareness: *awareness,
                values: *values,
            }),
            _ => None,
        })
    }

    /// Select a named level, on a model that offers them.
    ///
    /// The device's own mask decides what is allowed. Sending a value outside it
    /// is refused with payload `06` — writing every value `0x00`-`0x0b` to a
    /// QuietComfort 35 had eight of the twelve rejected — so this checks first
    /// and reports the model's answer rather than the wire's.
    pub fn set_level(&mut self, level: Level) -> io::Result<()> {
        match self.capabilities.anc {
            Anc::Legacy => {
                match self.noise_cancelling()? {
                    Some(state) if !state.accepts(level) => {
                        return Err(unsupported("this model does not accept that level"))
                    }
                    _ => {}
                }
                self.session()
                    .request(&Message::set(0x01, 0x06, vec![level.wire()]))?;
                Ok(())
            }
            Anc::Modern => Err(unsupported("this model has a graded scale, not named levels")),
            Anc::Absent => Err(unsupported("this device has no noise cancelling")),
        }
    }

    /// Set cancellation on a graded model.
    ///
    /// Refused, deliberately. No capture of the official app writing `01 05`
    /// exists, so the format would be a guess, and a blind write to an
    /// unidentified opcode is the one move this protocol does not forgive.
    /// Select a mode instead: those carry a cancellation level and their write
    /// format is confirmed.
    pub fn set_cancelling(&mut self, _level: u8) -> io::Result<()> {
        match self.capabilities.anc {
            Anc::Modern => Err(unsupported("no confirmed write format for 01 05 — select a mode instead")),
            Anc::Legacy => Err(unsupported("this model has named levels, not a graded scale")),
            Anc::Absent => Err(unsupported("this device has no noise cancelling")),
        }
    }

    /// The equaliser, each band carrying the range the device reports.
    pub fn equaliser(&mut self) -> io::Result<Vec<Band>> {
        if !self.capabilities.equaliser {
            return Err(unsupported("this device has no equaliser"));
        }
        let Some(msgs) = self.session().request(&Message::get(0x01, 0x07))? else {
            return Ok(Vec::new());
        };
        let Some(m) = msgs.into_iter().find(|m| m.refusal().is_none()) else {
            return Ok(Vec::new());
        };
        Ok(m.payload
            .chunks_exact(4)
            .map(|c| Band {
                min: c[0] as i8,
                max: c[1] as i8,
                value: c[2] as i8,
                index: c[3],
            })
            .collect())
    }

    /// Set one band. The value is clamped to the range the device reported.
    pub fn set_band(&mut self, index: u8, value: i8) -> io::Result<()> {
        let band = self
            .equaliser()?
            .into_iter()
            .find(|b| b.index == index)
            .ok_or_else(|| unsupported("no such equaliser band"))?;
        let v = value.clamp(band.min, band.max);
        self.session()
            .request(&Message::set(0x01, 0x07, vec![v as u8, index]))?;
        Ok(())
    }

    pub fn immersive(&mut self) -> io::Result<Option<Immersive>> {
        if !self.capabilities.immersive {
            return Err(unsupported("this device has no immersive audio"));
        }
        let Some(msgs) = self.session().request(&Message::get(0x05, 0x0f))? else {
            return Ok(None);
        };
        Ok(msgs
            .into_iter()
            .find(|m| m.refusal().is_none())
            .and_then(|m| m.payload.first().copied())
            .and_then(|b| Immersive::try_from(b).ok()))
    }

    /// Select a stored mode by index.
    ///
    /// Refused. The form the official app sends — `1f 03 05 02 00 <index>` — is
    /// accepted by the device and **changes nothing**: reading `1f 03` back
    /// returns the previous mode, for every index. Operator `02` is refused
    /// outright.
    ///
    /// The capture established the syntax and was taken as establishing the
    /// semantics, which it does not. A field counts as writable when a write
    /// changes what a read returns, and this one does not.
    pub fn select_mode(&mut self, _index: u8) -> io::Result<()> {
        if !self.capabilities.modes {
            return Err(unsupported("this device has no modes"));
        }
        Err(unsupported(
            "selecting a mode over this protocol is not understood; use the earcup button",
        ))
    }

    /// The index of the mode currently active. Reading works.
    pub fn current_mode(&mut self) -> io::Result<Option<u8>> {
        if !self.capabilities.modes {
            return Err(unsupported("this device has no modes"));
        }
        let Some(msgs) = self.session().request(&Message::get(0x1f, 0x03))? else {
            return Ok(None);
        };
        Ok(msgs
            .into_iter()
            .find(|m| m.refusal().is_none())
            .and_then(|m| m.payload.first().copied()))
    }

    /// The stored modes, as `(index, name)`. Empty slots are skipped.
    pub fn modes(&mut self) -> io::Result<Vec<(u8, String)>> {
        if !self.capabilities.modes {
            return Err(unsupported("this device has no modes"));
        }
        let msgs = self.session().enumerate(0x1f)?;
        Ok(msgs
            .into_iter()
            .filter(|m| m.opcode == 0x06 && m.payload.len() >= 47)
            // Byte 5 says whether the slot holds a mode. Byte 46 is wind
            // block, which is 0 on any mode that does not use it — reading
            // that as occupancy silently drops real modes.
            .filter(|m| m.payload[5] == 0x01)
            .map(|m| {
                let name = m.payload[6..]
                    .split(|&b| b == 0)
                    .next()
                    .unwrap_or_default();
                (m.payload[0], String::from_utf8_lossy(name).to_string())
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::transport::{Scripted, Session};

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn qc35() -> Device<Scripted> {
        let mut s = Scripted::new()
            .on(&hex("00010100"), &hex("00010305312e302e34"))
            .on(&hex("00030100"), &hex("00030303400c02"))
            .on(&hex("01060100"), &hex("01060302010b"))
            .on(&hex("0106020103"), &hex("01060302030b"))
            .silent(&hex("01070100"));
        for f in [0x00u8, 0x01, 0x02, 0x05] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x01, 0x00]);
        }
        Device::open(Session::open(s).unwrap()).unwrap()
    }

    fn ultra() -> Device<Scripted> {
        let mut s = Scripted::new()
            .on(&hex("00010100"), &hex("00010305312e322e30"))
            .on(&hex("00030100"), &hex("00030303406601"))
            .on(&hex("01060100"), &hex("0106040104"))
            .on(&hex("01050100"), &hex("010503030b0a03"))
            .on(&hex("01070100"), &hex("0107030cf60a0200f60a0101f60a0202"))
            .on(&hex("050f0100"), &hex("050f030102"));
        for f in [0x00u8, 0x01, 0x02, 0x05, 0x1f] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x01, 0x00]);
        }
        Device::open(Session::open(s).unwrap()).unwrap()
    }

    #[test]
    fn reads_noise_cancelling_from_whichever_opcode_the_model_uses() {
        assert!(matches!(
            qc35().noise_cancelling().unwrap().unwrap(),
            AncState::Named { level: Some(Level::High), accepted: 0x0b }
        ));
        assert!(matches!(
            ultra().noise_cancelling().unwrap().unwrap(),
            AncState::Graded { awareness: 0x0a, values: 0x0b }
        ));
    }

    #[test]
    fn the_graded_scale_is_inverted_and_the_api_un_inverts_it() {
        // Awareness 0x0a of eleven values is no cancelling at all.
        assert_eq!(ultra().noise_cancelling().unwrap().unwrap().cancelling(), Some(0));
        // Named models have levels, not a scale, so nothing is invented.
        assert_eq!(qc35().noise_cancelling().unwrap().unwrap().cancelling(), None);
    }

    #[test]
    fn the_mask_says_which_levels_a_named_model_accepts() {
        // 0x0b is 0b1011: levels 0, 1 and 3 — off, high and low. Writing
        // anything else to a QC35 is refused with payload 06.
        let a = qc35().noise_cancelling().unwrap().unwrap();
        assert!(a.accepts(Level::Off));
        assert!(a.accepts(Level::High));
        assert!(a.accepts(Level::Low));
    }

    #[test]
    fn writing_is_refused_where_the_format_is_unknown() {
        assert!(qc35().set_level(Level::Low).is_ok());
        // Graded models take no named level, and their write format is unknown.
        assert_eq!(ultra().set_level(Level::Low).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(ultra().set_cancelling(5).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn the_equaliser_reports_each_bands_own_range() {
        let bands = ultra().equaliser().unwrap();
        assert_eq!(bands.len(), 3);
        assert_eq!((bands[0].min, bands[0].max), (-10, 10));
        assert_eq!(bands[0].value, 2);
        assert_eq!(bands[2].index, 2);
    }

    #[test]
    fn asking_a_qc35_for_an_equaliser_is_an_error_not_an_empty_list() {
        // Silence on `01 07` means the control does not exist. Returning an
        // empty list would invite a client to draw one with no bands.
        assert_eq!(qc35().equaliser().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn reads_immersive_audio() {
        assert_eq!(ultra().immersive().unwrap(), Some(Immersive::Motion));
        assert_eq!(qc35().immersive().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }

    /// Build one 47-byte mode record the way a device sends it.
    fn mode_record(index: u8, occupied: bool, name: &str, awareness: u8, wind: u8) -> Vec<u8> {
        let mut r = vec![0u8; 47];
        r[0] = index;
        r[5] = if occupied { 0x01 } else { 0x00 };
        r[6..6 + name.len()].copy_from_slice(name.as_bytes());
        r[42] = awareness;
        r[46] = wind;
        r
    }

    #[test]
    fn a_stored_mode_with_wind_block_off_is_still_a_stored_mode() {
        // Byte 46 is wind block, not occupancy. Every mode that does not use it
        // carries 0 there, so filtering on it drops real modes — which is what
        // this did until a check against the reference caught it.
        let mut reply = vec![0x1f, 0x01, 0x07, 0x00];
        for r in [
            mode_record(0, true, "Quiet", 0x00, 0x00),
            mode_record(3, true, "Focus", 0x05, 0x00),
            mode_record(5, false, "None", 0x00, 0x00),
        ] {
            reply.extend_from_slice(&[0x1f, 0x06, 0x03, 47]);
            reply.extend_from_slice(&r);
        }
        reply.extend_from_slice(&[0x1f, 0x01, 0x06, 0x00]);

        let mut s = Scripted::new()
            .on(&hex("00010100"), &hex("00010305312e322e30"))
            .on(&hex("00030100"), &hex("00030303406601"))
            .on(&hex("01060100"), &hex("0106040104"))
            .on(&hex("01050100"), &hex("010503030b0a03"))
            .on(&hex("1f010500"), &reply);
        for f in [0x00u8, 0x01, 0x1f] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x01, 0x00]);
        }
        let modes = Device::open(Session::open(s).unwrap()).unwrap().modes().unwrap();
        assert_eq!(
            modes,
            vec![(0, "Quiet".to_string()), (3, "Focus".to_string())]
        );
    }

    #[test]
    fn selecting_a_mode_is_refused_because_it_does_not_work() {
        // The app's form is accepted by the device and changes nothing, for
        // every index. A control that silently does nothing is worse than none.
        assert_eq!(ultra().select_mode(1).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
