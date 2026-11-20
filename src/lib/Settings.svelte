<script lang="ts">
  /**
   * App-level settings strip at the bottom of the window: the "launch
   * at login" toggle (XDG autostart .desktop under the hood) plus a
   * reminder of the close-vs-quit lifecycle.
   */
  import { onMount } from "svelte";
  import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { fio } from "./fio.svelte";

  let autostart = $state(false);
  /** Checkbox stays disabled until the real backend state is known. */
  let ready = $state(false);

  const inTauri =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

  onMount(async () => {
    if (!inTauri) {
      ready = true;
      return;
    }
    try {
      autostart = await isEnabled();
    } catch (e) {
      fio.error = `autostart check failed: ${e}`;
    } finally {
      ready = true;
    }
  });

  async function toggle() {
    if (!inTauri) return;
    try {
      if (autostart) {
        await enable();
      } else {
        await disable();
      }
    } catch (e) {
      // Backend call failed — put the checkbox back.
      autostart = !autostart;
      fio.error = `autostart failed: ${e}`;
    }
  }
</script>

<footer class="foot">
  <label class="setting">
    <input
      type="checkbox"
      bind:checked={autostart}
      disabled={!ready}
      onchange={toggle}
    />
    launch at login
  </label>
  <span class="foot-hint">
    closing the window keeps fio in the tray — quit from the tray icon;
    your devices keep working
  </span>
</footer>

<style>
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 10px 2px;
    margin-top: auto;
    border-top: 1px solid var(--border);
  }

  .setting {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: pointer;
    user-select: none;
  }

  .setting input {
    accent-color: var(--accent);
    margin: 0;
  }

  .foot-hint {
    font-size: 11px;
    color: var(--text-faint);
    text-align: right;
  }
</style>
