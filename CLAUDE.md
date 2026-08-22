# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A Rust client for the Bose RFCOMM control protocol. The protocol description it
implements lives at <https://github.com/kaledcorona/bose-rfcomm> ("the
reference"); where the two disagree, the reference wins. Edition 2024, `libc`
the only dependency.

## Commands

    cargo test                                   # 85 tests, none need hardware
    cargo test wire::tests::encodes_a_read       # one test, by module path
    cargo test catalog::                         # one module
    cargo build --release
    cargo doc --open                             # the catalog documents itself

Tests are inline `#[cfg(test)] mod tests` per module — there is no `tests/`
directory, and adding one is not the convention here. `--features mock` exports
`fixtures` and the mock transports to downstream crates. The binary's tests
reach them through the self dev-dependency in `Cargo.toml`, since `cfg(test)`
on the lib does not apply to a bin target.

Running against hardware needs `bluez` and the headphones paired
(`bluetoothctl`): `cargo run -- <MAC> info`.

## Layers

One direction, each knowing only the one below. A change that makes a lower
layer reach upward is the wrong change.

    transport   bytes                                    src/transport/
    session     bytes → records, one exchange            src/session.rs
    wire        records ↔ payloads                       src/wire.rs
    codec       payloads ↔ values                        src/codec.rs
    catalog     which value lives where, how well known  src/catalog.rs
    surface     which of those this device answers       src/surface.rs
    device      get / set — the only two verbs           src/device.rs
    api         names for the verbs                      src/api.rs

`Device::get`/`set` carry every access. What differs between records lives in
the catalog; what differs between models lives in the surface. Neither should
become a match arm in `device.rs`, and `api.rs` entries are one line over `get`
unless the protocol imposes something the caller should not have to know.

## Adding a finding

One `catalog!` entry in `src/catalog.rs` — address, label, codec, evidence,
note. The macro emits both the `pub const Field` and the flat `CATALOG` slice
from the same text, so the surface probe, the `catalog` and `raw` commands and
rustdoc all pick it up with no further edits. Then add the observed bytes to the
matching device in `src/fixtures.rs`; the suite then covers it without hardware.
A name in `src/api.rs` is optional.

Catalog notes are copied from the reference verbatim rather than paraphrased, so
the two can be diffed. They carry raw byte layouts, hence the
`allow(rustdoc::invalid_html_tags)` at the top of the module — do not backtick
them, the same strings are a terminal table in the `catalog` command.

## Invariants worth not breaking

**Evidence gates writes.** `Evidence::Confirmed` means a write was seen to change
what a read returned. `Device::set` requires both confirmed evidence *and* an
encoder, so a format captured but never verified cannot be sent by accident. The
two operations this crate refuses — writing ANC on an Ultra, selecting a mode —
are catalog entries carrying their reason string, not special cases in code. The
reference's lesson made structural: **a capture establishes the syntax, not the
semantics.**

**Three answers, not two.** A value, a refusal, or silence. `Error::Refused`
(with the `Refusal` code), `Error::Silent`, `Error::Absent` when the surface
already knew. Collapsing silence into "unsupported" throws away the refusal
codes, which are the only signal for whether a function is worth probing;
collapsing it into "busy" invites a retry loop against an opcode that never
answers.

**Capability comes from the device.** `Surface::discover` runs one enumeration
per function the catalog mentions. Only refusal `03` (function absent) writes a
function off; every other refusal means it exists and will not list itself, so
its opcodes stay `Unproven` and the first read settles them. Never reintroduce a
device-id table — being wrong about unowned models is the failure mode this
crate exists to avoid.

**`probe_channel` skips channel 24** (`SRfcomm`, silent, most plausibly firmware
update) and returns the *open session*, not the channel number: closing and
reconnecting races the kernel releasing the channel and surfaces as `EBUSY`.
`EHOSTDOWN`/`EHOSTUNREACH`/`ETIMEDOUT` end the scan rather than costing thirty
timeouts. Scan once and keep the answer.

**RFCOMM is a stream.** `Session` holds a `pending` buffer: replies split across
reads and two replies in one read both happen. Mock transports imitate this —
they hand over what fits and keep the tail, and answer a scripted reply once
rather than forever (a repeating double turns any read-until-terminator loop
into a spin).

**Addresses reverse for the kernel** (`transport::rfcomm::bdaddr`), the opposite
of how bluez prints them. Getting it backwards connects to nothing, silently.

**`scan` defaults to `00`–`3f`.** Every sweep in the reference stopped at `0x0f`,
which is why the mode table at `0x1f` went unfound through twenty-nine labelled
observations.

## CLI

`src/bin/bose-connect.rs` settles everything the arguments can say in `parse`
before a socket opens — the probe costs up to four seconds a channel, so a typo
must fail first. `run` is generic over `Transport`, so output is tested against
the fixtures. Addresses (`raw`, `scan`) are hex; quantities are decimal. Exit 2
for usage, 1 for a device error.

## Testing style

`Scripted` (by request, unscripted requests refuse with `03`) for anything
driving the surface probe; `Replay` (by turn) only for short fixed exchanges.
`src/fixtures.rs` holds three devices across two generations — `qc35`,
`ultra_hp`, `sport` — which is how both generations are covered without owning
both. (The module doc still says four.) Test
names are sentences stating the invariant, and the comment above an assertion
says why the case exists, usually citing the observation that produced it.
