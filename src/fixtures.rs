//! The observed devices, as data.
//!
//! Transcribed from the reference so that a code path can be exercised without
//! the hardware it was found on — which matters here, because covering the
//! protocol needs headphones from two generations and most people have one.
//!
//! A new finding lands here as one line beside its catalog entry.

use crate::transport::Scripted;
use crate::wire::{Addr, Message, Operator};

fn bytes(addr: Addr, operator: Operator, payload: &[u8]) -> Vec<u8> {
    Message::new(addr, operator, payload.to_vec()).encode().expect("fixture fits")
}

/// A reply carrying a value.
fn status(f: u8, o: u8, payload: &[u8]) -> Vec<u8> {
    bytes(Addr::at(f, o), Operator::Status, payload)
}

/// `04` with a reason: `03` function absent, `04` opcode absent, `05` operator
/// not valid here, `06` argument not accepted.
fn refusal(f: u8, o: u8, code: u8) -> Vec<u8> {
    bytes(Addr::at(f, o), Operator::Error, &[code])
}

/// A function listing itself: `Processing` acknowledges it, the records follow
/// as `Status`, then a zero-length `Result` closes it.
fn listing(f: u8, records: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = bytes(Addr::at(f, 0x01), Operator::Processing, &[]);
    for (o, p) in records {
        out.extend_from_slice(&status(f, *o, p));
    }
    out.extend_from_slice(&bytes(Addr::at(f, 0x01), Operator::Result, &[]));
    out
}

/// A device that answers `records` and refuses everything else with the code a
/// real one uses for a function it does not implement.
struct Profile(Scripted);

impl Profile {
    fn new() -> Self {
        // The handshake. Some devices answer nothing at all until it is sent.
        Profile(Scripted::new().on(&[0x00, 0x01, 0x01, 0x00], &status(0x00, 0x01, b"1.0.4")))
    }

    fn read(mut self, f: u8, o: u8, payload: &[u8]) -> Self {
        self.0 = self.0.on(&[f, o, 0x01, 0x00], &status(f, o, payload));
        self
    }

    /// A read, plus the writes this record accepts. Anything else is refused
    /// with `06`, argument not accepted — which is what a QuietComfort 35 does
    /// to eight of the twelve noise-cancelling values.
    fn writable(mut self, f: u8, o: u8, payload: &[u8], accepted: &[&[u8]]) -> Self {
        self = self.read(f, o, payload);
        for w in accepted {
            let mut req = vec![f, o, 0x02, w.len() as u8];
            req.extend_from_slice(w);
            self.0 = self.0.on(&req, &status(f, o, payload));
        }
        self
    }

    fn lists(mut self, f: u8, records: &[(u8, &[u8])]) -> Self {
        self.0 = self.0.on(&[f, 0x01, 0x05, 0x00], &listing(f, records));
        for (o, p) in records {
            self.0 = self.0.on(&[f, *o, 0x01, 0x00], &status(f, *o, p));
        }
        self
    }

    /// Mode selection: a `Start` of `<index> <announce>` answered by a `Result`
    /// carrying the mode the device settled on. Scripts both prompt values for
    /// every index in `0..slots`, so a caller can select any of them.
    fn selects(mut self, slots: u8) -> Self {
        for index in 0..slots {
            for announce in [0x00, 0x01] {
                let request = [0x1f, 0x03, 0x05, 0x02, index, announce];
                let reply = bytes(Addr::at(0x1f, 0x03), Operator::Result, &[index]);
                self.0 = self.0.on(&request, &reply);
            }
        }
        self
    }

    /// Accept one mode write: the exact SetGet payload `written`, answered by a
    /// Status carrying `stored` in read format — what the device echoes back.
    fn saves_mode(mut self, written: &[u8], stored: &[u8]) -> Self {
        let mut req = vec![0x1f, 0x06, 0x02, written.len() as u8];
        req.extend_from_slice(written);
        self.0 = self.0.on(&req, &status(0x1f, 0x06, stored));
        self
    }

    /// Mode reset: a `Start` of `<index>` at `1f 09`, answered by an empty
    /// `Result`, for every index in `0..slots`.
    fn resets(mut self, slots: u8) -> Self {
        for index in 0..slots {
            let request = [0x1f, 0x09, 0x05, 0x01, index];
            let reply = bytes(Addr::at(0x1f, 0x09), Operator::Result, &[]);
            self.0 = self.0.on(&request, &reply);
        }
        self
    }

