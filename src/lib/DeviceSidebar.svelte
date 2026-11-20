<script lang="ts">
  /**
   * Left rail: the brand mark, the virtual-device list with an inline
   * "+ new device" row, and per-row delete (two-step confirm). Rows are
   * real buttons; the selected device drives the canvas.
   */
  import { createDevice, deleteDevice } from "./fio.svelte";
  import { monitorName, sinkName, type UiState } from "./types";

  interface Props {
    ui: UiState | null;
    selectedId: string | null;
    onselect: (id: string) => void;
  }
  let { ui, selectedId, onselect }: Props = $props();

  let adding = $state(false);
  let newName = $state("");
  let confirmId = $state<string | null>(null);

  const counts = $derived(
    ui
      ? `${ui.devices.length} device${ui.devices.length === 1 ? "" : "s"} · ${ui.routes.length} route${ui.routes.length === 1 ? "" : "s"} · ${ui.live_streams.length} live`
      : "",
  );

  function openAdd() {
    adding = true;
    newName = "";
  }

  function submit() {
    const name = newName.trim();
    if (!name) return;
    void createDevice(name).then((ok) => {
      if (ok) {
        newName = "";
        adding = false;
      }
    });
  }

  function askDelete(id: string) {
    confirmId = confirmId === id ? null : id;
  }

  function doDelete(id: string) {
    confirmId = null;
    void deleteDevice(id);
  }
</script>

<aside class="rail" aria-label="virtual devices">
  <header class="brand">
    <span class="mark">fio</span>
    <span class="tag">virtual audio patchbay</span>
  </header>

  <div class="list-head">
    <span class="sec-label">Devices</span>
    <button
      class="plus"
      aria-label="new virtual device"
      title="new virtual device"
      onclick={openAdd}
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

  {#if adding}
    <form
      class="new-row"
      onsubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <input
        aria-label="new device name"
        placeholder="Name — e.g. Game, Chat"
        bind:value={newName}
      />
      <button class="primary" type="submit" disabled={newName.trim() === ""}>
        add
      </button>
    </form>
  {/if}

  <nav class="list">
    {#if !ui}
      <p class="empty">loading…</p>
    {:else if ui.devices.length === 0}
      <p class="empty">
        no devices yet — each one appears as a speaker and a mic in your
        audio settings
      </p>
    {:else}
      {#each ui.devices as d (d.id)}
        {#if confirmId === d.id}
          <div class="row confirm">
            <span class="confirm-text">Delete “{d.name}”?</span>
            <button class="danger" onclick={() => doDelete(d.id)}>delete</button>
            <button onclick={() => (confirmId = null)}>keep</button>
          </div>
        {:else}
          <div class="row" class:sel={selectedId === d.id}>
            <button
              class="row-main"
              onclick={() => onselect(d.id)}
              aria-current={selectedId === d.id}
            >
              <svg
                class="dev-icon"
                viewBox="0 0 14 14"
                width="14"
                height="14"
                aria-hidden="true"
              >
                <rect
                  x="1.2"
                  y="1.2"
                  width="11.6"
                  height="11.6"
                  rx="2.6"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.2"
                />
                <path
                  d="M4.6 9.4V6.2M7 10.4V4.4M9.4 8.6V5.6"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.2"
                  stroke-linecap="round"
                />
              </svg>
              <span class="row-text">
                <span class="name">{d.name}</span>
                <span class="sub mono">{sinkName(d)}</span>
              </span>
              {#if ui.default_source === monitorName(d)}
                <span class="mic-dot" title="default mic" aria-hidden="true"></span>
              {/if}
            </button>
            <button
              class="row-x"
              aria-label="delete device {d.name}"
              title="delete device"
              onclick={() => askDelete(d.id)}
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
      {/each}
    {/if}
  </nav>

  {#if counts}
    <footer class="rail-foot">{counts}</footer>
  {/if}
</aside>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    width: 236px;
    flex: none;
    background: var(--bg-rail);
    border-right: 1px solid var(--border);
    min-height: 0;
  }

  .brand {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 16px 14px 12px;
    border-bottom: 1px solid var(--border-soft);
  }

  .mark {
    font-size: 19px;
    font-weight: 700;
    letter-spacing: -0.03em;
    color: var(--text);
  }

  .tag {
    font-size: 10.5px;
    color: var(--text-faint);
  }

  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 14px 6px;
  }

  .plus {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 6px;
    color: var(--text-dim);
  }

  .new-row {
    display: flex;
    gap: 6px;
    padding: 4px 10px 8px;
  }

  .new-row input {
    flex: 1;
  }

  .list {
    flex: 1;
    overflow-y: auto;
    padding: 2px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .empty {
    margin: 6px 4px;
    color: var(--text-faint);
    font-size: 12px;
    max-width: 26ch;
  }

  .row {
    display: flex;
    align-items: center;
    border-radius: 7px;
  }

  .row.sel {
    background: var(--accent-soft);
  }

  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 8px;
    border: none;
    background: none;
    text-align: left;
    border-radius: 7px;
  }

  .row-main:hover:not(:disabled) {
    border: none;
    color: inherit;
  }

  .row:not(.sel):hover {
    background: rgba(255, 255, 255, 0.65);
  }

  .row.sel .row-main {
    color: var(--accent-deep);
  }

  .dev-icon {
    flex: none;
    color: var(--text-faint);
  }

  .row.sel .dev-icon {
    color: var(--accent);
  }

  .row-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .name {
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sub {
    color: var(--text-faint);
    font-size: 10px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .row.sel .sub {
    color: var(--accent-deep);
    opacity: 0.75;
  }

  .mic-dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }

  .row-x {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    margin-right: 6px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text-faint);
    opacity: 0;
    transition: opacity 0.12s, background 0.12s, color 0.12s;
  }

  .row:hover .row-x,
  .row-x:focus-visible {
    opacity: 1;
  }

  .row-x:hover:not(:disabled) {
    background: var(--danger-soft);
    color: var(--danger);
    border: none;
  }

  .confirm {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 8px;
    background: var(--danger-soft);
    border: 1px solid var(--danger-border);
  }

  .confirm-text {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    color: var(--danger);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .confirm button {
    padding: 1px 7px;
    font-size: 11px;
  }

  .rail-foot {
    padding: 8px 14px;
    border-top: 1px solid var(--border-soft);
    font-size: 10.5px;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
