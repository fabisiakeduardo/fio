# Fio — Loopback-style virtual audio devices for Linux

Working name: **Fio** (trocadilho com PipeWire; name is changeable, not load-bearing).

## Intent and why

A Linux equivalent of Rogue Amoeba's Loopback for macOS: a GUI app to create
virtual audio devices and route audio between apps and physical devices, without
touching `pactl`/terminal plumbing. Eduardo's portfolio piece — it should be a
real, shippable tool that demos clean systems work, not a toy.

Today on Linux this workflow exists only as manual PipeWire/PulseAudio module
incantations (null-sinks, remaps, loopbacks) or graph editors like qpwgraph that
expose the raw graph rather than a device-oriented model. Fio wraps the common
Loopback patterns in a device-oriented UI that lives in the tray.

## Goals

- Create/remove named virtual audio devices. Each virtual device appears both as
  an output ("speaker") and an input ("microphone") to every app on the system —
  Discord, OBS, browsers — because that's what a PipeWire null-sink + monitor
  source already is.
- Route **application audio** into a virtual device (one or many apps can feed
  the same device — the Loopback signature use case: game + music → one virtual
  mic for voice chat).
- Route **physical capture devices** (real mics) into virtual devices or
  physical outputs — the "hear my mic in this channel" pass-through Eduardo
  specified, implemented via `module-loopback` instances.
- Route virtual devices to physical outputs (monitoring: hear what the virtual
  mic is carrying) — also `module-loopback`, from the sink's monitor source.
- Enforce routes over time: when a routed app starts a new stream, it gets moved
  automatically (event subscription + saved rules).
- Run in the background: system tray icon, window hides instead of quitting,
  optional autostart, devices+routes restored on launch.

## Non-goals (v1)

- Audio processing/effects (EQ, compression, mixing levels per-source inside a
  virtual device).
- Arbitrary graph editing (that's qpwgraph/Helvum's job — explicitly out of
  scope; Fio is device-oriented, not graph-oriented).
- JACK support, native PipeWire node creation, per-channel routing.
- Windows/macOS support.

## Anti-goals

- Must never silently break the user's audio: Fio only moves streams it has
  rules for; quitting the app leaves devices loaded but stops enforcement —
  nothing the user didn't ask for gets rerouted.
- No state drift surprises: if something outside Fio removes a module we loaded,
  the UI shows drift rather than pretending the device exists.

## Constraints

- Linux only. Requires **PipeWire with `pipewire-pulse`** (the standard on
  modern distros; verified on the dev machine: PipeWire 1.6.11, pipewire-pulse
  present). No PulseAudio-only support needed but the code happens to work there.
- Tauri v2 requires webkit2gtk-4.1 + gtk3 (present on dev machine).
- Stack fixed by Eduardo: **Rust + Tauri**, runs in background.

## Approaches

Three ways to drive the audio layer were considered:

- **A — PulseAudio client API via `libpulse-binding`** (chosen). Speaks to the
  server PipeWire exposes via `pipewire-pulse`. Covers everything v1 needs:
  loading/unloading modules, introspection (sinks, sources, sink-inputs,
  source-outputs, modules), moving streams, event subscription. Mature API,
  documented, and virtual devices show up natively system-wide.
- **B — Native PipeWire via the `pipewire` crate.** Full power (custom nodes,
  filter chains) but low-level bindings, manual mainloop management, and far
  more complexity than v1 needs. Remains the migration path if we ever need
  PipeWire-exclusive features; the `audio` module is a boundary that keeps this
  option open.
- **C — Shell out to `pactl`/`pw-cli` and parse output.** Zero deps, fragile,
  no event subscription. Rejected.

Chosen: **A**, for coverage-to-complexity ratio.

## Technical design

### Pulse/PipeWire primitives → Fio concepts

| Fio concept | Implementation |
|---|---|
| Virtual device | `module-null-sink` with `sink_name`, `source_name`, and `sink_properties`/`source_properties` `device.description` set to the device name. The monitor source IS the virtual mic — one module = both ends, matching Loopback's model. |
| App → device | `move_sink_input_by_index` for live streams; a saved rule keyed on the stream's `application.name`/`media.name` re-applied on `NEW` sink-input events. |
| Physical capture → target | `module-loopback source=<src> sink=<dst>` (with `latency_msec`, `dont_move` flags as needed). Each loaded instance is one link; N links = N module instances. |
| Device → physical output (monitor) | `module-loopback` from `<vdev sink>.monitor` to the output sink. Same mechanism, different endpoints. |
| Per-device volume/mute | `set_sink_volume_by_name` / `set_sink_mute_by_name` (stretch goal, cheap to add). |

### Architecture

Single Tauri v2 app; `src-tauri` organized as:

