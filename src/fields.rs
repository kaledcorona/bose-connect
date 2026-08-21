//! The rest of the settings.
//!
//! One method per record in the reference. Where a field's meaning differs by
//! generation the type says so; where a value is documented but not understood,
//! it is passed through rather than dressed up.

use std::io;

use crate::device::Device;
use crate::framing::Message;
use crate::settings::unsupported;
use crate::transport::Transport;

/// A setting that is simply on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// `01 0a` — two devices connected at once.
    Multipoint,
    /// `01 18` — pause when the headphones come off.
    HeadDetection,
    /// `01 1b` — answer a call by putting them on.
    AutoAnswer,
    /// `1f 05` — return to the last mode on power-up. Lives with the modes, not
    /// with the other toggles.
    RememberMode,
}

impl Toggle {
    fn address(self) -> (u8, u8) {
        match self {
            Toggle::Multipoint => (0x01, 0x0a),
            Toggle::HeadDetection => (0x01, 0x18),
            Toggle::AutoAnswer => (0x01, 0x1b),
            Toggle::RememberMode => (0x1f, 0x05),
        }
    }
}

/// Voice-prompt language. Only two values have been seen on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Spanish,
    Other(u8),
}

impl Language {
    fn wire(self) -> u8 {
        match self {
            Language::English => 0x21,
            Language::Spanish => 0x26,
            Language::Other(b) => b,
        }
    }
    fn from_wire(b: u8) -> Self {
        match b {
            0x21 => Language::English,
            0x26 => Language::Spanish,
            other => Language::Other(other),
        }
    }
}

/// `01 03`, which carries two settings in one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompts {
    pub language: Language,
    /// Whether the device announces its charge on power-up.
    ///
    /// This was first read as the voice-prompt switch, from a capture where the
    /// assignment was positional. A later capture toggled it four times under a
    /// category recorded unambiguously as the battery announcement. Whether the
    /// prompts for calls and connections share this record is **not known**.
    pub battery_announcement: bool,
}

/// One battery cell.
///
/// A QuietComfort 35 reports a bare percentage; the Ultra family reports a
/// four-byte record per cell, so earbuds give two and a headband gives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub index: u8,
    pub percent: u8,
}

/// `05 05`. The device states the size of its own scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub current: u8,
    /// Positions on the scale; `0x19` on every model seen, a range of `0`–`0x18`.
    pub steps: u8,
}

impl<T: Transport> Device<T> {
    fn read(&mut self, function: u8, opcode: u8) -> io::Result<Option<Vec<u8>>> {
        let Some(msgs) = self.session().request(&Message::get(function, opcode))? else {
            return Ok(None);
        };
        Ok(msgs
            .into_iter()
            .find(|m| m.function == function && m.opcode == opcode && m.refusal().is_none())
            .map(|m| m.payload))
    }

    fn write(&mut self, function: u8, opcode: u8, payload: Vec<u8>) -> io::Result<()> {
        self.session().request(&Message::set(function, opcode, payload))?;
        Ok(())
    }

    pub fn toggle(&mut self, t: Toggle) -> io::Result<Option<bool>> {
        let (f, o) = t.address();
        Ok(self.read(f, o)?.and_then(|p| p.first().map(|&b| b != 0)))
    }

    pub fn set_toggle(&mut self, t: Toggle, on: bool) -> io::Result<()> {
        let (f, o) = t.address();
        self.write(f, o, vec![u8::from(on)])
    }

    /// `01 02`. The read carries a leading `00`; the write does not.
    pub fn name(&mut self) -> io::Result<Option<String>> {
        Ok(self.read(0x01, 0x02)?.map(|p| {
            let text = if p.first() == Some(&0) { &p[1..] } else { &p[..] };
            String::from_utf8_lossy(text).trim_matches('\0').to_string()
        }))
    }

    /// Rename the device. The name lives in its firmware and shows up on every
    /// other host that pairs with it, so this is not a local alias.
    pub fn set_name(&mut self, name: &str) -> io::Result<()> {
        self.write(0x01, 0x02, name.as_bytes().to_vec())
    }

    pub fn prompts(&mut self) -> io::Result<Option<Prompts>> {
        Ok(self.read(0x01, 0x03)?.and_then(|p| match p.as_slice() {
            [lang, ..] => Some(Prompts {
                language: Language::from_wire(*lang),
                // The read packs flags into the language byte; bit 0x20 is set
                // when announcements are on. The write uses a separate byte.
                battery_announcement: lang & 0x20 != 0,
            }),
            _ => None,
        }))
    }

    pub fn set_prompts(&mut self, p: Prompts) -> io::Result<()> {
        self.write(0x01, 0x03, vec![p.language.wire(), u8::from(p.battery_announcement)])
    }

    /// Minutes of inactivity before powering down. `None` means never.
    pub fn auto_off(&mut self) -> io::Result<Option<Option<u8>>> {
        Ok(self
            .read(0x01, 0x04)?
            .and_then(|p| p.first().copied())
            .map(|m| if m == 0 { None } else { Some(m) }))
    }

    pub fn set_auto_off(&mut self, minutes: Option<u8>) -> io::Result<()> {
        self.write(0x01, 0x04, vec![minutes.unwrap_or(0)])
    }

    /// `01 0b`. Level `0` is off; `1` and `3` were the other values seen.
    pub fn self_voice(&mut self) -> io::Result<Option<u8>> {
        Ok(self.read(0x01, 0x0b)?.and_then(|p| p.get(1).copied()))
    }

    pub fn set_self_voice(&mut self, level: u8) -> io::Result<()> {
        self.write(0x01, 0x0b, vec![0x01, level])
    }

