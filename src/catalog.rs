//! Every record this crate knows, and how well it knows it.
//!
//! This is the machine-readable half of the reference. A finding lands here as
//! one entry — address, label, codec, evidence, note — and nothing else in the
//! crate has to be told about it: [`CATALOG`] carries it into the surface
//! probe, the `raw` command and the generated documentation.
//!
//! Notes are copied from the reference rather than paraphrased, so the two can
//! be diffed.

// Notes carry byte layouts — `<pct> ff ff <index>` — which rustdoc reads as
// unclosed HTML. Backticking them would put punctuation into the `catalog`
// command's output, where the same strings are a terminal table.
#![allow(rustdoc::invalid_html_tags)]

use crate::codec::{self, *};
use crate::transport::Address;
use crate::wire::Addr;

/// How well a direction of a record is understood.
///
/// The reference's own lesson, made structural: **a capture establishes the
/// syntax, not the semantics.** Only [`Evidence::Confirmed`] is writable, so a
/// format that was seen on the wire but never shown to change anything cannot
/// be sent by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// A write changed what a read returned, or a read was matched against a
    /// value known beforehand.
    Confirmed,
    /// Seen on the wire. The effect was never verified.
    Syntax(&'static str),
    /// The form is accepted and changes nothing.
    Ineffective(&'static str),
    /// Not understood, or not present in this direction.
    Unknown,
}

impl Evidence {
    pub fn usable(self) -> bool {
        self == Evidence::Confirmed
    }

    /// What to tell someone who asked for it anyway.
    pub fn why(self) -> &'static str {
        match self {
            Evidence::Confirmed => "understood",
            Evidence::Syntax(w) | Evidence::Ineffective(w) => w,
            Evidence::Unknown => "no confirmed format for this write",
        }
    }
}

/// What is known about a record, without its codec.
///
/// Type-free, so the whole catalog fits in one slice and can be listed, swept
/// and printed by code that knows nothing about any particular value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub addr: Addr,
    pub label: &'static str,
    pub read: Evidence,
    pub write: Evidence,
    /// One line, from the reference.
    pub note: &'static str,
}

/// One record, with the codecs for its two directions.
///
/// `R` and `W` differ where the protocol does — the shortcut reads a
/// model-specific blob and writes a single action byte, and the language record
/// reads a flags byte and writes a pair.
pub struct Field<R: 'static, W: 'static = R> {
    pub meta: Meta,
    pub decode: fn(&[u8]) -> Option<R>,
    pub encode: Option<fn(W) -> Vec<u8>>,
}

