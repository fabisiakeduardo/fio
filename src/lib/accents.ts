/**
 * Source-kind accents: each routed source block takes a distinct colour,
 * and its wires inherit it. Hues are deep enough that white text stays
 * legible on the solid block.
 */
import type { RouteSource } from "./types";

export type SourceKind = "app" | "capture" | "monitor";

export function sourceKind(s: RouteSource): SourceKind {
  if ("AppName" in s) return "app";
  if ("CaptureDevice" in s) return "capture";
  return "monitor";
}

export const ACCENT: Record<SourceKind, { base: string; soft: string }> = {
  app: { base: "#b75d17", soft: "#faece0" },
  capture: { base: "#2f6fb8", soft: "#e4edf8" },
  monitor: { base: "#7b53b8", soft: "#efe9f9" },
};

export const sourceAccent = (s: RouteSource) => ACCENT[sourceKind(s)];

export const KIND_LABEL: Record<SourceKind, string> = {
  app: "app rule",
  capture: "capture device",
  monitor: "monitor feed",
};

export const kindLabel = (s: RouteSource) => KIND_LABEL[sourceKind(s)];
