//! Payloads to values, and back.
//!
//! Pure functions and the types they produce. Nothing here reaches a device, so
//! every one of them can be checked against a byte string copied out of the
//! reference.
//!
//! A decoder returns `None` when the payload is not the shape it knows. That is
//! not a refusal — it is this crate being wrong about a model — and it surfaces
//! as [`crate::error::Error::Malformed`] with the bytes attached, so the next
//! observation starts from what the device actually said.

use crate::transport::Address;

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

pub fn raw(p: &[u8]) -> Option<Vec<u8>> {
    Some(p.to_vec())
}

pub fn ascii(p: &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(p).trim_matches('\0').to_string())
}

pub fn byte(p: &[u8]) -> Option<u8> {
    p.first().copied()
}

pub fn flag(p: &[u8]) -> Option<bool> {
    p.first().map(|&b| b != 0)
}

pub fn set_flag(on: bool) -> Vec<u8> {
    vec![u8::from(on)]
}

pub fn set_byte(b: u8) -> Vec<u8> {
    vec![b]
}

/// Six bytes as an address.
///
/// The order the device uses is **not established** — every address in the
/// reference is redacted, so a capture cannot settle it. Bytes are taken as
/// they arrive.
pub fn address(p: &[u8]) -> Option<Address> {
    <[u8; 6]>::try_from(p.get(..6)?).ok().map(Address)
}

/// `<count> <addr…>`.
pub fn addresses(p: &[u8]) -> Option<Vec<Address>> {
    Some(p.get(1..)?.chunks_exact(6).filter_map(address).collect())
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// `00 03`, and the same number bluez reports as the `Modalias` product id — so
/// it can also be had without connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceId {
    pub id: u16,
    pub index: u8,
}

pub fn device_id(p: &[u8]) -> Option<DeviceId> {
    match p {
        [hi, lo, index, ..] => Some(DeviceId {
            id: u16::from(*hi) << 8 | u16::from(*lo),
            index: *index,
        }),
        _ => None,
    }
}

/// `01 02`. The read carries a leading `00`; the write does not.
pub fn name(p: &[u8]) -> Option<String> {
    ascii(if p.first() == Some(&0) { &p[1..] } else { p })
}

pub fn set_name(n: String) -> Vec<u8> {
    n.into_bytes()
}

// ---------------------------------------------------------------------------
// Noise cancelling
// ---------------------------------------------------------------------------

/// A named noise-cancelling level, on models that offer a fixed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    High,
    Low,
}

impl Level {
    pub fn wire(self) -> u8 {
        match self {
            Level::Off => 0x00,
            Level::High => 0x01,
            Level::Low => 0x03,
        }
    }

    pub fn from_wire(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Level::Off),
            0x01 => Some(Level::High),
            0x03 => Some(Level::Low),
            _ => None,
        }
    }

    pub const ALL: [Level; 3] = [Level::Off, Level::High, Level::Low];
}

/// `01 06`, QuietComfort 35. `<level> <mask>`.
///
/// Three named levels and a bitmask saying which the model accepts — bit *n*
/// set means level *n* is allowed. Confirmed by writing every value `0x00`–`0x0b`
/// and having eight of the twelve refused with payload `06`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Named {
    pub level: Option<Level>,
    pub accepted: u8,
}

impl Named {
    pub fn accepts(&self, level: Level) -> bool {
        self.accepted & (1 << level.wire()) != 0
    }

    pub fn levels(&self) -> impl Iterator<Item = Level> + '_ {
        Level::ALL.into_iter().filter(|&l| self.accepts(l))
    }
}

pub fn named(p: &[u8]) -> Option<Named> {
    match p {
        [level, accepted, ..] => Some(Named { level: Level::from_wire(*level), accepted: *accepted }),
        _ => None,
    }
}

pub fn set_named(l: Level) -> Vec<u8> {
    vec![l.wire()]
}

/// `01 05`, Ultra family. `<values> <awareness> <unknown>`.
///
/// The field counts **awareness**, not cancellation: `0` is maximum cancelling
/// and `values - 1` lets everything through. Eleven wire values `0x00`–`0x0a`,
/// of which the app's live slider shows ten; the eleventh is Aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Graded {
    pub awareness: u8,
    pub values: u8,
}