    /// `01 09`. Four action values were captured — `0e`, `03`, `13`, `01` — but
    /// the capture changed the enabled state and the selection together, so
    /// which is which was never established. Passed through raw.
    pub fn shortcut(&mut self) -> io::Result<Option<u8>> {
        Ok(self.read(0x01, 0x09)?.and_then(|p| p.get(2).copied()))
    }

    pub fn set_shortcut(&mut self, action: u8) -> io::Result<()> {
        self.write(0x01, 0x09, vec![0x80, 0x09, action])
    }

    pub fn battery(&mut self) -> io::Result<Vec<Cell>> {
        let Some(p) = self.read(0x02, 0x02)? else {
            return Ok(Vec::new());
        };
        Ok(match p.len() {
            // A QuietComfort 35 answers with the percentage and nothing else.
            1 => vec![Cell { index: 0, percent: p[0] }],
            _ => p
                .chunks_exact(4)
                .map(|c| Cell { percent: c[0], index: c[3] })
                .collect(),
        })
    }

    pub fn volume(&mut self) -> io::Result<Option<Volume>> {
        Ok(self.read(0x05, 0x05)?.and_then(|p| match p.as_slice() {
            [steps, current, ..] => Some(Volume { current: *current, steps: *steps }),
            _ => None,
        }))
    }

    /// Clamped to the range the device reports rather than a hardcoded one.
    pub fn set_volume(&mut self, level: u8) -> io::Result<()> {
        let max = match self.volume()? {
            Some(v) => v.steps.saturating_sub(1),
            None => return Err(unsupported("this device does not report a volume")),
        };
        self.write(0x05, 0x05, vec![level.min(max)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Scripted, Session};

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn dev(extra: Scripted) -> Device<Scripted> {
        let mut s = extra
            .on(&hex("00010100"), &hex("00010305312e302e34"))
            .on(&hex("00030100"), &hex("00030303400c02"));
        for f in [0x00u8, 0x01, 0x02, 0x05, 0x1f] {
            s = s.on(&[f, 0x00, 0x01, 0x00], &[f, 0x00, 0x03, 0x01, 0x00]);
        }
        Device::open(Session::open(s).unwrap()).unwrap()
    }

    #[test]
    fn reads_the_name_without_its_leading_zero() {
        let mut d = dev(Scripted::new().on(&hex("01020100"), &hex("010203050041424344")));
        assert_eq!(d.name().unwrap().as_deref(), Some("ABCD"));
    }

    #[test]
    fn writes_the_name_without_one() {
        // The read carries a leading 00 and the write does not; sending it back
        // verbatim would name the device "\0ABCD".
        let mut d = dev(Scripted::new());
        d.set_name("ABCD").unwrap();
        assert!(d.session().transport_sent().contains(&hex("0102020441424344")));
    }

    #[test]
    fn battery_reads_one_cell_on_a_qc35_and_several_on_earbuds() {
        let mut a = dev(Scripted::new().on(&hex("02020100"), &hex("0202030146")));
        assert_eq!(a.battery().unwrap(), vec![Cell { index: 0, percent: 0x46 }]);

        let mut b = dev(Scripted::new().on(&hex("02020100"), &hex("0202030864ffff0164ffff02")));
        assert_eq!(
            b.battery().unwrap(),
            vec![Cell { index: 1, percent: 100 }, Cell { index: 2, percent: 100 }]
        );
    }

    #[test]
    fn volume_is_clamped_to_the_scale_the_device_reports() {
        let mut d = dev(Scripted::new().on(&hex("05050100"), &hex("05050302190e")));
        assert_eq!(d.volume().unwrap(), Some(Volume { current: 0x0e, steps: 0x19 }));
        d.set_volume(0xff).unwrap();
        // 0x19 positions means a maximum of 0x18, not 0xff.
        assert!(d.session().transport_sent().contains(&hex("0505020118")));
    }

    #[test]
    fn auto_off_distinguishes_never_from_zero_minutes() {
        let mut d = dev(Scripted::new().on(&hex("01040100"), &hex("010403013c")));
        assert_eq!(d.auto_off().unwrap(), Some(Some(60)));
        let mut d = dev(Scripted::new().on(&hex("01040100"), &hex("0104030100")));
        assert_eq!(d.auto_off().unwrap(), Some(None));
    }

    #[test]
    fn toggles_share_one_path_including_the_one_that_lives_elsewhere() {
        let mut d = dev(Scripted::new()
            .on(&hex("010a0100"), &hex("010a030101"))
            .on(&hex("1f050100"), &hex("1f05030100")));
        assert_eq!(d.toggle(Toggle::Multipoint).unwrap(), Some(true));
        // Remember-my-mode is function 0x1f, not 0x01, because it belongs to
        // the mode system rather than the settings block.
        assert_eq!(d.toggle(Toggle::RememberMode).unwrap(), Some(false));
        d.set_toggle(Toggle::RememberMode, true).unwrap();
        assert!(d.session().transport_sent().contains(&hex("1f05020101")));
    }

    #[test]
    fn language_survives_a_round_trip_and_unknown_values_pass_through() {
        let mut d = dev(Scripted::new().on(&hex("01030100"), &hex("01030305a10004c3de")));
        let p = d.prompts().unwrap().unwrap();
        // 0xa1: bit 0x20 set, so announcements on; the low bits are not 0x21.
        assert!(p.battery_announcement);
        assert_eq!(p.language, Language::Other(0xa1));
        d.set_prompts(Prompts { language: Language::Spanish, battery_announcement: false })
            .unwrap();
        assert!(d.session().transport_sent().contains(&hex("010302022600")));
    }
}