    /// Exists, will not list itself. Five functions on the Ultra do this while
    /// holding data, so the surface must leave their opcodes unproven.
    fn opaque(mut self, f: u8) -> Self {
        self.0 = self.0.on(&[f, 0x01, 0x05, 0x00], &refusal(f, 0x01, 0x05));
        self.0 = self.0.on(&[f, 0x00, 0x01, 0x00], &status(f, 0x00, b"1.1.0"));
        self
    }

    fn refuses(mut self, f: u8, o: u8, code: u8) -> Self {
        self.0 = self.0.on(&[f, o, 0x01, 0x00], &refusal(f, o, code));
        self
    }

    /// Neither a reply nor a refusal — the third answer.
    fn silent(mut self, f: u8, o: u8) -> Self {
        self.0 = self.0.silent(&[f, o, 0x01, 0x00]);
        self
    }

    fn done(self) -> Scripted {
        self.0
    }
}

/// QuietComfort 35, `0x400c`, firmware 3.0.3, channel 8.
///
/// Eight functions. Noise cancelling at `01 06`; no equaliser, no immersive
/// audio, no modes. Battery is one byte.
pub fn qc35() -> Scripted {
    Profile::new()
        .opaque(0x00)
        .read(0x00, 0x03, &[0x40, 0x0c, 0x02])
        .read(0x00, 0x05, b"3.0.3")
        .read(0x00, 0x07, b"REDACTED-SERIAL")
        .refuses(0x00, 0x0f, 0x04)
        .lists(0x01, &[
            (0x02, b"\0qc35"),
            (0x03, &[0xa1, 0x00, 0x04, 0xc3, 0xde]),
            (0x04, &[0x3c]),
            (0x06, &[0x01, 0x0b]),
        ])
        // The equaliser opcode does not refuse here — it says nothing at all.
        .silent(0x01, 0x07)
        .writable(0x01, 0x06, &[0x01, 0x0b], &[&[0x03], &[0x00], &[0x01]])
        .writable(0x01, 0x02, b"\0qc35", &[b"renamed"])
        .lists(0x02, &[(0x02, &[0x46])])
        .opaque(0x04)
        .read(0x04, 0x09, &[0xaa, 0xbb, 0xcc, 0x00, 0x00, 0x01])
        .opaque(0x05)
        .writable(0x05, 0x05, &[0x19, 0x0e], &[&[0x05], &[0x18]])
        .refuses(0x05, 0x0f, 0x04)
        .done()
}

/// QuietComfort Ultra Headphones, `0x4066`, channel 1.
///
/// Noise cancelling moved to `01 05` and the old opcode refuses; function `1f`
/// carries the modes; battery is four bytes per cell.
pub fn ultra_hp() -> Scripted {
    Profile::new()
        .opaque(0x00)
        .read(0x00, 0x01, b"1.2.0")
        .read(0x00, 0x03, &[0x40, 0x66, 0x01])
        .read(0x00, 0x05, b"1.6.7+g6ebabd2")
        .read(0x00, 0x07, b"REDACTED-SERIAL")
        .read(0x00, 0x0f, b"Bose QC Ultra Headphones")
        .lists(0x01, &[
            (0x00, b"1.1.0"),
            (0x02, b"\0ultra-hp"),
            (0x03, &[0xe1, 0x00, 0x01, 0x81, 0x5e, 0x01, 0x01]),
            (0x04, &[0x05, 0x00, 0x00]),
            (0x05, &[0x0b, 0x0a, 0x03]),
            (0x07, &[0xf6, 0x0a, 0x02, 0x00, 0xf6, 0x0a, 0x00, 0x01, 0xf6, 0x0a, 0x02, 0x02]),
            (0x0a, &[0x01]),
            (0x18, &[0x00]),
            (0x1b, &[0x01]),
        ])
        .refuses(0x01, 0x06, 0x04)
        .writable(0x01, 0x0a, &[0x01], &[&[0x00], &[0x01]])
        .writable(0x01, 0x18, &[0x00], &[&[0x00], &[0x01]])
        .writable(0x01, 0x03, &[0xe1, 0x00, 0x01, 0x81, 0x5e, 0x01, 0x01], &[&[0x21, 0x01], &[0x26, 0x00]])
        .writable(0x01, 0x07, &[0xf6, 0x0a, 0x02, 0x00, 0xf6, 0x0a, 0x00, 0x01, 0xf6, 0x0a, 0x02, 0x02], &[&[0x06, 0x00], &[0x0a, 0x00]])
        .lists(0x02, &[(0x02, &[0x5a, 0xff, 0xff, 0x00])])
        .opaque(0x04)
        .opaque(0x05)
        .read(0x05, 0x0f, &[0x00])
        .opaque(0x07)
        .lists(0x1f, &modes())
        .writable(0x1f, 0x05, &[0x01], &[&[0x00], &[0x01]])
        .selects(10)
        // Accept the one write the mode-write test makes, echoing the slot with
        // wind block now on. The request bytes come from the real encoder, so
        // the fixture and the code under test cannot drift apart.
        .saves_mode(
            &crate::codec::set_mode(wind_test_config()),
            slot(9, Some(("WindTest", 0)), 0, 1),
        )
        .resets(10)
        .done()
}

