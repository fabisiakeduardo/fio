//! Owned, serializable-friendly snapshots of the audio server's actual state.
//!
//! These mirror (a small subset of) the fields libpulse returns from its
//! `Introspector` list calls, converted to owned `String`s so the results can
//! leave the callback context and cross thread boundaries freely.

use serde::Serialize;

/// A sink known to the server (physical output or virtual device).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SinkInfo {
    /// Server-assigned index. Per-session only — never persist.
    pub index: u32,
    /// Stable name, e.g. `alsa_output.pci-....analog-stereo` or `fio-<slug>`.
    pub name: String,
    /// Human-readable description, if set.
    pub description: Option<String>,
    /// Index of the owning module, if loaded by one.
    pub module: Option<u32>,
}

/// A source known to the server (physical input or a sink's monitor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceInfo {
    /// Server-assigned index. Per-session only — never persist.
    pub index: u32,
    /// Stable name, e.g. `alsa_input.usb-....mono-fallback` or
    /// `<sink name>.monitor`.
    pub name: String,
    /// Human-readable description, if set.
    pub description: Option<String>,
    /// Index of the owning module, if loaded by one.
    pub module: Option<u32>,
    /// Index of the sink this source monitors, if it is a monitor source.
    /// This is how a `module-null-sink`'s virtual mic end is recognized.
    pub monitor_of_sink: Option<u32>,
}

/// A playback stream (`sink-input`) — one app currently producing audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamInfo {
    /// Server-assigned index. Per-session only — never persist.
    pub index: u32,
    /// `application.name` proplist entry — the routing-rule key.
    pub app_name: Option<String>,
    /// `media.name` proplist entry.
    pub media_name: Option<String>,
    /// Index of the sink the stream currently plays to.
    pub sink: u32,
}

/// A module loaded in the server (`module-null-sink`, `module-loopback`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleInfo {
    /// Server-assigned index. Per-session only — never persist.
    pub index: u32,
    /// Module name, e.g. `module-null-sink`.
    pub name: String,
    /// Argument string the module was loaded with, if any. For modules Fio
    /// owns this carries the `sink_name=`/`source_name=` keys used to adopt
    /// them on restart.
    pub argument: Option<String>,
}

/// What the server actually has, right now — the "actual" side of the
/// desired-vs-actual reconcile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ActualState {
    pub sinks: Vec<SinkInfo>,
    pub sources: Vec<SourceInfo>,
    pub sink_inputs: Vec<StreamInfo>,
    pub modules: Vec<ModuleInfo>,
}
