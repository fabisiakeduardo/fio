## Direction contract

THESIS: the app is a patchbay drawing, not a settings list — one selected
virtual device rendered as a Loopback-style wiring diagram, refusing the
generic three-column CRUD layout and any card-grid.

OWN-WORLD: Loopback 2 adapted to desktop Linux — light warm-gray canvas
(#f0efec), white blocks with hairline borders, an 8px radius grammar,
colored source blocks (app rules orange, capture devices blue, monitor
feeds violet) whose bezier wires fan into a neutral Output Channels block
with L/R channel strips. system-ui stack; the device name is the only
large type, section headers are tracked small-caps eyebrows, node names
are small mono.

STORY: the user opens fio, sees their devices in a left rail, picks one,
and reads instantly what audio flows into it, that it surfaces as
`fio-{slug}` plus `fio-{slug}.monitor`, and what it is monitored through.

FIRST VIEWPORT: left sidebar (device rows + "+" new-device affordance,
delete on the row); right canvas holds the selected device as one large
rounded block: "Sources" column of colored blocks on the left, "Output
Channels" block on the right, curved wires between them; a "Monitors"
strip below with the default-mic action; "+ Add Source" opens the
route-creation flow scoped to this device; an "Outputs" strip under the
canvas lists routes aimed at physical sinks; drift banner sits above the
canvas; autostart toggle lives in the footer.

FORM: pinned — seed: none (pinned). "Loopback (Rogue Amoeba)" is named
whole in the brief as the replacement world, including its first-surface
layout.

FINISH: unreviewed and undocumented is unfinished; this build ends with
the finish review, the verdict, DESIGN.md, and every shipping raster
carrying its provenance.
