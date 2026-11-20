# fio

**Loopback for Linux** — a virtual audio patchbay for PipeWire and PulseAudio.

fio creates virtual audio devices that show up in your system mixer like real
hardware, then wires app output, microphones, and device monitors into them by
rules — so "send Discord and the game to the stream mix" or "hear a virtual
speaker's output on my headphones" is a saved configuration, not a `pactl`
incantation you re-run every boot.

## Requirements

- **Linux** with a PulseAudio-compatible audio server — in practice
  **PipeWire with `pipewire-pulse`** (the default on Fedora, Arch, Ubuntu 22.10+).
  Real PulseAudio works too.
- Build deps: Node.js + npm, Rust toolchain (`cargo`), `libpulse`,
  `webkit2gtk-4.1`, `gtk3`.

## Run it

```sh
npm install
npx tauri dev          # dev build, hot-reloads the frontend
npx tauri build        # release build + AppImage/deb bundles
```

(`cargo tauri build` works the same if you have `cargo-tauri` installed; the npm
CLI is what's vendored in `package.json`.)

Bundles land in `src-tauri/target/release/bundle/` (`appimage/`, `deb/`). On
Arch there's a `PKGBUILD` at the repo root that builds from source and installs
`/usr/bin/fio` + a `.desktop` entry.

## What it does

- **Virtual devices.** Each device is a real sink named `fio-{name}` with a
  microphone end `fio-{name}.monitor` — it appears in GNOME/KDE audio settings,
  Discord's mic picker, OBS sources, anywhere a device can be selected.
- **App routing.** Rules match a playback stream by its PulseAudio
  `application.name` and force it onto a virtual device — enforced on live
  streams now and on matching streams that appear later.
- **Mic → output loopback.** Route a physical capture device into a virtual
  device, or a virtual device's monitor onto real hardware (this is how a
  virtual speaker gets listened to) — or into another fio device, chaining them.
- **Default mic.** One click makes a device's `.monitor` the system default
  source, so it behaves like a virtual microphone everywhere.
- **Tray + autostart.** Closing the window hides to the system tray; a
  "launch at login" toggle writes the XDG autostart entry.
- **Drift detection.** If the live server no longer matches your saved config
  (device unloaded, route end missing, link gone), the UI shows what drifted
  and offers a one-click restore.

## The `pactl` equivalent

Everything fio does is standard PulseAudio-module plumbing; it just does it
persistently, atomically, and from a UI. The manual equivalent of each feature:

| fio action | `pactl` equivalent |
| --- | --- |
| Create device "music" | `pactl load-module module-null-sink sink_name=fio-music sink_properties=device.description='music'` |
| Send app's playback to a device | `pactl list sink-inputs` → `pactl move-sink-input <idx> fio-music` |
| Mic → device | `pactl load-module module-loopback source=<mic> sink=fio-music source_dont_move=1 sink_dont_move=1` |
| Listen to a device on hardware | `pactl load-module module-loopback source=fio-music.monitor sink=<hw-sink> source_dont_move=1 sink_dont_move=1` |
| Chain device B into device A | `pactl load-module module-loopback source=fio-b.monitor sink=fio-a` |
| Use device as default mic | `pactl set-default-source fio-music.monitor` |
| Delete a device | `pactl list modules short` → `pactl unload-module <idx>` (after moving its streams off) |

The difference: fio tracks desired state, reloads it at startup, enforces app
rules on new streams as they appear, tears links down without orphaning loaded
modules, and rescues stranded streams to a safe sink when a device is deleted.

## Architecture

fio talks to the audio server through the **PulseAudio compatibility API**
(`libpulse-binding` in Rust) — on modern systems that's pipewire-pulse, so no
PipeWire-specific code is needed and devices compose with every Pulse-aware
app on the system.

The backend is split deliberately:

- **`audio/`** — a thin `PulseEngine` wrapper over the threaded mainloop:
  introspection (sinks, sources, streams, modules), module load/unload,
  stream moves, server defaults.
- **`model/`** — the persisted desired state (`~/.config/fio/state.json`):
  devices, routes, seen app names.
- **`reconcile/`** — a *pure* planning layer. Given desired state + a live
  server snapshot it computes `Action`s (load null-sink, move stream, load
  loopback link) and detects drift. No I/O in the planners — the `Driver`
  executes actions and holds live bookkeeping (stream rescues).
- **`commands/`** — the Tauri command surface (`ui_state` + six mutations);
  each mutating command delegates to the `Manager`, then emits
  `fio-state-changed` so the Svelte frontend re-queries.

Links between endpoints are carried by **`module-loopback`** instances pinned
at both ends (`source_dont_move`/`sink_dont_move`), identified in the server by
their `source=`/`sink=` argument pair.

The frontend is SvelteKit (static adapter) + TypeScript — a device-centric
wiring diagram styled after Loopback: sidebar of devices, colored source
blocks, bezier wires into an output-channels block, and a monitors strip.

## Screenshots

_Screenshots pending — drop captures in `docs/screenshots/` and link them here._

<!-- ![fio main window](docs/screenshots/main.png) -->

## Security notes

- The webview runs under a tightened CSP (`default-src 'self'` + Tauri's IPC
  endpoints + `'unsafe-inline'` styles for Svelte's inline style attributes);
  Tauri hashes the bundled bootstrap script at build time.
- `npm audit` currently reports **3 vulnerabilities (2 low, 1 high)**, all in
  the SvelteKit dev-toolchain chain (`@sveltejs/kit`, `cookie`). They affect
  dev-server code paths, not the shipped Tauri bundle — the fix requires a
  breaking `@sveltejs/kit@3` major upgrade and is deferred.

## Roadmap

- **Native PipeWire nodes** via the PipeWire API (virtual devices without the
  Pulse-compat layer).
- **Per-source volume** faders on each wire.
- **Effects** — EQ/noise suppression chains on device inputs.
- Flatpak is a non-goal for now: the sandbox blocks the PulseAudio socket.

## License

MIT
