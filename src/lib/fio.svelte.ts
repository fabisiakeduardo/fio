/**
 * Frontend state + command bridge. The backend emits `fio-state-changed`
 * after every successful mutation; we listen once and re-fetch `ui_state`.
 * Command wrappers surface backend error strings into `fio.error`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RouteSource, RouteTarget, UiState } from "./types";

export const fio = $state({
  ui: null as UiState | null,
  loading: true,
  /** Last backend error message, shown in the toast. */
  error: null as string | null,
});

export async function refresh(): Promise<void> {
  try {
    fio.ui = await invoke<UiState>("ui_state");
  } catch (e) {
    fio.error = `ui_state failed: ${e}`;
  } finally {
    fio.loading = false;
  }
}

let unlisten: UnlistenFn | null = null;

export async function start(): Promise<void> {
  await refresh();
  unlisten = await listen("fio-state-changed", () => {
    void refresh();
  });
}

export function stop(): void {
  unlisten?.();
  unlisten = null;
}

/** Run a mutating command, surfacing its error string on failure. */
async function cmd(promise: Promise<unknown>): Promise<boolean> {
  try {
    fio.error = null;
    await promise;
    return true;
  } catch (e) {
    fio.error = String(e);
    return false;
  } finally {
    // `fio-state-changed` only fires on success — refetch here too so a
    // degraded backend still converges the view.
    void refresh();
  }
}

export const createDevice = (name: string) =>
  cmd(invoke("create_device", { name }));
export const deleteDevice = (id: string) => cmd(invoke("delete_device", { id }));
export const createRoute = (source: RouteSource, target: RouteTarget) =>
  cmd(invoke("create_route", { source, target }));
export const deleteRoute = (id: string) => cmd(invoke("delete_route", { id }));
export const setDefaultMic = (deviceId: string) =>
  cmd(invoke("set_default_mic", { deviceId }));
export const restoreDrift = () => cmd(invoke("restore_drift"));
export const clearError = () => {
  fio.error = null;
};
