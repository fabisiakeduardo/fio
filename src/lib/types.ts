/**
 * Types mirroring the Rust-side `UiState` contract (src-tauri/src/commands.rs).
 * Serde externally-tagged enums serialize as `{ Variant: payload }` and keep
 * snake_case field names.
 */

export type RouteSource =
  | { AppName: string }
  | { CaptureDevice: string }
  | { VirtualMonitor: string };

export type RouteTarget =
  | { VirtualDevice: string }
  | { PhysicalSink: string };

export interface VirtualDevice {
  id: string;
  name: string;
  slug: string;
}

export interface Route {
  id: string;
  source: RouteSource;
  target: RouteTarget;
}

export interface SinkInfo {
  index: number;
  name: string;
  description: string | null;
  module: number | null;
}

export interface SourceInfo {
  index: number;
  name: string;
  description: string | null;
  module: number | null;
  monitor_of_sink: number | null;
}

export interface StreamInfo {
  index: number;
  app_name: string | null;
  media_name: string | null;
  sink: number;
}

export type Drift =
  | { MissingDevice: { device_id: string } }
  | { MissingTarget: { route_id: string } }
  | { MissingLink: { route_id: string } };

export interface UiState {
  devices: VirtualDevice[];
  routes: Route[];
  sinks: SinkInfo[];
  sources: SourceInfo[];
  live_streams: StreamInfo[];
  drift: Drift[];
  default_source: string | null;
  seen_apps: string[];
}

/* ------- helpers ------- */

export const sinkName = (d: VirtualDevice): string => `fio-${d.slug}`;
export const monitorName = (d: VirtualDevice): string => `${sinkName(d)}.monitor`;

/** Sink name a route target resolves to — `null` for deleted devices. */
export function targetSinkName(target: RouteTarget, ui: UiState): string | null {
  if ("PhysicalSink" in target) return target.PhysicalSink;
  const d = ui.devices.find((d) => d.id === target.VirtualDevice);
  return d ? sinkName(d) : null;
}
export const isVirtualSinkName = (name: string): boolean => name.startsWith("fio-");

/** Rule key a stream matches against: application.name, else media.name. */
export const streamLabel = (s: StreamInfo): string =>
  s.app_name ?? s.media_name ?? `stream #${s.index}`;

/** Physical playback outputs: every sink that isn't one of ours. */
export const physicalSinks = (ui: UiState): SinkInfo[] =>
  ui.sinks.filter((s) => !isVirtualSinkName(s.name));

/** Physical capture devices: sources that are not monitors of some sink. */
export const captureSources = (ui: UiState): SourceInfo[] =>
  ui.sources.filter((s) => s.monitor_of_sink == null);

export function sinkNameForIndex(ui: UiState, index: number): string {
  const s = ui.sinks.find((s) => s.index === index);
  return s ? (s.description ?? s.name) : `#${index}`;
}

export function sourceLabel(source: RouteSource, ui: UiState): string {
  if ("AppName" in source) return `app "${source.AppName}"`;
  if ("CaptureDevice" in source) {
    const s = ui.sources.find((s) => s.name === source.CaptureDevice);
    return s?.description ?? source.CaptureDevice;
  }
  const d = ui.devices.find((d) => d.id === source.VirtualMonitor);
  return d ? `${d.name} monitor` : "deleted device monitor";
}

export function targetLabel(target: RouteTarget, ui: UiState): string {
  if ("VirtualDevice" in target) {
    const d = ui.devices.find((d) => d.id === target.VirtualDevice);
    return d ? d.name : "deleted device";
  }
  const s = ui.sinks.find((s) => s.name === target.PhysicalSink);
  return s?.description ?? target.PhysicalSink;
}

export const routeLabel = (r: Route, ui: UiState): string =>
  `${sourceLabel(r.source, ui)} → ${targetLabel(r.target, ui)}`;

export const sameSource = (a: RouteSource, b: RouteSource): boolean =>
  JSON.stringify(a) === JSON.stringify(b);
export const sameTarget = (a: RouteTarget, b: RouteTarget): boolean =>
  JSON.stringify(a) === JSON.stringify(b);

/* ------- drift ------- */

export function deviceDrifted(ui: UiState, deviceId: string): boolean {
  return ui.drift.some(
    (d) => "MissingDevice" in d && d.MissingDevice.device_id === deviceId,
  );
}

export type RouteDrift = "missing end" | "missing link";

export function routeDrift(ui: UiState, routeId: string): RouteDrift | null {
  for (const d of ui.drift) {
    if ("MissingTarget" in d && d.MissingTarget.route_id === routeId)
      return "missing end";
    if ("MissingLink" in d && d.MissingLink.route_id === routeId)
      return "missing link";
  }
  return null;
}

export function driftMessage(d: Drift, ui: UiState): string {
  if ("MissingDevice" in d) {
    const dev = ui.devices.find((x) => x.id === d.MissingDevice.device_id);
    return `device "${dev?.name ?? d.MissingDevice.device_id}" is absent from the server`;
  }
  const route = ui.routes.find((r) =>
    "MissingTarget" in d
      ? r.id === d.MissingTarget.route_id
      : r.id === d.MissingLink.route_id,
  );
  const label = route ? routeLabel(route, ui) : "unknown route";
  return "MissingTarget" in d
    ? `route end not on the server: ${label}`
    : `loopback link not loaded: ${label}`;
}
