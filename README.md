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

## Install

    cargo install --path .                     # into ~/.cargo/bin
    cargo install --path . --features bluez    # + the `devices` subcommand

Or build in place with `cargo build --release` and run
`target/release/bose-connect`. Either way `bluez` is needed at runtime, and the
headphones paired first with `bluetoothctl`. The `bluez` feature adds
`bose-connect devices` and the D-Bus stack it needs, so it is off by default.

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
    $ bose-connect $ULTRA mode 3        # switch to it

Creating and editing a mode — its name, cancellation, wind block, spatial audio
and spoken prompt — is a library call (`save_mode`); a libadwaita front-end puts
it behind a form.

The channel probe runs once per device, four seconds a channel, and the
answer is kept under `$XDG_CACHE_HOME/bose-connect/`. To bypass both:

    $ bose-connect --channel 8 $QC35 battery

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
AUTO_AWARE: bool = (0x01, 0x1d) "auto aware mode"
    read  Confirmed, codec::flag,
    write Unknown,   None,
    note  "drops the mode to transparency on its own";
```

That is the whole change. The surface probe picks it up, `bose-connect catalog`
lists it, `cargo doc` documents it, and `dev.get(&AUTO_AWARE)` works. A name in
`src/api.rs` is optional and one line:

```rust
pub fn auto_aware(&mut self) -> Result<bool> { self.get(&AUTO_AWARE) }
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

So a write the crate will not make is not a special case in the code — it is a
table entry, and its reason string is the error the user reads.

Mode selection is the case that earned this its keep. The app's captured form sat
as `Ineffective` — refused rather than sent — until the real form, a `Start`
carrying the index and prompt, was confirmed against a device. It is a working
verb now. The one write still refused is setting cancellation on an Ultra: its
`01 05` format was never captured, so the catalog marks it `Unknown` and
`dev.set` returns the reason rather than send a guess.

That encodes the reference's own lesson: **a capture establishes the syntax,
not the semantics.**

## Finding the address

    $ bose-connect devices
    AA:BB:CC:00:00:01  009e:4066  Bose QC Ultra Headphones     connected

Reads what bluez has paired; pairing itself stays with `bluetoothctl`. Needs
`--features bluez`, which brings a D-Bus stack the rest of the crate does not
need, so it is off by default.

## Exploring

Three commands need no per-record code, so they reach the parts the catalog has
not named yet:

    $ bose-connect catalog             # every record this build knows
    $ bose-connect $MAC raw 1f 03      # read one address, decoded by nobody
    $ bose-connect $MAC scan           # which functions answer, 00-3f

Addresses are hex, as `catalog` prints them; quantities are decimal.

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

## As a library

Two verbs carry everything — `get` and `set` — and a named accessor is one line
over them. `Transport` is a trait, so the protocol drives from recorded traffic
as readily as from a socket, which is how the tests cover three device models
with none present.

The **[library guide](https://github.com/kaledcorona/bose-connect/wiki/Library)**
covers the eight layers, the fixtures, and the three-answer error model — a
request gets a value, a refusal, or silence, and the three are not the same.

## Ultra cancellation is a mode, not a dial

The QuietComfort 35 sets a raw cancellation level; the Ultra does not. `01 05`
refuses a write — *operator not supported* — so there is no level to set
directly. On the Ultra, cancellation lives in a **mode**: select one, or edit a
mode's level and select it. Both work, and that is how the app does it too.

That is the crate's rule throughout. Where the device refuses, or a write has
never been shown to take, it says so with the reason rather than offer a control
that silently does nothing.

## Tests

    cargo test        # 99, none need hardware

## Roadmap

See `ROADMAP.md`. Captures wanted are listed first; each is one catalog entry away from a feature.

## Licence

MIT. The protocol is not covered — opcodes and byte layouts are facts.
