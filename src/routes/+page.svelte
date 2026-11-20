<script lang="ts">
  import { onMount } from "svelte";
  import DeviceCard from "$lib/DeviceCard.svelte";
  import SourceList from "$lib/SourceList.svelte";
  import {
    clearError,
    createDevice,
    deleteRoute,
    fio,
    restoreDrift,
    start,
    stop,
  } from "$lib/fio.svelte";
  import { driftMessage, physicalSinks, routeDrift, routeLabel } from "$lib/types";

  let deviceName = $state("");

  const ui = $derived(fio.ui);
  const outputs = $derived(ui ? physicalSinks(ui) : []);
  const serverOffline = $derived(
    ui !== null && ui.sinks.length === 0 && ui.sources.length === 0,
  );

  onMount(() => {
    void start();
    return stop;
  });

  function submitDevice(e: Event) {
    e.preventDefault();
    const name = deviceName.trim();
    if (!name) return;
    void createDevice(name).then((ok) => {
      if (ok) deviceName = "";
    });
  }

  /** Routes pointing at a physical sink, shown as chips under that output. */
  function routesToSink(name: string) {
    return (ui?.routes ?? []).filter(
      (r) => "PhysicalSink" in r.target && r.target.PhysicalSink === name,
    );
  }
</script>

<div class="shell">
  <header class="top">
    <div class="brand">
      <span class="mark">fio</span>
      <span class="tag">virtual audio patchbay</span>
    </div>
    <div class="status">
      {#if ui}
        <span class="stat">{ui.live_streams.length} live stream{ui.live_streams.length === 1 ? "" : "s"}</span>
        <span class="sep">·</span>
        <span class="stat">{ui.devices.length} device{ui.devices.length === 1 ? "" : "s"}</span>
        <span class="sep">·</span>
        <span class="stat">{ui.routes.length} route{ui.routes.length === 1 ? "" : "s"}</span>
        {#if serverOffline}
          <span class="badge danger-badge">audio server offline</span>
        {/if}
      {:else if fio.loading}
        <span class="stat dim">connecting…</span>
      {/if}
    </div>
  </header>

  {#if ui && ui.drift.length > 0}
    <aside class="drift-banner" role="alert">
      <div class="drift-head">
        <span class="badge warn">drift</span>
        <span class="drift-title">
          {ui.drift.length} item{ui.drift.length === 1 ? "" : "s"} out of sync
          with the audio server
        </span>
        <button class="primary" onclick={() => void restoreDrift()}>
          restore
        </button>
      </div>
      <ul class="drift-list">
        {#each ui.drift as d, i (i)}
          <li>{driftMessage(d, ui)}</li>
        {/each}
      </ul>
    </aside>
  {/if}

  <main class="patchbay">
    <SourceList />

    <section class="col" aria-label="virtual devices">
      <header class="col-head">
        <h2>Virtual devices</h2>
      </header>

      <form class="create-form" onsubmit={submitDevice}>
        <input
          class="grow"
          placeholder="New device name — e.g. Game, Chat"
          bind:value={deviceName}
        />
        <button class="primary" type="submit" disabled={deviceName.trim() === ""}>
          add
        </button>
      </form>

      <div class="devices">
        {#if !ui}
          <p class="empty">loading…</p>
        {:else if ui.devices.length === 0}
          <p class="empty">
            no virtual devices yet — each one you add appears as a speaker
            (<span class="mono">fio-*</span>) and a mic (<span class="mono"
              >fio-*.monitor</span
            >) in your audio settings
          </p>
        {:else}
          {#each ui.devices as device (device.id)}
            <DeviceCard {device} />
          {/each}
        {/if}
      </div>
    </section>

    <section class="col" aria-label="outputs">
      <header class="col-head">
        <h2>Outputs</h2>
      </header>
      {#if ui}
        {#if outputs.length === 0}
          <p class="empty">no physical outputs found</p>
        {:else}
          {#each outputs as sink (sink.index)}
            {@const playing = ui.live_streams.filter(
              (s) => s.sink === sink.index,
            ).length}
            {@const routes = routesToSink(sink.name)}
            <div class="output card">
              <div class="row-main">
                <span class="name">{sink.description ?? sink.name}</span>
                <span class="sub mono">{sink.name}</span>
              </div>
              {#if playing > 0}
                <span class="badge">{playing} stream{playing === 1 ? "" : "s"}</span>
              {/if}
              {#if routes.length > 0}
                <div class="route-block">
                  {#each routes as r (r.id)}
                    <div
                      class="route-chip"
                      class:drifted={routeDrift(ui, r.id) !== null}
                    >
                      <span class="route-label">{routeLabel(r, ui)}</span>
                      {#if routeDrift(ui, r.id)}
                        <span class="badge warn">{routeDrift(ui, r.id)}</span>
                      {/if}
                      <button
                        class="chip-x"
                        title="delete route"
                        onclick={() => void deleteRoute(r.id)}>×</button
                      >
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        {/if}
      {/if}
    </section>
  </main>

  {#if fio.error}
    <div class="toast" role="alert">
      <span class="toast-msg">{fio.error}</span>
      <button class="chip-x" title="dismiss" onclick={clearError}>×</button>
    </div>
  {/if}
</div>

<style>
  .shell {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-soft);
    background: var(--panel);
  }

  .brand {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .mark {
    font-family: var(--mono);
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.5px;
    color: var(--accent);
  }

  .tag {
    color: var(--text-dim);
    font-size: 12px;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .sep {
    color: var(--text-faint);
  }

  .stat.dim {
    color: var(--text-faint);
  }

  .drift-banner {
    margin: 14px 20px 0;
    padding: 12px 14px;
    border: 1px solid var(--warn);
    border-radius: 8px;
    background: var(--warn-soft);
  }

  .drift-head {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .drift-title {
    flex: 1;
    color: var(--warn);
    font-weight: 600;
  }

  .drift-list {
    margin: 8px 0 0;
    padding-left: 18px;
    color: var(--text-dim);
    font-size: 12px;
  }

  .patchbay {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(280px, 1fr) minmax(340px, 1.25fr) minmax(
        280px,
        1fr
      );
    gap: 14px;
    padding: 14px 20px 20px;
    align-items: start;
  }

  @media (max-width: 980px) {
    .patchbay {
      grid-template-columns: 1fr;
    }
  }

  :global(.col) {
    background: var(--panel);
    border: 1px solid var(--border-soft);
    border-radius: 10px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-height: 200px;
  }

  :global(.col-head) {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  :global(.col-head h2) {
    margin: 0;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--text-dim);
  }

  :global(.group) {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  :global(.group h3) {
    margin: 4px 0 2px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--text-faint);
  }

  :global(.card) {
    background: var(--panel-2);
    border: 1px solid var(--border-soft);
    border-radius: 7px;
    padding: 8px 10px;
  }

  :global(.row) {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  :global(.row-main) {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  :global(.row-side) {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  :global(.name) {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  :global(.sub) {
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  :global(.mono) {
    font-family: var(--mono);
    font-size: 11px;
  }

  :global(.empty) {
    margin: 4px 0;
    color: var(--text-faint);
    font-size: 12px;
  }

  :global(.hint) {
    margin: 2px 0 0;
    color: var(--text-faint);
    font-size: 11px;
  }

  :global(.grow) {
    flex: 1;
    min-width: 0;
  }

  :global(.badge) {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 4px;
    border: 1px solid var(--border);
    color: var(--text-dim);
    white-space: nowrap;
  }

  :global(.badge.accent) {
    border-color: var(--accent);
    color: var(--accent);
    background: var(--accent-soft);
  }

  :global(.badge.warn) {
    border-color: var(--warn);
    color: var(--warn);
    background: var(--warn-soft);
  }

  .danger-badge {
    border-color: var(--danger);
    color: var(--danger);
    background: var(--danger-soft);
  }

  :global(.dot) {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    flex: none;
  }

  .create-form {
    display: flex;
    gap: 8px;
  }

  .devices {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  :global(.device) {
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  :global(.device.missing) {
    border-left-color: var(--warn);
  }

  :global(.device-head) {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
  }

  :global(.device-title) {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  :global(.device-title .name) {
    font-size: 15px;
  }

  :global(.device-actions) {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  :global(.nodes) {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    color: var(--text-dim);
  }

  :global(.node) {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }

  :global(.node-kind) {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--text-faint);
  }

  :global(.route-block) {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  :global(.route-block h4) {
    margin: 0;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--text-faint);
  }

  :global(.route-chip) {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--panel-3);
    border: 1px solid var(--border-soft);
    border-radius: 5px;
    padding: 3px 6px 3px 8px;
    font-size: 11px;
  }

  :global(.route-chip.drifted) {
    border-color: var(--warn);
  }

  :global(.route-label) {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
  }

  :global(.chip-x) {
    border: none;
    background: none;
    color: var(--text-faint);
    padding: 0 4px;
    font-size: 14px;
    line-height: 1;
  }

  :global(.chip-x:hover) {
    color: var(--danger);
  }

  :global(.device-foot) {
    display: flex;
    justify-content: flex-end;
  }

  .output {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .toast {
    position: fixed;
    bottom: 18px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: min(720px, calc(100vw - 40px));
    padding: 10px 14px;
    border: 1px solid var(--danger);
    border-radius: 8px;
    background: #1c1113;
    color: var(--danger);
    font-size: 12px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  }

  .toast-msg {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
