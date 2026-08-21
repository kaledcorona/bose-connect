# bose-connect

Control Bose headphones from Linux.

Implements the protocol described in
[bose-rfcomm](https://github.com/kaledcorona/bose-rfcomm). Tested on a
QuietComfort 35 and QuietComfort Ultra Headphones.

## Why

Bose ships no Linux app, and drops older models from the one it does ship. The
three existing tools each hardcode a table of device ids and a fixed RFCOMM
channel, so they are wrong about any model their author did not own.

This asks the device instead. Same code path for a 2016 QC35 and a 2023 Ultra,
with the differences handled rather than assumed.

## Build

    cargo build --release

Needs `bluez`. Pair the headphones first with `bluetoothctl`.

## Examples

    $ bose-connect AA:BB:CC:00:00:01 info
    channel   1
    id        0x4066 index 1
    model     Bose QC Ultra Headphones
    version   1.6.7+g6ebabd2
    serial    REDACTED-SERIAL
    supports  anc:graded eq:true immersive:true modes:true
    functions 00 01 02 04 05 07 1f
    immersive off

Two models, same command, different answers:

    $ bose-connect $ULTRA anc
    cancelling 10 of 10  (awareness 0)

    $ bose-connect $QC35 anc
    high  (accepts off, high, low)

The QC35 has three named levels; the Ultra has a scale. The library reads each
from the opcode that model uses, and the accepted levels come from the device's
own bitmask.

    $ bose-connect $ULTRA eq
    band 0    0  [-10..10]
    band 1    0  [-10..10]
    band 2    0  [-10..10]

    $ bose-connect $ULTRA eq 0 6      # band 0 to +6

Ranges come from the device, so a client can draw the control without knowing
the model.

    $ bose-connect $QC35 battery
    cell 0  70%

    $ bose-connect $ULTRA battery
    cell 1  100%
    cell 2  100%

    $ bose-connect $QC35 volume
    14 of 24
    $ bose-connect $QC35 volume 8

    $ bose-connect $QC35 auto-off
    60 min
    $ bose-connect $QC35 auto-off never

    $ bose-connect $ULTRA modes
    0  Quiet
    1  Aware
    2  Immersion
    3  Focus
    4  Home

Asking for something a model lacks says so, rather than timing out:

    $ bose-connect $QC35 eq
    bose-connect: 01 07: this model does not have it

and says it without a round trip: the QC35's function `01` enumerates and does
not mention `07`, where probing it directly would wait out the whole receive
timeout on an opcode that answers nothing.

## Adding a finding

The protocol is still being discovered. A new record is **one entry**, in
`src/catalog.rs`, in the same shape as the reference's own tables:

```rust
WIND_BLOCK: bool = (0x1f, 0x0b) "wind block"
    read  Confirmed, codec::flag,
    write Unknown,   None,
    note  "forces cancellation to maximum while on";
```

That is the whole change. The surface probe picks it up, `bose-connect catalog`
lists it, `cargo doc` documents it, and `dev.get(&WIND_BLOCK)` works. A name in
`src/api.rs` is optional and one line:

```rust
pub fn wind_block(&mut self) -> Result<bool> { self.get(&WIND_BLOCK) }
```

Add the bytes you observed to the matching device in `src/fixtures.rs` and the
test suite covers it without hardware.

### Evidence is part of the record

`read` and `write` each carry how well that direction is understood, and only
`Confirmed` is writable:

| | |
|---|---|
| `Confirmed` | a write changed what a read returned |
| `Syntax("…")` | seen on the wire; the effect was never verified |
| `Ineffective("…")` | the form is accepted and changes nothing |
| `Unknown` | no format |

So the two operations this crate refuses are not special cases in the code —
they are table entries, and the string is the error the user reads:

    $ bose-connect $ULTRA mode 1
    bose-connect: 1f 03: the app's form is accepted and changes nothing, for every index

That encodes the reference's own lesson: **a capture establishes the syntax,
not the semantics.**

## Exploring

Three commands need no per-record code, so they reach the parts the catalog has
not named yet:

    $ bose-connect $MAC catalog        # every record this build knows
    $ bose-connect $MAC raw 07 01      # read one address, decoded by nobody
    $ bose-connect $MAC scan           # which functions answer, 00-3f

`scan` defaults to `00`–`3f` deliberately. Every sweep in the reference stopped
at `0x0f`, which is why the mode table at `0x1f` went unfound through
twenty-nine labelled observations.

## From Python, or anything else

`json` prints the identity and capabilities:

    $ bose-connect AA:BB:CC:00:00:01 json
    {
      "channel": 1,
      "id": "0x4066",
      "model": "Bose QC Ultra Headphones",
      "version": "1.6.7+g6ebabd2",
      "serial": "REDACTED-SERIAL",
      "anc": "graded",
      "equaliser": true,
      "immersive": true,
      "modes": true,
      "battery": [{"cell": 0, "percent": 80}]
    }

So a status bar, a script, or a tray applet needs no bindings:

```python
import json, subprocess

def bose(mac, *args):
    return subprocess.run(["bose-connect", mac, *args],
                          capture_output=True, text=True, check=True).stdout

info = json.loads(bose(MAC, "json"))
if info["battery"]:
    print(f"{info['battery'][0]['percent']}%")

if info["anc"] == "named":
    bose(MAC, "anc", "low")
```

## As a Rust library

Two verbs carry everything. Named accessors are one line over them.

```rust
use bose_connect::api::Anc;
use bose_connect::catalog::{BATTERY, VOLUME};
use bose_connect::codec::Level;
use bose_connect::device::Device;
use bose_connect::transport::{probe_channel, Address};

let addr: Address = "AA:BB:CC:00:00:01".parse()?;
let (_channel, session) = probe_channel(addr, TIMEOUT)?;
let mut dev = Device::open(session)?;

// By name, or straight from the catalog — the same code path.
let cells = dev.battery()?;
let vol   = dev.get(&VOLUME)?;
dev.set(&VOLUME, 8)?;

match dev.noise_cancelling()? {
    Anc::Named(a) if a.accepts(Level::Low) => dev.set_level(Level::Low)?,
    Anc::Named(_)  => {}
    Anc::Graded(a) => println!("cancelling {} of {}", a.cancelling(), a.top()),
}
```

### Layers

One direction, each knowing only the one below:

    transport   bytes
    session     bytes → records, one exchange
    wire        records ↔ payloads
    codec       payloads ↔ values
    catalog     which value lives where, and how well we know it
    surface     which of those this device answers
    device      get / set — the only two verbs
    api         names for the verbs

`Transport` is a trait, so the protocol can be driven from recorded traffic
instead of a socket. `src/fixtures.rs` holds four devices from two generations
as data, which is how the tests cover both without either device present. Build
with `--features mock` to use them from your own tests.

### Three answers, not two

A request gets a reply, a refusal, or **nothing at all**, and the three are
different:

| behaviour | `Error` |
|---|---|
| a value | — |
| `04` + `03` / `04` | `Refused`, or `Absent` if the surface knew already |
| silence | `Silent` |

Reading silence as "busy" invites a retry loop against an opcode that will
never answer; reading it as "unsupported" throws away the refusal codes, which
are the only signal for whether a function is worth probing further.

## Limits

**Setting cancellation on an Ultra.** No capture of the official app writing
`01 05` exists, so the format would be a guess.

**Selecting a mode.** The form the app sends is accepted by the device and
changes nothing, for every index. Reading the active mode works.

Both are in the catalog with their reasons. Offering a control that silently
does nothing would be worse than not offering it.

## Tests

    cargo test        # 72, none need hardware

## Licence

MIT. The protocol is not covered — opcodes and byte layouts are facts.
