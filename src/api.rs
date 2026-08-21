//! Names for the verbs.
//!
//! One line each over [`Device::get`] and [`Device::set`], so a caller writes
//! `dev.battery()` and this crate keeps a single code path. Anything the
//! catalog knows is reachable through `get` the moment it is declared; a name
//! here is a convenience, added when one is worth having.
//!
//! The few functions with a body are the ones where the protocol imposes
//! something a caller should not have to know: a scale to clamp to, a field
//! that travels with another, an opcode that moved between generations.

use crate::catalog::*;
use crate::codec::*;
use crate::device::Device;
use crate::error::{Error, Result};
use crate::transport::Transport;

/// A setting that is simply on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Multipoint,
    HeadDetection,
    AutoAnswer,
    /// Lives with the modes rather than with the other toggles, which is where
    /// it belongs: it is a property of the mode system.
    RememberMode,
}

impl Toggle {
    pub fn field(self) -> Field<bool> {
        match self {
            Toggle::Multipoint => MULTIPOINT,
            Toggle::HeadDetection => HEAD_DETECTION,
            Toggle::AutoAnswer => AUTO_ANSWER,
            Toggle::RememberMode => REMEMBER_MODE,
        }
    }

    pub const ALL: [Toggle; 4] = [
        Toggle::Multipoint,
        Toggle::HeadDetection,
        Toggle::AutoAnswer,
        Toggle::RememberMode,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Toggle::Multipoint => "multipoint",
            Toggle::HeadDetection => "head-detection",
            Toggle::AutoAnswer => "auto-answer",
            Toggle::RememberMode => "remember-mode",
        }
    }
}

/// The noise-cancelling control, whichever kind this model has.
///
/// The two generations do not share the field, so neither does this type. A
/// QuietComfort 35 offers three named levels and a mask saying which; the Ultra
/// family offers a scale that counts awareness rather than cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anc {
    Named(Named),
    Graded(Graded),
}

/// What a model has, from its surface rather than from a table of device ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supports {
    pub anc: Option<AncKind>,
    pub equaliser: bool,
    pub immersive: bool,
    pub modes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncKind {
    Named,
    Graded,
}

impl AncKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AncKind::Named => "named",
            AncKind::Graded => "graded",
        }
    }
}

impl<T: Transport> Device<T> {
    // -- identity and shape -------------------------------------------------

    pub fn supports(&self) -> Supports {
        Supports {
            anc: match (self.has(&ANC_NAMED), self.has(&ANC_GRADED)) {
                (true, _) => Some(AncKind::Named),
                (_, true) => Some(AncKind::Graded),
                _ => None,
            },
            equaliser: self.has(&EQUALISER),
            immersive: self.has(&IMMERSIVE),
            modes: self.has(&MODE_SLOT) || self.has(&CURRENT_MODE),
        }
    }

    // -- function 01 --------------------------------------------------------

    pub fn name(&mut self) -> Result<String> {
        self.get(&NAME)
    }

