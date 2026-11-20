<script lang="ts">
  /**
   * The Loopback canvas: the selected virtual device drawn as one large
   * rounded block. Sources are coloured blocks on the left; the Output
   * Channels block on the right carries the stereo L/R strips; bezier
   * wires in each source's accent fan into both channels. A Monitors
   * strip underneath holds the device's monitor feeds and the
   * default-mic action.
   */
  import { onMount, tick } from "svelte";
  import AddSource from "./AddSource.svelte";
  import RouteForm from "./RouteForm.svelte";
  import { createRoute, deleteRoute, setDefaultMic } from "./fio.svelte";
  import { kindLabel, sourceAccent } from "./accents";
  import {
    deviceDrifted,
    monitorName,
    routeDrift,
    sinkName,
    sourceLabel,
    targetLabel,
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

  const monitor = $derived(monitorName(device));
  const isDefaultMic = $derived(ui.default_source === monitor);
  const missing = $derived(deviceDrifted(ui, device.id));

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
    void createRoute({ VirtualMonitor: device.id }, target);
  }

  /* ------- wires ------- */

  interface Wire {
    key: string;
    d: string;
    color: string;
    x2: number;
    y2: number;
  }

  let patchEl = $state<HTMLDivElement | null>(null);
  const srcEls = new Map<string, HTMLElement>();
  let stripL = $state<HTMLElement | null>(null);
  let stripR = $state<HTMLElement | null>(null);
  let wires = $state<Wire[]>([]);
  let wireBox = $state({ w: 0, h: 0 });

  function measure() {
    const host = patchEl;
    if (!host) return;
    const hr = host.getBoundingClientRect();
    wireBox = { w: hr.width, h: hr.height };
    const strips = [stripL, stripR].filter((s): s is HTMLElement => s != null);
    const out: Wire[] = [];
    const n = routesIn.length;
    routesIn.forEach((r, i) => {
      const el = srcEls.get(r.id);
      if (!el) return;
      const sr = el.getBoundingClientRect();
      const x1 = sr.right - hr.left;
      const y1 = sr.top + sr.height / 2 - hr.top;
      const color = sourceAccent(r.source).base;
      strips.forEach((st, j) => {
        const tr = st.getBoundingClientRect();
        const pad = Math.min(16, tr.height * 0.14);
        const y2 =
          tr.top + pad + ((tr.height - pad * 2) * (i + 1)) / (n + 1) - hr.top;
        const x2 = tr.left - hr.left + 1;
        const c = Math.max(30, (x2 - x1) * 0.55);
        out.push({
          key: `${r.id}-${j}`,
          d: `M ${x1} ${y1} C ${x1 + c} ${y1}, ${x2 - c} ${y2}, ${x2} ${y2}`,
          color,
          x2,
          y2,
        });
      });
    });
    wires = out;
  }

  $effect(() => {
    // Re-measure whenever anything that moves the geometry changes.
    routesIn.length;
    device.id;
    addOpen;
    ui;
    stripL;
    stripR;
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
    <div class="dev-id">
      <svg
        class="dev-glyph"
        viewBox="0 0 20 20"
        width="22"
        height="22"
        aria-hidden="true"
      >
        <rect
          x="1.5"
          y="1.5"
          width="17"
          height="17"
          rx="4.5"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
        />
        <path
          d="M6.4 13.4V8.8M10 14.6V5.6M13.6 12.4V7.8"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
        />
      </svg>
      <div class="dev-names">
        <h2 class="dev-name">{device.name}</h2>
        <p class="dev-nodes mono">
          {sinkName(device)} · {monitor}
        </p>
      </div>
    </div>
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
        <path d={w.d} fill="none" stroke={w.color} stroke-width="2.2" />
        <circle cx={w.x2} cy={w.y2} r="3" fill={w.color} />
      {/each}
    </svg>

    <section class="sources" aria-label="sources">
      <div class="sec-head">
        <h3 class="sec-label">Sources</h3>
        {#if routesIn.length > 0}
          <span class="count">{routesIn.length}</span>
        {/if}
      </div>

      {#if routesIn.length === 0 && !addOpen}
        <p class="quiet">nothing routed in yet</p>
      {/if}

      {#each routesIn as r (r.id)}
        {@const accent = sourceAccent(r.source)}
        {@const drift = routeDrift(ui, r.id)}
        <div
          class="src"
          class:drifted={drift !== null}
          style="--src:{accent.base}"
          {@attach (el: HTMLElement) => {
            srcEls.set(r.id, el);
            return () => srcEls.delete(r.id);
          }}
        >
          <div class="src-main">
            <span class="src-name">{sourceLabel(r.source, ui)}</span>
            <span class="src-kind">{kindLabel(r.source)}</span>
          </div>
          {#if drift}
            <span class="src-drift">{drift}</span>
          {/if}
          <button
            class="src-x"
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
      {/each}

      {#if addOpen}
        <AddSource {ui} {device} onclose={() => (addOpen = false)} />
      {:else}
        <button class="add-src" onclick={() => (addOpen = true)}>
          <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
            <path
              d="M6 1.5v9M1.5 6h9"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
            />
          </svg>
          add source
        </button>
      {/if}
    </section>

    <section class="outblock" aria-label="output channels">
      <div class="sec-head">
        <h3 class="sec-label">Output Channels</h3>
      </div>
      <div class="strips">
        <div class="strip" bind:this={stripL}>
          <span class="ch-num">1</span>
          <span class="ch-side">L</span>
        </div>
        <div class="strip" bind:this={stripR}>
          <span class="ch-num">2</span>
          <span class="ch-side">R</span>
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
  </div>

  <footer class="monitors">
    <div class="sec-head">
      <h3 class="sec-label">Monitors</h3>
    </div>
    <div class="mon-row">
      {#if monitorRoutes.length === 0}
        <span class="quiet">not monitored — output is silent to you</span>
      {:else}
        {#each monitorRoutes as r (r.id)}
          {@const drift = routeDrift(ui, r.id)}
          <span class="mon-chip" class:drifted={drift !== null}>
            <svg
              viewBox="0 0 12 12"
              width="10"
              height="10"
              aria-hidden="true"
              class="mon-ic"
            >
              <path
                d="M2 8.5V6a4 4 0 0 1 8 0v2.5M1.6 8.5h2.4v2H1.6zM8 8.5h2.4v2H8z"
                fill="none"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linejoin="round"
              />
            </svg>
            {targetLabel(r.target, ui)}
            {#if drift}
              <span class="chip-drift">{drift}</span>
            {/if}
            <button
              class="chip-x"
              aria-label="delete monitor feed to {targetLabel(r.target, ui)}"
              title="delete route"
              onclick={() => void deleteRoute(r.id)}
            >
              <svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
                <path
                  d="M1.5 1.5l7 7M8.5 1.5l-7 7"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
            </button>
          </span>
        {/each}
      {/if}

      <RouteForm
        {ui}
        source={{ VirtualMonitor: device.id }}
        exclude={[{ VirtualDevice: device.id }]}
        placeholder="monitor through…"
        onroute={feedMonitor}
      />

      <span class="mon-spacer"></span>

      {#if isDefaultMic}
        <span class="pill accent">default mic</span>
      {:else}
        <button onclick={() => void setDefaultMic(device.id)}>
          use as default mic
        </button>
      {/if}
    </div>
  </footer>
</article>

<style>
  .devblock {
    background: var(--block);
    border: 1px solid var(--border);
    border-radius: var(--r-block);
    box-shadow: var(--shadow-block);
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .devblock.missing {
    border-color: var(--warn-border);
  }

  .dev-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 16px 18px 12px;
  }

  .dev-id {
    display: flex;
    align-items: center;
    gap: 11px;
    min-width: 0;
  }

  .dev-glyph {
    flex: none;
    color: var(--accent);
  }

  .dev-names {
    min-width: 0;
  }

  .dev-name {
    margin: 0;
    font-size: 17px;
    font-weight: 650;
    letter-spacing: -0.015em;
    text-wrap: balance;
  }

  .dev-nodes {
    margin: 1px 0 0;
    color: var(--text-faint);
    font-size: 10.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dev-head .pill {
    flex: none;
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
    grid-template-columns: minmax(220px, 1fr) minmax(240px, 0.9fr);
    gap: 0;
    padding: 4px 18px 16px;
    align-items: stretch;
  }

  .wires {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 5;
    overflow: visible;
  }

  .sec-head {
    display: flex;
    align-items: baseline;
    gap: 7px;
    margin-bottom: 8px;
  }

  .count {
    font-size: 10px;
    font-weight: 700;
    color: var(--text-faint);
    background: var(--block-sunk);
    border: 1px solid var(--border-soft);
    border-radius: 999px;
    padding: 0 6px;
  }

  .sources {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    padding-right: 64px;
  }

  .quiet {
    margin: 2px 0;
    font-size: 12px;
    color: var(--text-faint);
  }

  .src {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--src);
    border-radius: var(--r-inner);
    padding: 9px 8px 9px 12px;
    color: #fff;
    box-shadow: 0 1px 2px rgba(43, 41, 37, 0.18);
    position: relative;
    z-index: 1;
  }

  .src.drifted {
    outline: 2px dashed var(--warn-border);
    outline-offset: 2px;
  }

  .src-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .src-name {
    font-weight: 650;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .src-kind {
    font-size: 10px;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: rgba(255, 255, 255, 0.82);
  }

  .src-drift {
    flex: none;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.9);
    color: var(--warn);
  }

  .src-x {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.18);
    color: #fff;
  }

  .src-x:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.34);
    color: #fff;
    border: none;
  }

  .add-src {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    padding: 8px 10px;
    border: 1.5px dashed var(--border);
    border-radius: var(--r-inner);
    background: transparent;
    color: var(--text-dim);
    font-weight: 600;
    position: relative;
    z-index: 1;
  }

  .add-src:hover:not(:disabled) {
    border-style: dashed;
    border-color: var(--accent);
    color: var(--accent-deep);
    background: var(--accent-soft);
  }

  .outblock {
    background: var(--block-sunk);
    border: 1px solid var(--border-soft);
    border-radius: var(--r-inner);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    min-width: 0;
    position: relative;
    z-index: 1;
  }

  .strips {
    flex: 1;
    display: flex;
    gap: 10px;
    min-height: 96px;
  }

  .strip {
    flex: none;
    width: 66px;
    background: var(--block);
    border: 1px solid var(--border-soft);
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    position: relative;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.4);
  }

  .ch-num {
    font-size: 15px;
    font-weight: 700;
    color: var(--text-dim);
  }

  .ch-side {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--text-faint);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
  }

  .nodes {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 10px;
    padding-top: 9px;
    border-top: 1px solid var(--border-soft);
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

  .monitors {
    border-top: 1px solid var(--border-soft);
    padding: 12px 18px 14px;
  }

  .mon-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .mon-row :global(.route-select) {
    max-width: 230px;
  }

  .mon-spacer {
    flex: 1;
  }

  .mon-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 5px 4px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--block-sunk);
    font-size: 12px;
    color: var(--text);
    max-width: 320px;
  }

  .mon-chip.drifted {
    border-color: var(--warn-border);
    background: var(--warn-soft);
  }

  .mon-ic {
    color: var(--src-monitor);
    flex: none;
  }

  .chip-drift {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--warn);
  }

  .chip-x {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text-faint);
  }

  .chip-x:hover:not(:disabled) {
    background: var(--danger-soft);
    color: var(--danger);
    border: none;
  }
</style>