impl Graded {
    /// Cancellation the way a person pictures it: `0` none, higher is more.
    pub fn cancelling(&self) -> u8 {
        self.top().saturating_sub(self.awareness)
    }

    /// The highest cancellation this device offers.
    pub fn top(&self) -> u8 {
        self.values.saturating_sub(1)
    }
}

pub fn graded(p: &[u8]) -> Option<Graded> {
    match p {
        [values, awareness, ..] => Some(Graded { awareness: *awareness, values: *values }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The rest of function 01
// ---------------------------------------------------------------------------

/// Voice-prompt language.
///
/// The read and the write do not carry the same byte. What comes back is a
/// flags byte whose low five bits select the language; the write sends
/// `0x20 | language`, which is where `21` and `26` come from. Comparing the
/// whole byte against `21` therefore never matches a real device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Spanish,
    Other(u8),
}

impl Language {
    pub fn code(self) -> u8 {
        match self {
            Language::English => 0x01,
            Language::Spanish => 0x06,
            Language::Other(c) => c & 0x1f,
        }
    }

    pub fn wire(self) -> u8 {
        0x20 | self.code()
    }

    pub fn from_flags(b: u8) -> Self {
        match b & 0x1f {
            0x01 => Language::English,
            0x06 => Language::Spanish,
            other => Language::Other(other),
        }
    }
}

/// `01 03`, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompts {
    pub language: Language,
    /// Bit `0x20`.
    pub voice_prompts: bool,
    /// The whole first byte. Bit `0x40` is unexplained: the same Ultra shows it
    /// set in English and clear in Spanish, and a QC35 in English has it clear,
    /// so it is neither generation nor language on its own.
    pub flags: u8,
    /// Where its position is known.
    ///
    /// The write is `<language> <battery announcement>`, but the second field
    /// does not read back second — on the seven-byte record it lands last, and
    /// byte 1 is a constant `00`. Confirmed by writing both values and watching
    /// byte 6 follow. The QC35's five-byte record is not mapped.
    pub battery_announcement: Option<bool>,
    /// The whole record. Most of it is still unidentified.
    pub raw: Vec<u8>,
}

/// Where the battery announcement reads back, for record layouts where that has
/// been established by writing both values and watching which byte followed.
pub fn announcement_at(record: &[u8]) -> Option<usize> {
    match record.len() {
        7 => Some(6),
        _ => None,
    }
}

pub fn prompts(p: &[u8]) -> Option<Prompts> {
    let flags = *p.first()?;
    Some(Prompts {
        language: Language::from_flags(flags),
        voice_prompts: flags & 0x20 != 0,
        flags,
        battery_announcement: announcement_at(p).map(|i| p[i] != 0),
        raw: p.to_vec(),
    })
}

/// The record is written whole, so the announcement travels with the language
/// whether or not the caller meant to touch it.
pub fn set_prompts((language, announcement): (Language, bool)) -> Vec<u8> {
    vec![language.wire(), u8::from(announcement)]
}

/// How long the headphones wait, idle, before powering themselves off.
///
/// Zero minutes on the wire means never, not immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoOff {
    Never,
    After(u8),
}

impl AutoOff {
    pub fn from_minutes(m: u8) -> Self {
        if m == 0 { AutoOff::Never } else { AutoOff::After(m) }
    }

    pub fn minutes(self) -> u8 {
        match self {
            AutoOff::Never => 0,
            AutoOff::After(m) => m,
        }
    }
}

impl std::fmt::Display for AutoOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoOff::Never => write!(f, "never"),
            AutoOff::After(m) => write!(f, "{m} min"),
        }
    }
}

/// Only the one-byte form is understood. The Ultra generation answers with
/// three — `05 00 00` — and a bare minute count no longer fits, so that shape is
/// rejected rather than read as five minutes.
pub fn auto_off(p: &[u8]) -> Option<AutoOff> {
    match p {
        [m] => Some(AutoOff::from_minutes(*m)),
        _ => None,
    }
}

pub fn set_auto_off(a: AutoOff) -> Vec<u8> {
    vec![a.minutes()]
}

/// One equaliser band, carrying its own limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub index: u8,
    pub min: i8,
    pub max: i8,
    pub value: i8,
}

