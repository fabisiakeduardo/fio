<script lang="ts">
  /**
   * The Loopback-style patchbay for the selected device: three columns —
   * Sources, Output Channels, Monitors — wired channel-row → channel-row
   * with teal cubic béziers. Each routed source is a white card carrying
   * "1 (L)" / "2 (R)" rows with a wire anchor on the right edge. The
   * device is stereo, so Output Channels is a single "Channels 1 & 2"
   * pair card with anchors on both sides; every monitor feed (this
   * device's VirtualMonitor → a target) is a card whose channel rows
   * take wires from the output pair's right-hand anchors.
   */
  import { onMount, tick } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import AddSource from "./AddSource.svelte";
  import RouteForm from "./RouteForm.svelte";
  import {
    createRoute,
    deleteRoute,
    levelOf,
    setDefaultMic,
  } from "./fio.svelte";
  import { kindLabel, sourceAccent, sourceKind } from "./accents";
  import {
    deviceDrifted,
    monitorName,
    routeDrift,
    sinkName,
    sourceLabel,
    targetLabel,
    targetSinkName,
    type RouteTarget,
    type UiState,
    type VirtualDevice,
  } from "./types";

  interface Props {
    ui: UiState;
    device: VirtualDevice;
  }
  let { ui, device }: Props = $props();

  let addOpen = $state(false);
  let monOpen = $state(false);
  let monHidden = $state(false);

  const monitor = $derived(monitorName(device));
  const isDefaultMic = $derived(ui.default_source === monitor);
  const missing = $derived(deviceDrifted(ui, device.id));

  /** Device output level — the `fio-{slug}.monitor` peak. */
  const outLevel = $derived(levelOf(`dev:${device.id}`));

  const routesIn = $derived(
    ui.routes.filter(
      (r) => "VirtualDevice" in r.target && r.target.VirtualDevice === device.id,
    ),
  );
  const monitorRoutes = $derived(
    ui.routes.filter(
      (r) => "VirtualMonitor" in r.source && r.source.VirtualMonitor === device.id,
    ),
  );

  function feedMonitor(target: RouteTarget) {
    void createRoute({ VirtualMonitor: device.id }, target).then((ok) => {
      if (ok) monOpen = false;
    });
  }

  /* ------- wires: channel-row anchor → channel-row anchor ------- */

  interface Wire {
    key: string;
    d: string;
    x1: number;
    y1: number;
    x2: number;
    y2: number;
  }

  let patchEl = $state<HTMLDivElement | null>(null);
  /** Live anchor dots, keyed `s:{routeId}:{ch}` / `o:{in|out}:{ch}` / `m:{routeId}:{ch}`. */
  const ports = new Map<string, HTMLElement>();
  let wires = $state<Wire[]>([]);
  let wireBox = $state({ w: 0, h: 0 });

  const port =
    (key: string): Attachment<HTMLElement> =>
    (el) => {
      ports.set(key, el);
      return () => {
        ports.delete(key);
      };
    };

  function measure() {
    const host = patchEl;
    if (!host) return;
    const hr = host.getBoundingClientRect();
    wireBox = { w: hr.width, h: hr.height };
    const pt = (key: string) => {
      const el = ports.get(key);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return {
        x: r.left + r.width / 2 - hr.left,
        y: r.top + r.height / 2 - hr.top,
      };
    };
    const out: Wire[] = [];
    const wire = (key: string, a: { x: number; y: number } | null, b: { x: number; y: number } | null) => {
      if (!a || !b) return;
      const c = Math.max(24, Math.abs(b.x - a.x) * 0.5);
      out.push({
        key,
        d: `M ${a.x} ${a.y} C ${a.x + c} ${a.y}, ${b.x - c} ${b.y}, ${b.x} ${b.y}`,
        x1: a.x,
        y1: a.y,
        x2: b.x,
        y2: b.y,
      });
    };
    // source channel rows → output pair's left anchors
    for (const r of routesIn) {
      wire(`${r.id}-l`, pt(`s:${r.id}:0`), pt("o:in:0"));
      wire(`${r.id}-r`, pt(`s:${r.id}:1`), pt("o:in:1"));
    }
    // output pair's right anchors → each monitor card's rows
    for (const r of monitorRoutes) {
      wire(`m-${r.id}-l`, pt("o:out:0"), pt(`m:${r.id}:0`));
      wire(`m-${r.id}-r`, pt("o:out:1"), pt(`m:${r.id}:1`));
    }
    wires = out;
  }

  $effect(() => {
    // Re-measure whenever anything that moves the geometry changes.
    routesIn.length;
    monitorRoutes.length;
    device.id;
    addOpen;
    monOpen;
    monHidden;
    ui;
    void tick().then(() => requestAnimationFrame(measure));
  });

  onMount(() => {
    const ro = new ResizeObserver(() => measure());
    if (patchEl) ro.observe(patchEl);
    return () => ro.disconnect();
  });
