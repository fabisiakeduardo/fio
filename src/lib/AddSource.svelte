<script lang="ts">
  /**
   * The "+ Add Source" panel inside a device block. Lists everything that
   * can feed this device — live app streams, physical capture devices,
   * other devices' monitors, seen app names, and a free-text app rule —
   * each row with an "add" button wiring it into this device plus a
   * "route to…" picker for every other target (physical outputs, other
   * devices). Sources already routed into this device drop out.
   */
  import RouteForm from "./RouteForm.svelte";
  import { createRoute } from "./fio.svelte";
  import { ACCENT, sourceKind } from "./accents";
  import {
    captureSources,
    monitorName,
    sameSource,
    sameTarget,
    sinkNameForIndex,
    type RouteSource,
    type UiState,
    type VirtualDevice,
  } from "./types";

  interface Props {
    ui: UiState;
    device: VirtualDevice;
    onclose: () => void;
  }
  let { ui, device, onclose }: Props = $props();

  let newApp = $state("");

  interface Item {
    name: string;
    sub: string;
    source: RouteSource;
  }

  const target = $derived({ VirtualDevice: device.id });

  const already = $derived(
    ui.routes.filter(
      (r) => "VirtualDevice" in r.target && r.target.VirtualDevice === device.id,
    ),
  );

  const routed = (s: RouteSource) =>
    already.some(
      (r) => sameSource(r.source, s) && sameTarget(r.target, target),
    );

  /** Live app-name keys, deduped. */
  const liveApps = $derived<Item[]>(
    [
      ...new Map(
        ui.live_streams
          .map((s) => [s.app_name ?? s.media_name, s] as const)
          .filter((e): e is [string, (typeof e)[1]] => e[0] != null),
      ).values(),
    ]
      .filter((s) => !routed({ AppName: (s.app_name ?? s.media_name)! }))
      .map((s) => ({
        name: (s.app_name ?? s.media_name)!,
        sub: `playing now → ${sinkNameForIndex(ui, s.sink)}`,
        source: { AppName: (s.app_name ?? s.media_name)! },
      })),
  );

  const captures = $derived<Item[]>(
    captureSources(ui)
      .filter((s) => !routed({ CaptureDevice: s.name }))
      .map((s) => ({
        name: s.description ?? s.name,
        sub: s.description ? s.name : "physical input",
        source: { CaptureDevice: s.name },
      })),
  );

  /** Other devices' monitors — never this device's own (no self-feedback). */
  const monitors = $derived<Item[]>(
    ui.devices
      .filter((d) => d.id !== device.id)
      .filter((d) => !routed({ VirtualMonitor: d.id }))
      .map((d) => ({
        name: `${d.name} monitor`,
        sub: monitorName(d),
        source: { VirtualMonitor: d.id },
      })),
  );

  const seenApps = $derived<Item[]>(
    ui.seen_apps
      .filter(
        (a) => !liveApps.some((l) => l.name === a) && !routed({ AppName: a }),
      )
      .map((a) => ({
        name: a,
        sub: "seen earlier",
        source: { AppName: a },
      })),
  );

  const groups = $derived(
    [
      { label: "Live streams", items: liveApps },
      { label: "Capture devices", items: captures },
      { label: "Device monitors", items: monitors },
      { label: "Seen apps", items: seenApps },
    ].filter((g) => g.items.length > 0),
  );

  const empty = $derived(groups.length === 0);

  /** Primary gesture: wire this source into the device. */
  function add(source: RouteSource) {
    void createRoute(source, target);
  }

  /** Alternate targets — any physical output or other virtual device. */
  function routeElsewhere(source: RouteSource) {
    return (t: Parameters<typeof createRoute>[1]) =>
      void createRoute(source, t);
  }

  function addFree() {
    const name = newApp.trim();
    if (!name) return;
    void createRoute({ AppName: name }, target).then((ok) => {
      if (ok) newApp = "";
    });
  }
</script>

<div class="add-panel">
  <div class="panel-head">
    <span class="sec-label">Add a source to {device.name}</span>
    <button class="done" onclick={onclose}>done</button>
  </div>

  <div class="panel-body">
    {#each groups as g (g.label)}
      <h4 class="sec-label">{g.label}</h4>
      {#each g.items as item (item.name)}
        {@const accent = ACCENT[sourceKind(item.source)]}
        <div class="opt">
          <span class="swatch" style="background:{accent.base}"></span>
          <span class="opt-main">
            <span class="opt-name">{item.name}</span>
            <span class="opt-sub mono">{item.sub}</span>
          </span>
          <button
            class="opt-add"
            aria-label="route {item.name} to {device.name}"
            onclick={() => add(item.source)}
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
            add
          </button>
          <RouteForm
            {ui}
            source={item.source}
            exclude={[target]}
            placeholder="route to…"
            onroute={routeElsewhere(item.source)}
          />
        </div>
      {/each}
    {/each}

    {#if empty}
      <p class="none">
        nothing unrouted right now — type an app name below to write a rule
      </p>
    {/if}
  </div>

  <form
    class="free"
    onsubmit={(e) => {
      e.preventDefault();
      addFree();
    }}
  >
    <input
      aria-label="app name rule"
      placeholder="app name — e.g. discord"
      bind:value={newApp}
    />
    <button class="primary" type="submit" disabled={newApp.trim() === ""}>
      add rule
    </button>
    <RouteForm
      {ui}
      source={{ AppName: newApp.trim() }}
      exclude={[target]}
      placeholder="route to…"
      disabled={newApp.trim() === ""}
      onroute={routeElsewhere({ AppName: newApp.trim() })}
    />
  </form>
  <p class="hint">
    app rules match PulseAudio's <span class="mono">application.name</span>
  </p>
</div>

<style>
  .add-panel {
    border: 1px solid var(--border);
    border-radius: var(--r-inner);
    background: var(--block-sunk);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border-soft);
  }

  .panel-head .sec-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .done {
    flex: none;
    padding: 1px 8px;
    font-size: 11px;
  }

  .panel-body {
    max-height: 218px;
    overflow-y: auto;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .panel-body h4 {
    margin: 8px 0 3px;
  }

  .opt {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 4px 4px 8px;
    border-radius: 6px;
    background: var(--block);
    border: 1px solid var(--border-soft);
  }

  .swatch {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .opt-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .opt-name {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .opt-sub {
    color: var(--text-faint);
    font-size: 10px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .opt-add {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    font-size: 11px;
    color: var(--accent-deep);
    border-color: var(--border);
  }

  .opt :global(.route-select),
  .free :global(.route-select) {
    flex: none;
    width: 118px;
    padding: 2px 4px;
    font-size: 11px;
  }

  .none {
    margin: 4px 2px;
    font-size: 11.5px;
    color: var(--text-faint);
    max-width: 32ch;
  }

  .free {
    display: flex;
    gap: 6px;
    padding: 8px 8px 0;
    border-top: 1px solid var(--border-soft);
  }

  .free input {
    flex: 1;
  }

  .hint {
    margin: 5px 8px 8px;
    font-size: 10.5px;
    color: var(--text-faint);
  }
</style>
