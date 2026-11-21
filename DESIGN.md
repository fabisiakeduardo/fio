# DESIGN.md — fio

The visual world is **Loopback (Rogue Amoeba) adapted to a desktop Linux
tool**: a device-centric wiring diagram, light and precise. Pinned by the
redesign brief; documented here from what was built.

## Structure

- **Left rail** (`DeviceSidebar`): brand mark, "Devices" list with a `+`
  affordance that opens an inline name row; rows are buttons with a device
  glyph and `fio-{slug}` node name; delete is a hover `×` that becomes a
  two-step inline confirm. A stats line sits at the rail's foot.
- **Canvas** (`DeviceCanvas`): the selected device as one large white
  rounded block — name + `fio-{slug}` / `.monitor` node names in the
  header, then the patch area, then a Monitors strip.
- **Sources**: one solid coloured block per route into the device. Accent
  is assigned by source kind — app rules amber `#b75d17`, capture devices
  blue `#2f6fb8`, monitor feeds violet `#7b53b8` (`accents.ts`). White
  text inside; kind caption in tracked uppercase.
- **Wires**: an absolutely-positioned SVG over the patch area draws a
  cubic bezier from each source block's right edge to the left edge of
  *both* channel strips (one wire per source×channel), in the source's
  accent, with a filled port dot where each wire lands. Geometry is
  measured from live DOM rects and recomputed on state change and resize.
- **Output Channels**: a sunken block holding two slim channel strips
  `1 L` / `2 R` plus the sink/mic node names.
- **Monitors strip**: headphone-glyph chips for the device's
  `{VirtualMonitor}` routes to hardware (and other targets), the
  `monitor through…` target picker, and the `use as default mic` action /
  `default mic` pill.
- **Outputs strip** (`Outputs`): every physical sink with route chips
  aimed at it (excluding the selected device's monitor feeds, which live
  in Monitors), plus a live `N playing` count.
- **Drift**: an amber banner above the canvas lists each drift item with
  a `restore` action; drifted routes carry a warn badge/dashed outline.
- **Footer** (`Settings`): `launch at login` autostart toggle + the
  hide-to-tray lifecycle hint, pinned to the window bottom.

## Tokens

Defined in `src/app.css`. Canvas `--bg #f0efec`, rail `#e9e7e2`, blocks
`#ffffff`, sunken `#f7f6f2`, hairline borders `#d9d5cc`. Text
`#2b2925` / dim `#6e695f` / faint `#9b968b`. Interactive accent
`#2f6fb8` with `#e4edf8` tint. Warn `#8f6410` on `#faf1db`, danger
`#b4362e` on `#fae9e7`. Radius grammar 10px outer / 7px inner; one soft
offset shadow, no halos.

## Type & icons

`system-ui` stack. The device name (17px, 650) is the only large type;
section labels are 10.5px tracked small-caps and carry the section name
themselves (no kickers). `ui-monospace` is reserved for real node names
(`fio-*`) and sink IDs. Icons are authored inline SVG in one 1.2–1.6px
stroke — device glyph, headphone, `+`, `×` — never unicode glyph
stand-ins.

## Interaction notes

- A device's own monitor is never offered as its own source.
- Delete affordances are icon buttons with `aria-label`s; all inputs are
  labelled. The `×` delete on every route is one click; device delete in
  the rail asks once.
- Outside Tauri (`vite dev` in a browser) `fio.svelte.ts` runs a local
  mock `UiState` so the whole UI stays interactive for design review.
