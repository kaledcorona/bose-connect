# bose-connect

A Rust client for the RFCOMM control protocol used by Bose headphones.

Implements [bose-rfcomm](https://github.com/kaledcorona/bose-rfcomm), which
documents the protocol and how each value was established. Where the two
disagree, that description is the one backed by observation.

Tested against a QuietComfort 35 (`0x400c`) and QuietComfort Ultra Headphones
(`0x4066`).

## Use

    cargo build --release
    bose-connect AA:BB:CC:00:00:01 info

    channel   1
    id        0x4066 index 1
    model     Bose QC Ultra Headphones
    supports  anc:graded eq:true immersive:true modes:true

Then `anc`, `eq`, `modes`, `battery`, `volume`, `name`, `auto-off`, `toggle`.
Each reads with no argument and writes with one. Run it bare for the list.

The device must already be connected — pair it with `bluetoothctl` first.

## As a library

```rust
use bose_connect::device::{Anc, Device};
use bose_connect::transport::{probe_channel, Address};

let addr: Address = "AA:BB:CC:00:00:01".parse()?;
let (_channel, session) = probe_channel(addr, TIMEOUT).ok_or(/* … */)?;
let mut dev = Device::open(session)?;

match dev.capabilities.anc {
    Anc::Legacy => dev.set_level(Level::Low)?,   // named levels
    Anc::Modern => { /* graded; see below */ }
    Anc::Absent => {}
}
```

## What it knows that a caller should not have to

**The channel differs per model** — 8 on a QuietComfort 35, 1 on the Ultra — and
neither SDP nor the device id predicts it. `probe_channel` asks, and hands back
the open session rather than the number: closing and reconnecting races the
kernel releasing the channel and surfaces as `EBUSY`.

**Noise cancelling is not one setting.** A QuietComfort 35 keeps it at `01 06`
with three named levels and a bitmask saying which it accepts. The Ultra keeps
it at `01 05`, refuses the old opcode, and counts **awareness** rather than
cancellation — `0` is maximum cancelling. `AncState` is two variants, not one
with a gap.

**Silence is not refusal.** A device can answer nothing at all, and that is a
third case. Reading it as "busy" invites a retry loop against an opcode that
will never answer; reading it as "unsupported" discards the refusal codes, which
say whether a function is worth probing further.

**Enumerations do not fit one read.** The mode table is 555 bytes against a much
smaller MTU, and a single `recv` returns a prefix that may parse cleanly and
omit most of the answer.

## What it refuses to do

Two operations are implemented as errors, on purpose:

`set_cancelling` on an Ultra. No capture of the app writing `01 05` exists, so
the format would be a guess, and a blind write to an unidentified opcode is the
one move this protocol does not forgive.

`select_mode`. The form the app sends is accepted by the device and changes
nothing, for every index. The capture established the syntax and was taken as
establishing the semantics, which it does not.

Offering either would be a control that silently does nothing.

## Testing

    cargo test

Thirty-six tests, none of which need hardware. `Transport` is a trait, and the
recorded exchanges from two generations exercise paths that would otherwise need
both devices in hand.

Two bugs reached hardware anyway, and both are the kind a mock cannot catch:
a `sockaddr_rc` declared `packed` is nine bytes where the kernel wants ten, and
probing a channel then reconnecting loses a race with the kernel.

## Licence

MIT. See `LICENSE`.

The protocol is not covered. Opcodes and byte layouts are facts, not expression.