/// Three four-byte groups, `<min> <max> <value> <band>`. A client can draw the
/// control without hardcoding limits.
pub fn equaliser(p: &[u8]) -> Option<Vec<Band>> {
    Some(
        p.chunks_exact(4)
            .map(|c| Band { min: c[0] as i8, max: c[1] as i8, value: c[2] as i8, index: c[3] })
            .collect(),
    )
}

/// `<value> <band>`, signed.
pub fn set_band((index, value): (u8, i8)) -> Vec<u8> {
    vec![value as u8, index]
}

/// `80 09 <action>`. Four action values were captured — `0e`, `03`, `13`, `01` —
/// but which is which was never established: the capture changed the enabled
/// state and the selection together.
pub fn set_shortcut(action: u8) -> Vec<u8> {
    vec![0x80, 0x09, action]
}

/// `01 <level>`. Levels `00`, `01` and `03` seen.
pub fn set_self_voice(level: u8) -> Vec<u8> {
    vec![0x01, level]
}

// ---------------------------------------------------------------------------
// Status, media, modes
// ---------------------------------------------------------------------------

/// One battery cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub index: u8,
    pub percent: u8,
}

/// A QuietComfort 35 reports a bare percentage; the Ultra family reports a
/// four-byte record per cell, so earbuds give two and a headband gives one.
/// Branch on the length, never on the model.
pub fn battery(p: &[u8]) -> Option<Vec<Cell>> {
    Some(match p {
        [percent] => vec![Cell { index: 0, percent: *percent }],
        _ => p.chunks_exact(4).map(|c| Cell { percent: c[0], index: c[3] }).collect(),
    })
}

/// `05 05`. The device states the size of its own scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub current: u8,
    /// Positions on the scale; `0x19` on every model seen, a range of `0`–`0x18`.
    pub steps: u8,
}

impl Volume {
    pub fn max(&self) -> u8 {
        self.steps.saturating_sub(1)
    }
}

pub fn volume(p: &[u8]) -> Option<Volume> {
    match p {
        [steps, current, ..] => Some(Volume { current: *current, steps: *steps }),
        _ => None,
    }
}

pub fn set_volume(level: u8) -> Vec<u8> {
    vec![level]
}

/// Immersive audio, on the Ultra generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Immersive {
    Off,
    Still,
    Motion,
}

pub fn immersive(p: &[u8]) -> Option<Immersive> {
    match p.first()? {
        0x00 => Some(Immersive::Off),
        0x01 => Some(Immersive::Still),
        0x02 => Some(Immersive::Motion),
        _ => None,
    }
}

/// One stored mode, from a 47-byte `1f 06` record.
///
/// `awareness` is the same inverted scale as [`Graded`]. `wind_block` forces
/// cancellation to its maximum while on, so the level is not independently
/// settable in that state.
///
/// Most of the record is still unidentified, so `raw` carries all 47 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub index: u8,
    pub name: String,
    pub awareness: u8,
    pub wind_block: bool,
    pub raw: Vec<u8>,
}