/// Sport Earbuds, `0x402D`, channel 8. **No noise cancelling at all**, and it
/// says so: both opcodes refuse. Two battery cells.
pub fn sport() -> Scripted {
    Profile::new()
        .opaque(0x00)
        .read(0x00, 0x03, &[0x40, 0x2d, 0x02])
        .refuses(0x00, 0x0f, 0x04)
        .lists(0x01, &[
            (0x02, b"\0sport"),
            (0x03, &[0xa1, 0x00, 0x01, 0x81, 0x5e, 0x01, 0x01]),
        ])
        .refuses(0x01, 0x05, 0x04)
        .refuses(0x01, 0x06, 0x04)
        .lists(0x02, &[(0x02, &[0x64, 0xff, 0xff, 0x01, 0x64, 0xff, 0xff, 0x02])])
        .done()
}

/// Ten mode slots: five stored, five empty. Kaled's own, from the capture —
/// each prompt id is the one the real record carries.
fn modes() -> Vec<(u8, &'static [u8])> {
    const NAMES: [(&str, u8, u8, u8); 5] = [
        ("Quiet", 0x01, 0, 0),
        ("Aware", 0x02, 10, 0),
        ("Immersion", 0x22, 0, 0),
        ("Focus", 0x0d, 5, 0),
        ("Home", 0x0a, 8, 1),
    ];
    let mut out: Vec<(u8, &'static [u8])> = Vec::new();
    for (i, (name, prompt, awareness, wind)) in NAMES.iter().enumerate() {
        out.push((0x06, slot(i as u8, Some((name, *prompt)), *awareness, *wind)));
    }
    for i in 5..10u8 {
        out.push((0x06, slot(i, None, 0, 0)));
    }
    out.push((0x08, &[0x0a, 0x00, 0x1f]));
    out.push((0x03, &[0x00]));
    out.push((0x05, &[0x01]));
    out
}

/// The mode the write test builds and the Ultra fixture accepts — one place, so
/// they cannot disagree. A silent (prompt 0) mode in slot 9 with wind block on.
pub fn wind_test_config() -> crate::codec::ModeConfig {
    crate::codec::ModeConfig {
        index: 9,
        prompt: 0,
        name: "WindTest".to_string(),
        cnc_level: 0,
        auto_cnc: false,
        spatial: Some(crate::codec::Immersive::Off),
        wind_block: Some(true),
        anc_toggle: None,
    }
}

/// One 47-byte record. Byte 2 is the prompt — `0` marks an empty slot, since a
/// stored mode always names one. Byte 5 is favorite, byte 42 the cancellation
/// level, byte 46 wind block; none of those three marks occupancy.
fn slot(index: u8, mode: Option<(&str, u8)>, awareness: u8, wind: u8) -> &'static [u8] {
    let mut r = vec![0u8; 47];
    r[0] = index;
    if let Some((name, prompt)) = mode {
        r[2] = prompt;
        r[6..6 + name.len()].copy_from_slice(name.as_bytes());
        r[42] = awareness;
        r[46] = wind;
    }
    Box::leak(r.into_boxed_slice())
}
