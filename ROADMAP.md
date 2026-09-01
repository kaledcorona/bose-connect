# Roadmap

What is worth doing, in the order it pays. Captures are listed first: each
turns a guess into a record, which is where every other item draws from.

## Captures wanted

One capture of the official app doing each, against the reference's method.

- [ ] `01 05` write — setting cancellation on an Ultra. Never observed.
- [ ] `1f 03` — selecting a mode. The app's form is accepted and changes
      nothing; the real form is somewhere else.
- [ ] `01 09` read — the shortcut action's per-model layout.
- [ ] `05 0f` write — immersive audio off/still/motion.
- [ ] `01 04` on an Ultra — the three-byte auto-off.
- [ ] `01 03` on a QC35 — where the battery announcement reads back.

## Library

- [ ] Per-call receive timeout on `Rfcomm` (`set_timeout` public, or a
      `recv_timeout`). Gates background `poll`, and so live state everywhere.
- [ ] Decode notifications against the catalog once the payload shape is
      confirmed per model; until then `notices` hands back the raw record.
- [ ] bluez `PropertiesChanged` on `Connected`, so a client learns when the
      device appears rather than asking — and never probes while bluez is
      still bringing A2DP up. Observed: a probe or a rename racing that
      connect leaves PipeWire with a dead transport and no sound until the
      card profile is cycled.
- [ ] Publish on crates.io; the GUI then drops its path dependency.

## CLI

- [ ] `watch` — print notifications as they arrive. Needs the timeout above.
- [ ] `devices --json`.

## GUI

- [ ] Live state: background `poll` once the library has a short idle timeout.
- [ ] Open the device page itself when the headphones connect.
- [ ] Tray: battery in the panel, ANC in the menu. `ksni` for sway/KDE;
      GNOME needs an extension, which is why this is after the window.
- [ ] Low-battery notification through the portal.
- [ ] Flathub: generate `cargo-sources.json`, build the manifest, submit.
- [ ] "Share with…": a PipeWire combine sink over two A2DP sinks. Host-side,
      nothing to do with the protocol; ±30 ms between links, fine for a room.

## Out of scope, and why

- **Improving immersive audio.** It is head-tracked spatialisation in the
  headphone's DSP; the protocol only toggles it. The better-than-Bose path is
  host-side — a PipeWire filter-chain doing binaural rendering on the
  computer, on every model, under your control.
- **Custom firmware.** Qualcomm QCC/CSR silicon, NDA'd SDK, signed update
  image over channel 24, and a failed flash is a brick. Everything firmware
  could do for audio is doable on the host at no risk.