/// Byte 5 is slot occupancy; an empty slot decodes to `None`.
///
/// Byte 46 is wind block and is `00` on any mode that does not use it, so
/// reading *that* as occupancy silently drops real modes.
pub fn mode(p: &[u8]) -> Option<Option<Mode>> {
    if p.len() < 47 {
        return None;
    }
    if p[5] != 0x01 {
        return Some(None);
    }
    // The name runs from byte 6 to the settings tail at byte 41.
    let name = p[6..41].split(|&b| b == 0).next().unwrap_or_default();
    Some(Some(Mode {
        index: p[0],
        name: String::from_utf8_lossy(name).to_string(),
        awareness: p[42],
        wind_block: p[46] != 0,
        raw: p.to_vec(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_reads_one_cell_on_a_qc35_and_several_on_earbuds() {
        assert_eq!(battery(&[0x46]).unwrap(), vec![Cell { index: 0, percent: 0x46 }]);
        let earbuds = [0x64, 0xff, 0xff, 0x01, 0x64, 0xff, 0xff, 0x02];
        assert_eq!(
            battery(&earbuds).unwrap(),
            vec![Cell { index: 1, percent: 100 }, Cell { index: 2, percent: 100 }]
        );
    }

    #[test]
    fn the_graded_scale_is_inverted_and_the_type_un_inverts_it() {
        // Awareness 0x0a of eleven values is no cancelling at all: Aware mode.
        assert_eq!(graded(&[0x0b, 0x0a, 0x03]).unwrap().cancelling(), 0);
        assert_eq!(graded(&[0x0b, 0x00, 0x03]).unwrap().cancelling(), 10);
        assert_eq!(graded(&[0x0b, 0x05, 0x03]).unwrap().cancelling(), 5);
    }

    #[test]
    fn the_mask_says_which_levels_a_named_model_accepts() {
        // 0x0b is bits 0, 1 and 3 — off, high and low. Writing anything else to
        // a QC35 is refused with payload 06.
        let a = named(&[0x01, 0x0b]).unwrap();
        assert_eq!(a.level, Some(Level::High));
        assert_eq!(a.levels().collect::<Vec<_>>(), Level::ALL);
        // A model offering only off and high would say so in the same byte.
        assert!(!named(&[0x00, 0x03]).unwrap().accepts(Level::Low));
    }

    #[test]
    fn the_name_read_drops_its_leading_zero_and_the_write_adds_none() {
        assert_eq!(name(b"\0ABCD").unwrap(), "ABCD");
        assert_eq!(set_name("ABCD".into()), b"ABCD");
    }

    #[test]
    fn auto_off_distinguishes_never_from_zero_minutes() {
        assert_eq!(auto_off(&[0x3c]), Some(AutoOff::After(60)));
        assert_eq!(auto_off(&[0x00]), Some(AutoOff::Never));
        // The Ultra's three-byte form is not a minute count; reading p[0] would
        // report five minutes.
        assert_eq!(auto_off(&[0x05, 0x00, 0x00]), None);
    }

    #[test]
    fn the_language_read_is_a_flags_byte_not_the_write_value() {
        // A real QC35 with English selected answers 0xa1, never 0x21.
        let p = prompts(&[0xa1, 0x00, 0x04, 0xc3, 0xde]).unwrap();
        assert_eq!(p.language, Language::English);
        assert!(p.voice_prompts);
        // Five bytes: the announcement's position is not established there.
        assert_eq!(p.battery_announcement, None);
        assert_eq!(set_prompts((Language::Spanish, false)), vec![0x26, 0x00]);
    }

    #[test]
    fn the_announcement_reads_back_last_not_second() {
        // Byte 1 is a constant 00; preserving it across a language change turned
        // the announcement off on a real device.
        let p = prompts(&[0xe1, 0x00, 0x01, 0x81, 0x5e, 0x01, 0x01]).unwrap();
        assert_eq!(p.battery_announcement, Some(true));
    }

    #[test]
    fn the_equaliser_reports_each_bands_own_range() {
        let bands = equaliser(&[0xf6, 0x0a, 0x02, 0x00, 0xf6, 0x0a, 0x00, 0x01]).unwrap();
        assert_eq!((bands[0].min, bands[0].max, bands[0].value), (-10, 10, 2));
        assert_eq!(bands[1].index, 1);
        assert_eq!(set_band((0, 6)), vec![0x06, 0x00]);
    }

    /// One 47-byte mode record the way a device sends it.
    fn record(index: u8, occupied: bool, name: &str, awareness: u8, wind: u8) -> Vec<u8> {
        let mut r = vec![0u8; 47];
        r[0] = index;
        r[5] = u8::from(occupied);
        r[6..6 + name.len()].copy_from_slice(name.as_bytes());
        r[42] = awareness;
        r[46] = wind;
        r
    }

    #[test]
    fn a_stored_mode_with_wind_block_off_is_still_a_stored_mode() {
        // Byte 46 is wind block, not occupancy. Every mode that does not use it
        // carries 0 there, so filtering on it drops real modes.
        let focus = mode(&record(3, true, "Focus", 0x05, 0x00)).unwrap().unwrap();
        assert_eq!((focus.index, focus.name.as_str(), focus.awareness), (3, "Focus", 5));
        assert!(!focus.wind_block);
        assert_eq!(mode(&record(5, false, "None", 0x00, 0x00)).unwrap(), None);
    }

    #[test]
    fn volume_states_the_size_of_its_own_scale() {
        let v = volume(&[0x19, 0x0e]).unwrap();
        assert_eq!((v.current, v.steps, v.max()), (14, 25, 24));
    }
}