    /// The name lives in the device's firmware and shows up on every other host
    /// that pairs with it, so this is not a local alias.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        self.set(&NAME, name.to_string())
    }

    pub fn prompts(&mut self) -> Result<Prompts> {
        self.get(&PROMPTS)
    }

    pub fn set_prompts(&mut self, language: Language, announcement: bool) -> Result<()> {
        self.set(&PROMPTS, (language, announcement))
    }

    /// Change the language, leaving the battery announcement as it was.
    ///
    /// The record is written whole, so setting the language means restating the
    /// announcement alongside it — and taking the second byte of the read is
    /// wrong: that byte is a constant `00` and the announcement reads back last.
    /// Preserving the wrong one turns the announcement off as a side effect of a
    /// language change, which is how this was found. Where the layout is not
    /// established, this refuses rather than guessing.
    pub fn set_language(&mut self, language: Language) -> Result<()> {
        let p = self.prompts()?;
        let announcement = p.battery_announcement.ok_or(Error::NotUnderstood {
            addr: PROMPTS.meta.addr,
            why: "this model's record is not mapped; set the announcement too",
        })?;
        self.set(&PROMPTS, (language, announcement))
    }

    pub fn auto_off(&mut self) -> Result<AutoOff> {
        self.get(&AUTO_OFF)
    }

    /// Read first: the Ultra's three-byte field is not a minute count, and
    /// writing one byte into it would be a guess.
    pub fn set_auto_off(&mut self, after: AutoOff) -> Result<()> {
        self.auto_off()?;
        self.set(&AUTO_OFF, after)
    }

    /// Noise cancelling, from whichever opcode this generation uses.
    ///
    /// The surface usually says which. When it does not — `01 05` answers a
    /// refusal around immersive-audio transitions and settles afterwards — both
    /// are tried rather than concluding the device has none.
    pub fn noise_cancelling(&mut self) -> Result<Anc> {
        if self.has(&ANC_NAMED) {
            return self.get(&ANC_NAMED).map(Anc::Named);
        }
        if self.has(&ANC_GRADED) {
            return self.get(&ANC_GRADED).map(Anc::Graded);
        }
        self.get(&ANC_NAMED)
            .map(Anc::Named)
            .or_else(|_| self.get(&ANC_GRADED).map(Anc::Graded))
    }

    /// Select a named level, on a model that offers them.
    ///
    /// The device's own mask decides what is allowed — writing every value
    /// `0x00`–`0x0b` to a QuietComfort 35 had eight of the twelve refused — so
    /// this checks the mask first and reports the model's answer rather than the
    /// wire's.
    pub fn set_level(&mut self, level: Level) -> Result<()> {
        if !self.get(&ANC_NAMED)?.accepts(level) {
            return Err(Error::NotUnderstood {
                addr: ANC_NAMED.meta.addr,
                why: "this model does not offer that level",
            });
        }
        self.set(&ANC_NAMED, level)
    }

    pub fn equaliser(&mut self) -> Result<Vec<Band>> {
        self.get(&EQUALISER)
    }

    /// The value is clamped to the range the device reported, not a hardcoded
    /// one — the record carries its own limits so a client can draw the control
    /// without knowing the model.
    pub fn set_band(&mut self, index: u8, value: i8) -> Result<()> {
        let band = self
            .equaliser()?
            .into_iter()
            .find(|b| b.index == index)
            .ok_or(Error::NotUnderstood {
                addr: EQUALISER.meta.addr,
                why: "no such equaliser band",
            })?;
        self.set(&EQUALISER, (index, value.clamp(band.min, band.max)))
    }

    pub fn toggle(&mut self, t: Toggle) -> Result<bool> {
        self.get(&t.field())
    }

    pub fn set_toggle(&mut self, t: Toggle, on: bool) -> Result<()> {
        self.set(&t.field(), on)
    }

    /// `01 09`, undecoded. The read has a model-specific shape — seven bytes on
    /// the Ultra Earbuds, eleven on the Sport — and arrives once per side, so
    /// indexing it as though it matched the write returns a byte that is not an
    /// action at all.
    pub fn shortcut(&mut self) -> Result<Vec<u8>> {
        self.get(&SHORTCUT)
    }

    pub fn set_shortcut(&mut self, action: u8) -> Result<()> {
        self.set(&SHORTCUT, action)
    }

    pub fn set_self_voice(&mut self, level: u8) -> Result<()> {
        self.set(&SELF_VOICE, level)
    }

    // -- status and media ---------------------------------------------------

    pub fn battery(&mut self) -> Result<Vec<Cell>> {
        self.get(&BATTERY)
    }

    pub fn volume(&mut self) -> Result<Volume> {
        self.get(&VOLUME)
    }

    /// Clamped to the scale the device reports.
    pub fn set_volume(&mut self, level: u8) -> Result<()> {
        let max = self.volume()?.max();
        self.set(&VOLUME, level.min(max))
    }

    pub fn immersive(&mut self) -> Result<Immersive> {
        self.get(&IMMERSIVE)
    }

    // -- modes --------------------------------------------------------------

    /// The stored modes. Empty slots are dropped.
    ///
    /// The table is ten records under one opcode, so it arrives as an
    /// enumeration rather than as a read — but it decodes with the catalog's
    /// codec like anything else.
    pub fn modes(&mut self) -> Result<Vec<Mode>> {
        let addr = MODE_SLOT.meta.addr;
        if !self.has(&MODE_SLOT) && !self.has(&CURRENT_MODE) {
            return Err(Error::Absent(addr));
        }
        let reply = self.session().enumerate(addr.function)?;
        Ok(reply
            .iter()
            .filter(|m| m.addr == addr && m.refusal().is_none())
            .filter_map(|m| (MODE_SLOT.decode)(&m.payload))
            .flatten()
            .collect())
    }

    /// The index of the mode currently active. Reading works; the write does
    /// not, and [`Device::set`] says so from the catalog rather than from here.
    pub fn current_mode(&mut self) -> Result<u8> {
        self.get(&CURRENT_MODE)
    }

    pub fn select_mode(&mut self, index: u8) -> Result<()> {
        self.set(&CURRENT_MODE, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::session::Session;
    use crate::transport::Scripted;

    fn open(t: Scripted) -> Device<Scripted> {
        Device::open(Session::open(t).unwrap()).unwrap()
    }

    fn qc35() -> Device<Scripted> {
        open(fixtures::qc35())
    }

    fn ultra() -> Device<Scripted> {
        open(fixtures::ultra_hp())
    }

    fn sent(d: &mut Device<Scripted>, bytes: &[u8]) -> bool {
        d.session().transport_sent().iter().any(|s| s == bytes)
    }

    #[test]
    fn finds_noise_cancelling_where_each_generation_keeps_it() {
        assert!(matches!(qc35().noise_cancelling(), Ok(Anc::Named(_))));
        assert!(matches!(ultra().noise_cancelling(), Ok(Anc::Graded(_))));
        // The Sport Earbuds have none, and both opcodes say so.
        assert!(open(fixtures::sport()).noise_cancelling().is_err());
    }

    #[test]
    fn features_that_arrived_with_the_ultra_are_not_claimed_for_older_models() {
        let old = qc35().supports();
        assert_eq!(old.anc, Some(AncKind::Named));
        assert!(!old.equaliser && !old.immersive && !old.modes);
        let new = ultra().supports();
        assert_eq!(new.anc, Some(AncKind::Graded));
        assert!(new.equaliser && new.modes);
    }

    #[test]
    fn the_mask_decides_which_levels_a_named_model_accepts() {
        // Writing every value 0x00-0x0b to a QC35 had eight of the twelve
        // refused. The device states which three in its own reply.
        let mut d = qc35();
        d.set_level(Level::Low).unwrap();
        assert!(sent(&mut d, &[0x01, 0x06, 0x02, 0x01, 0x03]));
    }

    #[test]
    fn a_refused_write_is_an_error_not_a_silent_no_op() {
        // Discarding the reply reports a refusal as success: the caller sets a
        // value, sees no error, and the device is unchanged.
        let mut d = qc35();
        assert!(d.set_name("a-name-the-fixture-refuses").is_err());
    }

    #[test]
    fn writes_the_name_without_the_leading_zero_the_read_carries() {
        // Sending the read back verbatim would name the device "\0qc35".
        let mut d = qc35();
        assert_eq!(d.name().unwrap(), "qc35");
        d.set_name("renamed").unwrap();
        assert!(sent(&mut d, b"\x01\x02\x02\x07renamed"));
    }

    #[test]
    fn volume_is_clamped_to_the_scale_the_device_reports() {
        let mut d = qc35();
        assert_eq!(d.volume().unwrap().max(), 24);
        d.set_volume(0xff).unwrap();
        // 0x19 positions means a maximum of 0x18, not 0xff.
        assert!(sent(&mut d, &[0x05, 0x05, 0x02, 0x01, 0x18]));
    }

    #[test]
    fn changing_the_language_leaves_the_battery_announcement_alone() {
        // The announcement does not read back where it is written — byte 1 is a
        // constant 00 and the flag is byte 6. Preserving byte 1 wrote `21 00`
        // and switched the announcement off, on a real device.
        let mut d = ultra();
        d.set_language(Language::English).unwrap();
        assert!(sent(&mut d, &[0x01, 0x03, 0x02, 0x02, 0x21, 0x01]));
        assert!(!sent(&mut d, &[0x01, 0x03, 0x02, 0x02, 0x21, 0x00]));
    }

    #[test]
    fn a_language_change_is_refused_where_the_layout_is_unmapped() {
        // The QC35's five-byte record was never tested, so the announcement's
        // position is unknown and writing the record whole would guess at it.
        let mut d = qc35();
        assert!(matches!(d.set_language(Language::English), Err(Error::NotUnderstood { .. })));
        assert_eq!(d.prompts().unwrap().battery_announcement, None);
    }

    #[test]
    fn auto_off_refuses_a_layout_it_does_not_know() {
        // The Ultra answers with three bytes; reading p[0] would report five
        // minutes, and writing one byte would guess at a field it cannot see.
        assert!(matches!(ultra().auto_off(), Err(Error::Malformed { .. })));
        assert_eq!(qc35().auto_off().unwrap(), AutoOff::After(60));
    }

    #[test]
    fn toggles_share_one_path_including_the_one_that_lives_elsewhere() {
        let mut d = ultra();
        assert!(d.toggle(Toggle::Multipoint).unwrap());
        assert!(!d.toggle(Toggle::HeadDetection).unwrap());
        // Remember-my-mode is function 1f, not 01, because it belongs to the
        // mode system rather than the settings block.
        d.set_toggle(Toggle::RememberMode, false).unwrap();
        assert!(sent(&mut d, &[0x1f, 0x05, 0x02, 0x01, 0x00]));
    }

    #[test]
    fn the_equaliser_reports_each_bands_own_range_and_clamps_to_it() {
        let mut d = ultra();
        let bands = d.equaliser().unwrap();
        assert_eq!(bands.len(), 3);
        assert_eq!((bands[0].min, bands[0].max), (-10, 10));
        d.set_band(0, 99).unwrap();
        assert!(sent(&mut d, &[0x01, 0x07, 0x02, 0x02, 0x0a, 0x00]));
    }

    #[test]
    fn asking_a_qc35_for_an_equaliser_says_so_rather_than_returning_nothing() {
        // An empty list would invite a client to draw a control with no bands.
        assert!(matches!(qc35().equaliser(), Err(Error::Absent(_))));
    }

    #[test]
    fn reads_immersive_audio_where_it_exists() {
        assert_eq!(ultra().immersive().unwrap(), Immersive::Off);
        assert!(qc35().immersive().is_err());
    }

    #[test]
    fn stored_modes_are_read_and_empty_slots_dropped() {
        let mut d = ultra();
        let modes = d.modes().unwrap();
        let named: Vec<(u8, &str)> = modes.iter().map(|m| (m.index, m.name.as_str())).collect();
        assert_eq!(
            named,
            vec![(0, "Quiet"), (1, "Aware"), (2, "Immersion"), (3, "Focus"), (4, "Home")]
        );
        // Byte 46 is wind block, not occupancy: Quiet has it off and is real.
        assert!(!modes[0].wind_block && modes[4].wind_block);
        assert_eq!(modes[3].awareness, 5);
        assert!(qc35().modes().is_err());
    }

    #[test]
    fn selecting_a_mode_is_refused_because_it_does_not_work() {
        // The app's form is accepted by the device and changes nothing, for
        // every index. A control that silently does nothing is worse than none.
        let mut d = ultra();
        assert_eq!(d.current_mode().unwrap(), 0);
        assert!(matches!(d.select_mode(1), Err(Error::NotUnderstood { .. })));
    }

    #[test]
    fn battery_grew_a_record_structure_between_generations() {
        assert_eq!(qc35().battery().unwrap().len(), 1);
        assert_eq!(open(fixtures::sport()).battery().unwrap().len(), 2);
    }
}
