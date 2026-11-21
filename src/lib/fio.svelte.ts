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
import { monitorName, targetSinkName } from "./types";

const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const fio = $state({
  ui: null as UiState | null,
  loading: true,
  /** Last backend error message, shown in the toast. */
  error: null as string | null,
  /**
   * Latest `fio-levels` payload (~8 Hz): `dev:{id}` device output,
   * `route:{id}` route audio, `mon:{sink}` monitor-feed destination —
   * each 0..1. Missing keys render flat via {@link levelOf}.
   */
  levels: {} as Record<string, number>,
});

/** Current meter reading for an `fio-levels` key — 0 when unmetered. */
export const levelOf = (key: string): number => fio.levels[key] ?? 0;

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
let unlistenLevels: UnlistenFn | null = null;
let mockLevelTimer: ReturnType<typeof setInterval> | null = null;

/**
 * Browser-mock levels: sine-ish waves per visible key so the dev-server
 * demo shows the meters moving without a server.
 */
function startMockLevels(): void {
  let t = 0;
  mockLevelTimer = setInterval(() => {
    t += 0.12;
    const ui = mock;
    if (!ui) return;
    const wave = (f: number, phase: number, base = 0.55) =>
      Math.max(
        0,
        Math.min(
          1,
          base +
            0.35 * Math.sin(t * f + phase) +
            0.08 * Math.sin(t * f * 2.7 + phase * 1.3),
        ),
      );
    const levels: Record<string, number> = {};
    ui.devices.forEach((d, i) => {
      levels[`dev:${d.id}`] = wave(0.9 + i * 0.5, i * 1.7);
    });
    ui.routes.forEach((r, i) => {
      levels[`route:${r.id}`] = wave(1.4 + i * 0.45, i * 2.3, 0.45);
      if ("VirtualMonitor" in r.source) {
        const sink = targetSinkName(r.target, ui);
        if (sink) levels[`mon:${sink}`] = wave(0.7 + i * 0.3, i * 1.1);
      }
    });
    fio.levels = levels;
  }, 120);
}

export async function start(): Promise<void> {
  await refresh();
  if (!inTauri) {
    startMockLevels();
    return;
  }
  unlisten = await listen("fio-state-changed", () => {
    void refresh();
  });
  unlistenLevels = await listen<Record<string, number>>("fio-levels", (e) => {
    fio.levels = e.payload;
  });
}

export function stop(): void {
  unlisten?.();
  unlisten = null;
  unlistenLevels?.();
  unlistenLevels = null;
  if (mockLevelTimer) {
    clearInterval(mockLevelTimer);
    mockLevelTimer = null;
  }
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
