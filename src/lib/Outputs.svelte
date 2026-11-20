<script lang="ts">
  /**
   * Slim strip under the device canvas: every physical output and the
   * routes aimed at it. The selected device's own monitor feeds live in
   * its Monitors strip and are excluded here to keep one reading.
   */
  import { deleteRoute } from "./fio.svelte";
  import { sourceAccent } from "./accents";
  import {
    physicalSinks,
    routeDrift,
    sourceLabel,
    type Route,
    type UiState,
  } from "./types";

  interface Props {
    ui: UiState;
    /** Route ids already shown in the selected device's Monitors strip. */
    hideRouteIds: Set<string>;
  }
  let { ui, hideRouteIds }: Props = $props();

  const outputs = $derived(physicalSinks(ui));

  const routesTo = (name: string): Route[] =>
    ui.routes.filter(
      (r) =>
        "PhysicalSink" in r.target &&
        r.target.PhysicalSink === name &&
        !hideRouteIds.has(r.id),
    );
</script>

<section class="outputs" aria-label="outputs">
  <div class="sec-head">
    <h3 class="sec-label">Outputs</h3>
  </div>
  {#if outputs.length === 0}
    <p class="empty">no physical outputs found</p>
  {:else}
    <div class="out-list">
      {#each outputs as sink (sink.index)}
        {@const routes = routesTo(sink.name)}
        {@const playing = ui.live_streams.filter(
          (s) => s.sink === sink.index,
        ).length}
        <div class="out">
          <div class="out-id">
            <span class="out-name">{sink.description ?? sink.name}</span>
            <span class="out-sub mono">{sink.name}</span>
          </div>
          {#if playing > 0}
            <span class="playing">{playing} playing</span>
          {/if}
          {#if routes.length > 0}
            <div class="out-routes">
              {#each routes as r (r.id)}
                {@const accent = sourceAccent(r.source)}
                {@const drift = routeDrift(ui, r.id)}
                <span class="out-chip" class:drifted={drift !== null}>
                  <span
                    class="chip-dot"
                    style="background:{accent.base}"
                  ></span>
                  <span class="chip-label">{sourceLabel(r.source, ui)}</span>
                  {#if drift}
                    <span class="chip-drift">{drift}</span>
                  {/if}
                  <button
                    class="chip-x"
                    aria-label="delete route {sourceLabel(r.source, ui)}"
                    title="delete route"
                    onclick={() => void deleteRoute(r.id)}
                  >
                    <svg
                      viewBox="0 0 10 10"
                      width="8"
                      height="8"
                      aria-hidden="true"
                    >
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
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .outputs {
    background: var(--block);
    border: 1px solid var(--border);
    border-radius: var(--r-block);
    box-shadow: var(--shadow-block);
    padding: 12px 16px 14px;
  }

  .sec-head {
    margin-bottom: 9px;
  }

  .empty {
    margin: 2px 0;
    font-size: 12px;
    color: var(--text-faint);
  }

  .out-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .out {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 6px 10px;
    border: 1px solid var(--border-soft);
    border-radius: var(--r-inner);
    background: var(--block-sunk);
  }

  .out-id {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 1 1 220px;
  }

  .out-name {
    font-weight: 600;
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .out-sub {
    color: var(--text-faint);
    font-size: 10px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .playing {
    flex: none;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--accent-deep);
    background: var(--accent-soft);
    border-radius: 999px;
    padding: 1px 7px;
  }

  .out-routes {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }

  .out-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 4px 3px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--block);
    font-size: 11.5px;
    color: var(--text-dim);
    max-width: 300px;
  }

  .out-chip.drifted {
    border-color: var(--warn-border);
    background: var(--warn-soft);
  }

  .chip-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }

  .chip-label {
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .chip-drift {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--warn);
    flex: none;
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