</script>

<article class="devblock" class:missing>
  <header class="dev-head">
    <h2 class="dev-name">{device.name}</h2>
    <p class="dev-nodes mono">{sinkName(device)} · {monitor}</p>
    {#if missing}
      <span class="pill warn">missing from server</span>
    {/if}
  </header>

  <div class="patch" bind:this={patchEl}>
    <svg
      class="wires"
      width={wireBox.w}
      height={wireBox.h}
      viewBox="0 0 {wireBox.w} {wireBox.h}"
      aria-hidden="true"
    >
      {#each wires as w (w.key)}
        <path d={w.d} fill="none" stroke="var(--wire)" stroke-width="1.6" />
      {/each}
    </svg>

    <section class="col" aria-label="sources">
      <div class="col-head">
        <div class="col-titles">
          <h3 class="col-title">Sources</h3>
          <span class="col-sub">
            {routesIn.length === 0
              ? "none routed"
              : `${routesIn.length} routed`}
          </span>
        </div>
        <button
          class="col-plus"
          class:open={addOpen}
          aria-label="add source"
          title="add source"
          onclick={() => (addOpen = !addOpen)}
        >
          <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
            <path
              d="M6 1.5v9M1.5 6h9"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
            />
          </svg>
        </button>
      </div>

      {#each routesIn as r (r.id)}
        {@const accent = sourceAccent(r.source)}
        {@const kind = sourceKind(r.source)}
        {@const drift = routeDrift(ui, r.id)}
        {@const lvl = levelOf(`route:${r.id}`)}
        <div
          class="card src"
          class:drifted={drift !== null}
          style="--a:{accent.base}; --as:{accent.soft}"
        >
          <div class="card-head">
            <span class="src-ic" aria-hidden="true">
              {#if kind === "app"}
                <svg viewBox="0 0 14 14" width="14" height="14">
                  <rect
                    x="1.8"
                    y="1.8"
                    width="10.4"
                    height="10.4"
                    rx="2.4"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                  />
                  <path
                    d="M1.8 4.6h10.4"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                  />
                  <circle cx="3.6" cy="3.2" r="0.7" fill="currentColor" />
                </svg>
              {:else if kind === "capture"}
                <svg viewBox="0 0 14 14" width="14" height="14">
                  <rect
                    x="5"
                    y="1.6"
                    width="4"
                    height="6.8"
                    rx="2"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                  />
                  <path
                    d="M3.2 7.4a3.8 3.8 0 0 0 7.6 0M7 11.2v1.6M5 12.8h4"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                    stroke-linecap="round"
                  />
                </svg>
              {:else}
                <svg viewBox="0 0 14 14" width="14" height="14">
                  <path
                    d="M2.6 9V7a4.4 4.4 0 0 1 8.8 0v2M2 9h2.6v3H2zM9.4 9H12v3H9.4z"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                    stroke-linejoin="round"
                  />
                </svg>
              {/if}
            </span>
            <div class="card-names">
              <span class="card-name">{sourceLabel(r.source, ui)}</span>
              <span class="card-kind">{kindLabel(r.source)}</span>
            </div>
            {#if drift}
              <span class="src-drift">{drift}</span>
            {/if}
            <button
              class="card-x"
              aria-label="delete route {sourceLabel(r.source, ui)}"
              title="delete route"
              onclick={() => void deleteRoute(r.id)}
            >
              <svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true">
                <path
                  d="M1.5 1.5l7 7M8.5 1.5l-7 7"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
            </button>
          </div>
          <div class="ch-row">
            <span class="ch-label">1 (L)</span>
            <span class="ch-bar">
              <span class="ch-fill" style="transform: scaleX({lvl})"></span>
            </span>
            <span class="port" {@attach port(`s:${r.id}:0`)}></span>
          </div>
          <div class="ch-row">
            <span class="ch-label">2 (R)</span>
            <span class="ch-bar">
              <span class="ch-fill" style="transform: scaleX({lvl})"></span>
            </span>
            <span class="port" {@attach port(`s:${r.id}:1`)}></span>
          </div>
        </div>
      {/each}

      {#if addOpen}
        <AddSource {ui} {device} onclose={() => (addOpen = false)} />
      {:else if routesIn.length === 0}
        <p class="quiet">nothing routed in yet. Press + to add a source</p>
      {/if}
    </section>

    <section class="col" aria-label="output channels">
      <div class="col-head">
        <div class="col-titles">
          <h3 class="col-title">Output Channels</h3>
          <span class="col-sub">2 channels</span>
        </div>
      </div>

      <div class="card pair">
        <div class="pair-head">Channels 1 &amp; 2</div>
        <div class="ch-row">
          <span class="port" {@attach port("o:in:0")}></span>
          <span class="ch-label">Channel 1 (L)</span>
          <span class="ch-bar">
            <span class="ch-fill" style="transform: scaleX({outLevel})"></span>
          </span>
          <span class="port" {@attach port("o:out:0")}></span>
        </div>
        <div class="ch-row">
          <span class="port" {@attach port("o:in:1")}></span>
          <span class="ch-label">Channel 2 (R)</span>
          <span class="ch-bar">
            <span class="ch-fill" style="transform: scaleX({outLevel})"></span>
          </span>
          <span class="port" {@attach port("o:out:1")}></span>
        </div>
      </div>

      <div class="nodes">
        <div class="node">
          <span class="node-kind">sink</span>
          <span class="mono">{sinkName(device)}</span>
        </div>
        <div class="node">
          <span class="node-kind">mic</span>
          <span class="mono">{monitor}</span>
        </div>
      </div>
    </section>

    <section class="col" aria-label="monitors">
      <div class="col-head">
        <div class="col-titles">
          <h3 class="col-title">Monitors</h3>
          <span class="col-sub">
            {monitorRoutes.length === 0
              ? "none"
              : `${monitorRoutes.length} device${monitorRoutes.length === 1 ? "" : "s"}`}
          </span>
        </div>
        <button
          class="col-plus"
          class:open={monOpen}
          aria-label="add monitor"
          title="monitor this device through an output"
          onclick={() => (monOpen = !monOpen)}
        >
          <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
            <path
              d="M6 1.5v9M1.5 6h9"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
            />
          </svg>
        </button>
      </div>

      {#if monOpen}
        <div class="mon-add">
          <RouteForm
            {ui}
            source={{ VirtualMonitor: device.id }}
            exclude={[{ VirtualDevice: device.id }]}
            placeholder="monitor through…"
            onroute={feedMonitor}
          />
        </div>
      {/if}

      {#if !monHidden}
        {#each monitorRoutes as r (r.id)}
          {@const drift = routeDrift(ui, r.id)}
          {@const tSink = targetSinkName(r.target, ui)}
          {@const mlvl = tSink ? levelOf(`mon:${tSink}`) : 0}
          <div class="card mon" class:drifted={drift !== null}>
            <div class="card-head">
              <span class="mon-ic" aria-hidden="true">
                <svg viewBox="0 0 14 14" width="13" height="13">
                  <path
                    d="M2.6 9V7a4.4 4.4 0 0 1 8.8 0v2M2 9h2.6v3H2zM9.4 9H12v3H9.4z"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.2"
                    stroke-linejoin="round"
                  />
                </svg>
              </span>
              <div class="card-names">
                <span class="card-name">{targetLabel(r.target, ui)}</span>
                <span class="card-kind">monitor feed</span>
              </div>
              {#if drift}
                <span class="src-drift">{drift}</span>
              {/if}
              <button
                class="card-x"
                aria-label="delete monitor feed to {targetLabel(r.target, ui)}"
                title="delete route"
                onclick={() => void deleteRoute(r.id)}
              >
                <svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true">
                  <path
                    d="M1.5 1.5l7 7M8.5 1.5l-7 7"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                  />
                </svg>
              </button>
            </div>
            <div class="ch-row">
              <span class="port" {@attach port(`m:${r.id}:0`)}></span>
              <span class="ch-label">Channel 1 (L)</span>
              <span class="ch-bar">
                <span class="ch-fill" style="transform: scaleX({mlvl})"></span>
              </span>
            </div>
            <div class="ch-row">
              <span class="port" {@attach port(`m:${r.id}:1`)}></span>
              <span class="ch-label">Channel 2 (R)</span>
              <span class="ch-bar">
                <span class="ch-fill" style="transform: scaleX({mlvl})"></span>
              </span>
            </div>
          </div>
        {:else}
          {#if !monOpen}
            <p class="quiet">not monitored, so the output is silent to you</p>
          {/if}
        {/each}
      {:else}
        <p class="quiet">monitors hidden</p>
      {/if}
    </section>
  </div>

  <footer class="dev-foot">
    {#if isDefaultMic}
      <span class="pill accent">default mic</span>
    {:else}
      <button class="ghost" onclick={() => void setDefaultMic(device.id)}>
        use as default mic
      </button>
    {/if}
    <span class="foot-spacer"></span>
    <button class="ghost" onclick={() => (monHidden = !monHidden)}>
      {monHidden ? "show monitors" : "hide monitors"}
    </button>
  </footer>
</article>

<style>
  .devblock {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .dev-head {
    position: relative;
    text-align: center;
    padding: 2px 0 14px;
  }

  .dev-name {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
    letter-spacing: -0.015em;
    text-wrap: balance;
  }

  .dev-nodes {
    margin: 2px 0 0;
    color: var(--text-faint);
    font-size: 10.5px;
  }

  .dev-head .pill {
    position: absolute;
    right: 0;
    top: 2px;
  }

  .pill {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-dim);
    white-space: nowrap;
  }

  .pill.accent {
    border-color: transparent;
    color: var(--accent-deep);
    background: var(--accent-soft);
  }

  .pill.warn {
    border-color: transparent;
    color: var(--warn);
    background: var(--warn-soft);
  }

  .patch {
    position: relative;
    display: grid;
    grid-template-columns:
      minmax(200px, 1.05fr) minmax(180px, 0.95fr)
      minmax(190px, 1fr);
    column-gap: clamp(34px, 4.5vw, 68px);
    align-items: start;
  }

  .wires {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 6;
    overflow: visible;
  }

  @media (max-width: 860px) {
    .patch {
      grid-template-columns: minmax(0, 1fr);
      row-gap: 22px;
    }
  }

  .col {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    position: relative;
    z-index: 1;
  }

  .col-head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
  }

  .col-titles {
    display: flex;
    flex-direction: column;
    gap: 0;
    min-width: 0;
  }

  .col-title {
    margin: 0;
    font-size: 13px;
    font-weight: 650;
    color: var(--text);
    white-space: nowrap;
  }

  .col-sub {
    font-size: 10.5px;
    color: var(--text-faint);
    white-space: nowrap;
  }

  .col-plus {
    flex: none;
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 19px;
    height: 19px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--border);
    background: var(--block);
    color: var(--text-dim);
  }

  .col-plus:hover:not(:disabled),
  .col-plus.open {
    border-color: var(--accent);
    color: var(--accent-deep);
    background: var(--accent-soft);
  }

  .card {
    background: var(--block);
    border: 1px solid var(--border-soft);
    border-radius: 8px;
    box-shadow:
      0 1px 2px rgba(43, 41, 37, 0.07),
      0 2px 8px rgba(43, 41, 37, 0.05);
    padding: 8px 10px 9px;
    min-width: 0;
  }

  .card.drifted {
    outline: 2px dashed var(--warn-border);
    outline-offset: 2px;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 9px;
    padding-bottom: 6px;
    min-width: 0;
  }

  .src-ic {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--a);
  }

  .mon-ic {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--src-monitor);
  }

  .card-names {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .card-name {
    font-weight: 650;
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .card-kind {
    font-size: 9.5px;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .src-drift {
    flex: none;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--warn-soft);
    color: var(--warn);
  }

  .card-x {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text-faint);
  }

  .card-x:hover:not(:disabled) {
    background: var(--danger-soft);
    color: var(--danger);
    border: none;
  }

  .ch-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0 4px 2px;
    border-top: 1px solid var(--border-soft);
  }

  .ch-label {
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .ch-bar {
    flex: 1;
    position: relative;
    height: 3px;
    border-radius: 2px;
    background: #ddd9cf;
    overflow: hidden;
  }

  /* Slim VU fill — scaleX keeps updates off the layout path at ~8 Hz. */
  .ch-fill {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: var(--wire);
    transform: scaleX(0);
    transform-origin: left center;
    transition: transform 110ms linear;
  }

  .port {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--block);
    border: 2px solid var(--wire);
  }

  .quiet {
    margin: 2px 0;
    font-size: 12px;
    color: var(--text-faint);
  }

  .pair-head {
    font-size: 12px;
    font-weight: 650;
    padding-bottom: 6px;
  }

  .nodes {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .node {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .node-kind {
    flex: none;
    width: 26px;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-faint);
  }

  .node .mono {
    color: var(--text-dim);
    font-size: 10.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mon-add {
    padding: 8px 10px;
    background: var(--block);
    border: 1px solid var(--border-soft);
    border-radius: 8px;
    box-shadow:
      0 1px 2px rgba(43, 41, 37, 0.07),
      0 2px 8px rgba(43, 41, 37, 0.05);
  }

  .mon-add :global(.route-select) {
    width: 100%;
  }

  .dev-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-top: 14px;
  }

  .foot-spacer {
    flex: 1;
  }

  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-dim);
    font-size: 11.5px;
    padding: 4px 10px;
  }

  .ghost:hover:not(:disabled) {
    background: var(--block);
  }
</style>