- `audio/` — the engine boundary. Wraps `libpulse-binding`: one mainloop thread,
  a `PulseEngine` handling introspection queries, module load/unload, stream
  moves, and an event subscription stream (new/removed sink-inputs, module
  changes). Everything Pulse-specific lives behind this module so approach B
  stays possible later.
- `model/` — `VirtualDevice`, `Route` (source = `AppStream { app_name }` |
  `CaptureDevice { node_name }` | `VirtualMonitor { device_id }`, target =
  device or physical sink), persisted desired-state.
- `reconcile/` — desired-state engine. Config = what should exist; on startup,
  on module/stream events, compare actual Pulse state to desired state and act
  (load missing modules, move matching streams). Reports drift it can't fix.
- `tray.rs`, `commands.rs` — Tauri commands bridging UI ↔ engine, tray menu.
- UI — patchbay-style three columns: sources (apps producing audio + physical
  inputs) → virtual devices → physical outputs. Assignment via drag or
  dropdown — builder's call. Frontend framework: **Svelte** (my call —
  lightest idiomatic Tauri pairing; Eduardo can veto).

### Enforcement model (reconcile)

The rule that decides everything: **Fio enforces, never fights.**

- **On app launch:** reconcile heals. Adopt surviving modules by
  `sink_name`/`source_name` (state.json stores names + stable ids, never module
  indices — indices are per-session), reload anything missing, re-apply rules
  to currently-live streams.
- **Mid-session:** enforcement fires only on `NEW` sink-input events (a rule
  matches → the stream moves). Change/remove events never trigger corrective
  action — if the user re-routes a stream in pavucontrol, that choice stands
  until the stream restarts.
- **Drift:** if something outside Fio unloads a module or link we own, the UI
  flags the device/route as broken and offers a "restore" action; Fio does not
  auto-reload mid-session (a user running `pactl unload-module` means it).
- **Deleting a virtual device:** first move its live sink-inputs back to the
  sink they came from (Fio records previous-sink per moved stream), then unload
  the module. If origin is unknown, move to the default sink. Never unload a
  sink with streams still attached.
- **Deleting a route:** stop enforcing; leave the already-moved stream where it
  is (it's still playing fine). Same for removing a loopback link — unload the
  module, audio stops on that path only.

### Route rules and app identity

- A rule's key is the stream's `application.name` property, exact match,
  falling back to `media.name` when `application.name` is absent.
- Rules can be created **before** an app produces audio — this is required for
  the flagship flow (pre-route the game's audio, then start the game). The
  source picker offers: live streams, recently-seen app names (Fio keeps a
  rolling history of `application.name` values it has observed), and free-text
  entry.
- Per-tab/per-window granularity (e.g. one Firefox tab but not another) is
  impossible at this API level — non-goal.

### Capture-side control

Loopback's own model: you pick the virtual mic inside the calling app. Fio v1:

- **Includes** a "use as default mic" button per virtual device
  (`set_default_source`) — one click replaces the Discord-settings trip for
  apps that follow the system default.
- **Does not** move live source-outputs (streams already recording). Rare
  need, extra API surface — non-goal for v1.

### Packaging

v1 ships as source + PKGBUILD (dev machine is Arch) and/or AppImage. Flatpak
is a **non-goal**: its sandbox blocks the PulseAudio socket and module loading
without `--socket=pulseaudio` finish-args, which defeats the audio layer's
assumptions. Revisit only if distribution demand appears.

### Persistence

Desired state in `~/.config/fio/state.json` (schema-versioned): virtual devices
and routes. On launch, reconcile recreates modules and re-applies routes.
Autostart via XDG `.desktop` file in `~/.config/autostart/`, toggled from
settings.

### Background behavior

- Close button hides window to tray; explicit Quit from tray menu exits.
- Tray menu: open window, quick list of virtual devices, quit.
- On quit: loaded PipeWire modules are left in place (they die with the
  PipeWire session anyway) — devices keep working, route enforcement stops.
  This is a deliberate choice, documented in-app.

### Known risks / assumptions

- **Wireplumber fights:** wireplumber's restore-stream policy may move streams
  back to previous sinks. Mitigation: our subscription enforces rules on new
  streams; remaining edge cases documented as known-issue for v1.
- **App identity is fuzzy:** `application.name` matching is heuristic (a browser
  shows one stream per tab, all named "Firefox" or similar). v1 routes by app
  name; per-tab granularity is out of scope.
- **Latency:** `module-loopback` adds configurable latency (default fine for
  voice; exposed in settings if needed).

## What's left to the builder

- Exact UI layout/visual design, frontend framework details.
- Error surfacing in UI (toast vs inline), copy.
- Whether per-device volume ships in v1 or immediately after.
- Module parameter tuning (rates, latency defaults).
