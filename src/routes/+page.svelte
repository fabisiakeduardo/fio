<script lang="ts">
  import { onMount } from "svelte";
  import DeviceCanvas from "$lib/DeviceCanvas.svelte";
  import DeviceSidebar from "$lib/DeviceSidebar.svelte";
  import Outputs from "$lib/Outputs.svelte";
  import Settings from "$lib/Settings.svelte";
  import {
    clearError,
    fio,
    restoreDrift,
    start,
    stop,
  } from "$lib/fio.svelte";
  import { driftMessage } from "$lib/types";

  let selectedId = $state<string | null>(null);

  const ui = $derived(fio.ui);
  const selected = $derived(
    ui?.devices.find((d) => d.id === selectedId) ?? ui?.devices[0] ?? null,
  );
  /** Monitor feeds of the selected device — they live in its Monitors strip. */
  const monitorRouteIds = $derived(
    new Set(
      (ui?.routes ?? [])
        .filter(
          (r) =>
            selected != null &&
            "VirtualMonitor" in r.source &&
            r.source.VirtualMonitor === selected.id,
        )
        .map((r) => r.id),
    ),
  );
  const serverOffline = $derived(
    ui !== null && ui.sinks.length === 0 && ui.sources.length === 0,
  );

  onMount(() => {
    void start();
    return stop;
  });
</script>

<div class="app">
  <DeviceSidebar {ui} {selectedId} onselect={(id) => (selectedId = id)} />

  <div class="main">
    {#if ui && ui.drift.length > 0}
      <aside class="drift" role="alert">
        <div class="drift-head">
          <strong class="drift-title">
            {ui.drift.length} item{ui.drift.length === 1 ? "" : "s"} out of
            sync with the audio server
          </strong>
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

    {#if serverOffline}
      <aside class="drift offline" role="alert">
        <strong class="drift-title">
          the audio server isn't answering. fio picks up when it's back
        </strong>
      </aside>
    {/if}

    <div class="canvas">
      {#if !ui}
        <div class="blank">
          <p>connecting to the audio server…</p>
        </div>
      {:else if selected}
        <DeviceCanvas {ui} device={selected} />
      {:else}
        <div class="blank">
          <svg
            viewBox="0 0 20 20"
            width="34"
            height="34"
            aria-hidden="true"
            class="blank-icon"
          >
            <rect
              x="1.5"
              y="1.5"
              width="17"
              height="17"
              rx="4.5"
              fill="none"
              stroke="currentColor"
              stroke-width="1.2"
            />
            <path
              d="M6.4 13.4V8.8M10 14.6V5.6M13.6 12.4V7.8"
              fill="none"
              stroke="currentColor"
              stroke-width="1.2"
              stroke-linecap="round"
            />
          </svg>
          <p class="blank-title">no virtual devices yet</p>
          <p class="blank-sub">
            press <span class="kbd">+</span> in the sidebar. Each device shows
            up as a speaker (<span class="mono">fio-*</span>) and a mic (<span
              class="mono">fio-*.monitor</span
            >) in your audio settings
          </p>
        </div>
      {/if}
    </div>

    {#if ui}
      <Outputs {ui} hideRouteIds={monitorRouteIds} />
    {/if}

    <Settings />
  </div>

  {#if fio.error}
    <div class="toast" role="alert">
      <span class="toast-msg">{fio.error}</span>
      <button
        class="chip-x"
        aria-label="dismiss error"
        onclick={clearError}
      >
        <svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true">
          <path
            d="M1.5 1.5l7 7M8.5 1.5l-7 7"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linecap="round"
          />
        </svg>
      </button>
    </div>
  {/if}
</div>

<style>
  .app {
    display: grid;
    grid-template-columns: auto 1fr;
    height: 100vh;
    overflow: hidden;
  }

  .main {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px 20px 0;
    overflow-y: auto;
    min-width: 0;
  }

  .canvas {
    min-height: 0;
  }

  .drift {
    border: 1px solid var(--warn-border);
    border-radius: var(--r-inner);
    background: var(--warn-soft);
    padding: 10px 14px;
  }

  .drift.offline {
    border-color: var(--danger-border);
    background: var(--danger-soft);
  }

  .drift.offline .drift-title {
    color: var(--danger);
  }

  .drift-head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .drift-title {
    flex: 1;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--warn);
  }

  .drift-list {
    margin: 7px 0 0;
    padding-left: 18px;
    color: var(--text-dim);
    font-size: 12px;
  }

  .blank {
    height: 100%;
    min-height: 260px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
    color: var(--text-dim);
  }

  .blank-icon {
    color: var(--text-faint);
  }

  .blank-title {
    margin: 4px 0 0;
    font-size: 15px;
    font-weight: 650;
    color: var(--text);
  }

  .blank-sub {
    margin: 0;
    max-width: 44ch;
    font-size: 12.5px;
  }

  .kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 4px;
    background: var(--block);
    font-size: 11px;
    font-weight: 700;
    color: var(--text-dim);
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
    padding: 9px 9px 9px 14px;
    border: 1px solid var(--danger-border);
    border-radius: var(--r-inner);
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 12px;
    box-shadow: var(--shadow-pop);
  }

  .toast-msg {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .chip-x {
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
    color: var(--danger);
  }

  .chip-x:hover:not(:disabled) {
    background: var(--danger-border);
    color: var(--danger);
    border: none;
  }
</style>
