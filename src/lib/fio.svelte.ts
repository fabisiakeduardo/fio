/**
 * Frontend state + command bridge. The backend emits `fio-state-changed`
 * after every successful mutation; we listen once and re-fetch `ui_state`.
 * Command wrappers surface backend error strings into `fio.error`.
 *
 * Outside Tauri (plain `vite dev` in a browser, e.g. for design review
 * screenshots) a local mock state stands in so the whole UI stays
 * interactive; mutations mutate the mock and re-render.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RouteSource, RouteTarget, UiState } from "./types";
import { monitorName } from "./types";

const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const fio = $state({
  ui: null as UiState | null,
  loading: true,
  /** Last backend error message, shown in the toast. */
  error: null as string | null,
});

/* ------- browser-dev mock ------- */

const PHYS_SINK = "alsa_output.pci-0000_00_1f.3.analog-stereo";
const USB_MIC = "alsa_input.usb-Blue_Microphones_Yeti-00.analog-stereo";

let mockSeq = 100;
const mockId = (p: string) => `${p}-${++mockSeq}`;
const slugify = (name: string) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "device";

let mock = $state<UiState | null>(null);

function mockState(): UiState {
  return {
    devices: [
      { id: "dev-game", name: "Game", slug: "game" },
      { id: "dev-chat", name: "Chat", slug: "chat" },
    ],
    routes: [
      {
        id: "r-spotify",
        source: { AppName: "spotify" },
        target: { VirtualDevice: "dev-game" },
      },
      {
        id: "r-yeti",
        source: { CaptureDevice: USB_MIC },
        target: { VirtualDevice: "dev-game" },
      },
      {
        id: "r-discord",
        source: { AppName: "discord" },
        target: { VirtualDevice: "dev-chat" },
      },
      {
        id: "r-chatmon",
        source: { VirtualMonitor: "dev-chat" },
        target: { VirtualDevice: "dev-game" },
      },
      {
        id: "r-gamemon",
        source: { VirtualMonitor: "dev-game" },
        target: { PhysicalSink: PHYS_SINK },
      },
      {
        id: "r-obs",
        source: { AppName: "obs" },
        target: { PhysicalSink: PHYS_SINK },
      },
    ],
    sinks: [
      {
        index: 40,
        name: PHYS_SINK,
        description: "Built-in Audio Analog Stereo",
        module: 7,
      },
      {
        index: 55,
        name: "alsa_output.usb-SteelSeries-00.stereo-game",
        description: "SteelSeries Arctis Game",
        module: 9,
      },
      { index: 61, name: "fio-game", description: "fio Game", module: null },
      { index: 62, name: "fio-chat", description: "fio Chat", module: null },
    ],
    sources: [
      {
        index: 50,
        name: USB_MIC,
        description: "Yeti Stereo Microphone",
        module: 8,
        monitor_of_sink: null,
      },
      {
        index: 51,
        name: "alsa_input.pci-0000_00_1f.3.analog-stereo",
        description: "Built-in Audio Analog Stereo",
        module: 7,
        monitor_of_sink: null,
      },
      {
        index: 52,
        name: "fio-game.monitor",
        description: "Monitor of fio Game",
        module: null,
        monitor_of_sink: 61,
      },
      {
        index: 53,
        name: "fio-chat.monitor",
        description: "Monitor of fio Chat",
        module: null,
        monitor_of_sink: 62,
      },
      {
        index: 54,
        name: "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
        description: "Monitor of Built-in Audio",
        module: null,
        monitor_of_sink: 40,
      },
    ],
    live_streams: [
      { index: 100, app_name: "spotify", media_name: null, sink: 61 },
      { index: 101, app_name: "firefox", media_name: "YouTube", sink: 40 },
      { index: 102, app_name: "discord", media_name: null, sink: 62 },
    ],
    drift: [{ MissingLink: { route_id: "r-obs" } }],
    default_source: "fio-game.monitor",
    seen_apps: ["spotify", "discord", "firefox", "obs", "slack"],
  };
}

/* ------- real bridge ------- */

export async function refresh(): Promise<void> {
  if (!inTauri) {
    mock ??= mockState();
    fio.ui = mock;
    fio.loading = false;
    return;
  }
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
  if (!inTauri) return;
  unlisten = await listen("fio-state-changed", () => {
    void refresh();
  });
}

export function stop(): void {
  unlisten?.();
  unlisten = null;
}

/**
 * Run a mutating command, surfacing its error string on failure.
 * In the browser mock, `fake` mutates the mock state instead.
 */
async function cmd(
  real: () => Promise<unknown>,
  fake?: (ui: UiState) => void,
): Promise<boolean> {
  try {
    fio.error = null;
    if (inTauri) {
      await real();
    } else {
      fake?.(mock ?? (mock = mockState()));
    }
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
  cmd(
    () => invoke("create_device", { name }),
    (ui) => {
      const slug = slugify(name);
      ui.devices.push({ id: mockId("dev"), name, slug });
      ui.sinks.push({
        index: mockSeq + 40,
        name: `fio-${slug}`,
        description: `fio ${name}`,
        module: null,
      });
    },
  );

export const deleteDevice = (id: string) =>
  cmd(
    () => invoke("delete_device", { id }),
    (ui) => {
      ui.devices = ui.devices.filter((d) => d.id !== id);
      ui.routes = ui.routes.filter(
        (r) =>
          !(
            ("VirtualDevice" in r.target && r.target.VirtualDevice === id) ||
            ("VirtualMonitor" in r.source && r.source.VirtualMonitor === id)
          ),
      );
    },
  );

export const createRoute = (source: RouteSource, target: RouteTarget) =>
  cmd(
    () => invoke("create_route", { source, target }),
    (ui) => {
      ui.routes.push({ id: mockId("r"), source, target });
    },
  );

export const deleteRoute = (id: string) =>
  cmd(
    () => invoke("delete_route", { id }),
    (ui) => {
      ui.routes = ui.routes.filter((r) => r.id !== id);
      ui.drift = ui.drift.filter(
        (d) =>
          !(
            ("MissingTarget" in d && d.MissingTarget.route_id === id) ||
            ("MissingLink" in d && d.MissingLink.route_id === id)
          ),
      );
    },
  );

export const setDefaultMic = (deviceId: string) =>
  cmd(
    () => invoke("set_default_mic", { deviceId }),
    (ui) => {
      const d = ui.devices.find((d) => d.id === deviceId);
      if (d) ui.default_source = monitorName(d);
    },
  );

export const restoreDrift = () =>
  cmd(
    () => invoke("restore_drift"),
    (ui) => {
      ui.drift = [];
    },
  );

export const clearError = () => {
  fio.error = null;
};