/// Declare records. One entry, one line in the reference.
///
/// Expands to a `pub const` [`Field`] per entry plus the flat [`CATALOG`], both
/// from the same text, so the two cannot drift.
macro_rules! catalog {
    ($(
        $(#[$attr:meta])*
        $name:ident : $r:ty $(| $w:ty)? = ($f:literal, $o:literal) $label:literal
            read  $read:expr,  $decode:path,
            write $write:expr, $encode:expr,
            note  $note:literal;
    )*) => {
        $(
            $(#[$attr])*
            #[doc = concat!("`", $label, "` — ", $note)]
            pub const $name: Field<$r $(, $w)?> = Field {
                meta: Meta {
                    addr: Addr::at($f, $o),
                    label: $label,
                    read: $read,
                    write: $write,
                    note: $note,
                },
                decode: $decode,
                encode: $encode,
            };
        )*

        /// Every record above, in declaration order.
        pub const CATALOG: &[Meta] = &[$($name.meta),*];
    };
}

use Evidence::*;

catalog! {
    // -- 00, identity -------------------------------------------------------

    PROTOCOL_VERSION: String = (0x00, 0x01) "protocol version"
        read  Confirmed, codec::ascii,
        write Unknown,   None,
        note  "1.0.4 on a QC35, 1.2.0 on an Ultra; not the firmware";

    DEVICE_ID: DeviceId = (0x00, 0x03) "device id"
        read  Confirmed, codec::device_id,
        write Unknown,   None,
        note  "<id:2> <index>; equals the Modalias product id";

    VERSION: String = (0x00, 0x05) "version"
        read  Confirmed, codec::ascii,
        write Unknown,   None,
        note  "firmware on a QC35, a build string on an Ultra; label it neither";

    OWN_ADDRESS: Address = (0x00, 0x06) "own address"
        read  Confirmed, codec::address,
        write Unknown,   None,
        note  "six bytes; the order is not established, every capture is redacted";

    SERIAL: String = (0x00, 0x07) "serial"
        read  Confirmed, codec::ascii,
        write Unknown,   None,
        note  "ASCII";

    GUID: Vec<u8> = (0x00, 0x0c) "unidentified, 16 bytes"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "GUID-shaped; Ultra only, a QC35 refuses";

    MODEL: String = (0x00, 0x0f) "model name"
        read  Confirmed, codec::ascii,
        write Unknown,   None,
        note  "plain text; Ultra only — a QC35 refuses, so a name table is still needed";

    // -- 01, settings -------------------------------------------------------

    NAME: String = (0x01, 0x02) "name"
        read  Confirmed, codec::name,
        write Confirmed, Some(codec::set_name),
        note  "read carries a leading 00, the write does not; lives in firmware. An Ultra dropped and re-established the link after the write, taking A2DP down with it";

    PROMPTS: Prompts | (Language, bool) = (0x01, 0x03) "language and battery announcement"
        read  Confirmed, codec::prompts,
        write Confirmed, Some(codec::set_prompts),
        note  "write <language> <announcement>, 21 EN 26 ES; the flag reads back at byte 6";

    AUTO_OFF: AutoOff = (0x01, 0x04) "auto-off"
        read  Confirmed, codec::auto_off,
        write Confirmed, Some(codec::set_auto_off),
        note  "minutes, 3c = 60, 00 = never; the Ultra's three-byte form is not mapped";

    ANC_GRADED: Graded | u8 = (0x01, 0x05) "noise cancelling"
        read  Confirmed, codec::graded,
        write Unknown,   None,
        note  "Ultra. 0b <awareness> 03, awareness = 10 - level; no write was ever captured";

    ANC_NAMED: Named | Level = (0x01, 0x06) "noise cancelling"
        read  Confirmed, codec::named,
        write Confirmed, Some(codec::set_named),
        note  "QC35. <level> <mask>; 0b = bits 0,1,3, so only off, high and low";

    EQUALISER: Vec<Band> | (u8, i8) = (0x01, 0x07) "equaliser"
        read  Confirmed, codec::equaliser,
        write Confirmed, Some(codec::set_band),
        note  "RangeControl, which the equaliser uses. Ultra. <min> <max> <value> <band> per band; write <value> <band>, signed";

    SHORTCUT: Vec<u8> | u8 = (0x01, 0x09) "shortcut action"
        read  Unknown,   codec::raw,
        write Confirmed, Some(codec::set_shortcut),
        note  "write 80 09 <action>, 0e 03 13 01 seen; the read has a different, per-model shape";

    MULTIPOINT: bool = (0x01, 0x0a) "multipoint"
        read  Confirmed, codec::flag,
        write Confirmed, Some(codec::set_flag),
        note  "two devices at once";

    SELF_VOICE: Vec<u8> | u8 = (0x01, 0x0b) "self-voice in calls"
        read  Unknown,   codec::raw,
        write Confirmed, Some(codec::set_self_voice),
        note  "write 01 <level>, 00 01 03 seen; the read layout is not established";

    UNKNOWN_01_0C: Vec<u8> = (0x01, 0x0c) "unidentified"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "one write captured, never identified";

    HEAD_DETECTION: bool = (0x01, 0x18) "auto play/pause"
        read  Confirmed, codec::flag,
        write Confirmed, Some(codec::set_flag),
        note  "AutoPlayPause: pause when they come off. Distinct from OnHeadDetection, which is 01 10";

    AUTO_ANSWER: bool = (0x01, 0x1b) "auto-answer on wearing"
        read  Confirmed, codec::flag,
        write Confirmed, Some(codec::set_flag),
        note  "answer a call by putting them on";

    // -- 02, status ---------------------------------------------------------

    BATTERY: Vec<Cell> = (0x02, 0x02) "battery"
        read  Confirmed, codec::battery,
        write Unknown,   None,
        note  "one byte on a QC35, <pct> ff ff <index> per cell on an Ultra";

    CLOCK_02_0D: Vec<u8> = (0x02, 0x0d) "battery log"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "BatteryLog. The ASCII 'empty' never varies; the trailing counter tracks time between reads, so it is not device state";

    CLOCK_02_0E: Vec<u8> = (0x02, 0x0e) "battery log, raw"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "BatteryLogRaw. Equal to 02 0d here; a field that moves every read manufactures correlations";

    // -- 04, pairing --------------------------------------------------------

    PAIRED: Vec<Address> = (0x04, 0x04) "paired devices"
        read  Confirmed, codec::addresses,
        write Unknown,   None,
        note  "ListDevices. A leading byte then one address each — the byte is not a count (01 before three on a QC35, 03 before three on an Ultra); possibly a connected-slot mask";

    ACTIVE_DEVICE: Address = (0x04, 0x09) "the app's own address"
        read  Confirmed, codec::address,
        write Unknown,   None,
        note  "AppAddress, not the active source — every model returns the same one, the host's";

    // -- 05, media ----------------------------------------------------------

    ACTIVE_DEVICE_05: Vec<u8> = (0x05, 0x01) "active device"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "00 02 01 <addr:6>; documented in the original project and confirmed on a QC35";

    VOLUME: Volume | u8 = (0x05, 0x05) "volume"
        read  Confirmed, codec::volume,
        write Confirmed, Some(codec::set_volume),
        note  "<steps> <current>; 19 = 25 positions, 0-0x18";

    IMMERSIVE: Immersive = (0x05, 0x0f) "immersive audio"
        read  Confirmed, codec::immersive,
        write Unknown,   None,
        note  "Ultra. 00 off, 01 still, 02 motion";

    // -- 07, unidentified ---------------------------------------------------

    UNKNOWN_07_01: Vec<u8> = (0x07, 0x01) "unidentified"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "two bytes on an Ultra; moved during none of thirty labelled observations";

    // -- 1f, modes ----------------------------------------------------------

    UNKNOWN_1F_02: Vec<u8> = (0x1f, 0x02) "unidentified"
        read  Unknown,   codec::raw,
        write Unknown,   None,
        note  "seven bytes on an Ultra";

    CURRENT_MODE: u8 = (0x1f, 0x03) "current mode"
        read  Confirmed, codec::byte,
        write Ineffective("a plain SetGet is refused; selection is a Start of \
                           <index> <prompt>, which Device::set does not send"),
              Some(codec::set_byte),
        note  "Ultra. Reading tracks the selected mode. Selection is confirmed: \
               1f 03 05 02 <index> <play voice prompt>, a Start, replying Result \
               with the resulting index. Not a get/set field — it is an operation";

    REMEMBER_MODE: bool = (0x1f, 0x05) "remember my mode"
        read  Confirmed, codec::flag,
        write Confirmed, Some(codec::set_flag),
        note  "returns to the last mode on power-up; lives with the modes, not the toggles";

    /// One slot. The table is ten of these and arrives as an enumeration, so
    /// [`crate::api`] reads it with `enumerate` and this codec, not with `get`.
    MODE_SLOT: Option<Mode> | codec::ModeConfig = (0x1f, 0x06) "mode table slot"
        read  Confirmed, codec::mode,
        write Confirmed, Some(codec::set_mode),
        note  "ModeConfig. Read 47 bytes on an Ultra HP, 44 on an Earbuds II: byte 2 prompt (0 = empty), 42 cncLevel, 46 wind block, byte 5 favorite not occupancy. The write is a shorter layout — see codec::set_mode — confirmed by writing wind block on and reading byte 46 back";

    MODE_SLOTS: u8 = (0x1f, 0x08) "mode slot count"
        read  Confirmed, codec::byte,
        write Unknown,   None,
        note  "0a 00 1f; 0a is the number of slots";
}

/// Every function the catalog mentions, in order.
///
/// What a surface probe enumerates. Sweeping all sixty-four instead costs a
/// round trip each, and a silent function costs the whole receive timeout.
pub fn functions() -> impl Iterator<Item = u8> {
    let mut seen = [false; 256];
    CATALOG.iter().map(|m| m.addr.function).filter(move |&f| !std::mem::replace(&mut seen[f as usize], true))
}

/// Look a record up by address, for the `raw` command and for error reporting.
pub fn find(addr: Addr) -> Option<&'static Meta> {
    CATALOG.iter().find(|m| m.addr == addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_no_duplicate_addresses() {
        // Two entries for one address means one of them is never reached, and
        // the flat list is what the surface probe and `raw` both walk.
        let mut seen: Vec<Addr> = CATALOG.iter().map(|m| m.addr).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before);
    }

    #[test]
    fn an_unconfirmed_write_carries_a_reason_rather_than_an_encoder() {
        // `1f 03` is the case this exists for: selection works, but as a Start
        // of <index> <prompt>, which Device::set (a SetGet) does not send. So
        // the byte codec here is not usable, and the reason says why.
        assert!(!CURRENT_MODE.meta.write.usable());
        assert!(CURRENT_MODE.meta.write.why().contains("Start"));
        // And `01 05` has no captured write at all.
        assert_eq!(ANC_GRADED.meta.write, Unknown);
        assert!(ANC_GRADED.encode.is_none());
    }

    #[test]
    fn evidence_and_encoder_must_agree_before_anything_is_sent() {
        // Neither alone is enough. `Device::set` requires both, so a record
        // marked confirmed without an encoder degrades to a refusal rather than
        // to a blind write.
        assert!(VOLUME.encode.is_some() && VOLUME.meta.write.usable());
        assert!(BATTERY.encode.is_none() && !BATTERY.meta.write.usable());
        // The interesting middle: an encoder exists and must not be used.
        assert!(CURRENT_MODE.encode.is_some() && !CURRENT_MODE.meta.write.usable());
    }

    #[test]
    fn functions_are_listed_once_each_in_declaration_order() {
        assert_eq!(functions().collect::<Vec<_>>(), vec![0x00, 0x01, 0x02, 0x04, 0x05, 0x07, 0x1f]);
    }

    #[test]
    fn a_record_can_be_found_by_address() {
        assert_eq!(find(Addr::at(0x05, 0x05)).unwrap().label, "volume");
        assert!(find(Addr::at(0x03, 0x03)).is_none());
    }
}
