# PRODUCT.md — fio

## What it is

fio is a small desktop Linux tool (Tauri 2 + SvelteKit frontend, Rust backend)
that creates and manages **virtual audio devices** on PipeWire/PulseAudio
(pipewire-pulse). Each virtual device the user creates exists as a sink named
`fio-{slug}` plus a microphone named `fio-{slug}.monitor`, so it appears in
ordinary system audio settings and app pickers.

## The mechanism

fio routes audio into and out of those devices by rules, not by hand:

- **App-name rules** `{ AppName }` match a live or future playback stream by
  its PulseAudio `application.name` (falling back to `media.name`) and force it
  onto a route target.
- **Capture-device rules** `{ CaptureDevice }` route a physical microphone
  (a source whose `monitor_of_sink` is null) to a target.
- **Monitor rules** `{ VirtualMonitor }` route one virtual device's `.monitor`
  source to a target — how a device's output is listened to, and how devices
  chain into each other.

Targets are `{ VirtualDevice }` (feed another fio sink) or `{ PhysicalSink }`
(send to real hardware).

## What the UI must make legible

- Which devices exist, and which audio feeds each one.
- What a device is monitored through (its monitor routes to real hardware).
- Whether a device's monitor is the system default mic.
- Drift: config no longer matching the live server (missing device, missing
  route end, missing loopback link), with a restore action.

## Constraints

- One window, Operate mode. Desktop Linux, system-ui typography.
- Backend contract is fixed: `invoke("ui_state")` and the six mutations;
  state refreshes on the `fio-state-changed` event.
- A device's own monitor must never be offered as its own source (no
  self-feedback).
- The app hides to a tray icon on close; "launch at login" (XDG autostart)
  is a footer setting.

## Brand commitments

- Name and mark: **fio**, lowercase.
- The pinned visual world is Rogue Amoeba's **Loopback** for macOS: left
  device sidebar, one large rounded device block on a light canvas, colored
  source blocks, bezier wires fanning into an Output Channels block, and a
  monitors strip. Light, airy, precise.
