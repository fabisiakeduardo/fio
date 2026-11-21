//! The Tauri command layer: [`Manager`] holds the engine + [`Driver`]
//! pair and exposes the operations the seven `#[tauri::command]`
//! wrappers call. Command wrappers are thin — they delegate, then emit
//! `fio-state-changed` so the UI re-queries `ui_state`; every mutating
//! command does real work in a `Manager` method so it stays testable
//! without an `AppHandle`.
//!
//! The manager never fails to construct: when no PulseAudio-compatible
//! server answers, it still serves persisted desired state (all of it
//! flagged as drift) and persists mutations; engine-dependent work
//! returns `Err("audio server unavailable")` until the driver's
//! reconnect lands — the banner's "fio will pick up when it's back" is
//! load-bearing, not a lie.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::audio::{
    ActualState, PulseEngine, SinkInfo, SourceInfo, StreamInfo, loopback_args, null_sink_args,
};
use crate::model::{
    PersistedState, Route, RouteSource, RouteTarget, Store, VirtualDevice, slugify,
};
use crate::reconcile::{
    Action, Drift, Driver, EngineSlot, detect_drift, link_names, link_present, plan_new_stream,
    plan_startup,
};

/// Everything the UI renders in one shot — persisted desired state plus
/// a live snapshot of the server. When the server isn't answering,
/// `sinks`/`sources`/`live_streams` are empty and every desired entity
/// shows up in `drift` rather than pretending to exist.
#[derive(Debug, Clone, Serialize)]
pub struct UiState {
    pub devices: Vec<VirtualDevice>,
    pub routes: Vec<Route>,
    pub sinks: Vec<SinkInfo>,
    pub sources: Vec<SourceInfo>,
    pub live_streams: Vec<StreamInfo>,
    pub drift: Vec<Drift>,
    /// Name of the server's current default source — `None` when the
    /// server isn't answering.
    pub default_source: Option<String>,
    pub seen_apps: Vec<String>,
}

/// The state file: `$XDG_CONFIG_HOME/fio/state.json`, falling back to
/// `~/.config` when `XDG_CONFIG_HOME` is unset or empty.
fn state_path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config.join("fio").join("state.json")
}

/// Owns the app's backend for the duration of the Tauri runtime: the
/// shared [`EngineSlot`] the command layer reads (the driver's event
/// thread clears it on context death and refills it on reconnect) and
/// the [`Driver`] that reconciles the store against it. Managed via
/// `.manage()` in `run()`.
///
/// The driver always runs — degraded mode is simply an empty slot.
/// Commands persist mutations regardless; engine-dependent work returns
/// `Err("audio server unavailable")` until a (re)connect lands.
pub struct Manager {
    engine: EngineSlot,
    driver: Driver,
}

impl Manager {
    /// Production constructor: engine + persisted state at
    /// [`state_path`]. Never fails — an unreachable audio server yields
    /// the degraded manager described in the module docs.
    pub fn new() -> Self {
        Self::at_path(state_path())
    }

    /// Constructor with an explicit state path — the seam tests (and
    /// any future alternate-config support) use.
    pub fn at_path(path: PathBuf) -> Self {
        let engine = match PulseEngine::start() {
            Ok(engine) => Some(Arc::new(engine)),
            Err(e) => {
                eprintln!(
                    "fio: audio server unavailable ({e}); starting degraded. \
                     changes are saved and applied once the server answers"
                );
                None
            }
        };
        let slot = Arc::new(Mutex::new(engine));
        let driver = Driver::new(Store::load(path), Arc::clone(&slot));
        Self {
            engine: slot,
            driver,
        }
    }

    /// Clones the live engine `Arc` out of the slot — `None` while the
    /// server is unreachable. Readers never hold the slot lock across an
    /// engine call, so a slow snapshot can't stall the reconnect writer.
    fn engine(&self) -> Option<Arc<PulseEngine>> {
        self.engine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Installs the app handle the driver uses to emit
    /// `fio-state-changed` when the engine connects or dies —
    /// connectivity changes aren't driven by a command, so nothing else
    /// would tell the UI to re-query `ui_state`. Called once from the
    /// Tauri setup hook.
    pub fn set_app_handle(&self, app: &AppHandle) {
        let app = app.clone();
        self.driver.set_notify(move || {
            let _ = app.emit("fio-state-changed", ());
        });
    }

    /// Runs `f` against the store — always via the driver, which owns it
    /// whether or not an engine is connected. `f` must never call the
    /// engine.
    fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        self.driver.with_store(f)
    }

    /// Clone of the persisted desired state (devices, routes, seen apps).
    pub fn desired(&self) -> PersistedState {
        self.with_store(|store| store.state().clone())
    }

    /// One `ui_state` snapshot: desired state + live server state +
    /// drift + server defaults. Best-effort — never fails; server
    /// trouble shows up as empty live lists and full drift.
    pub fn ui_state(&self) -> UiState {
        let desired = self.desired();
        let engine = self.engine();
        let actual = engine
            .as_deref()
            .and_then(|e| e.actual_state().ok())
            .unwrap_or_default();
        let default_source = engine
            .as_deref()
            .and_then(|e| e.server_defaults().ok())
            .and_then(|d| d.default_source);
        let drift = match &engine {
            Some(_) => self.driver.drift(),
            // No connection: report drift ourselves — with no server,
            // everything desired is by definition missing.
            None => detect_drift(&desired, &actual),
        };
        UiState {
            devices: desired.devices,
            routes: desired.routes,
            sinks: actual.sinks,
            sources: actual.sources,
            live_streams: actual.sink_inputs,
            drift,
            default_source,
            seen_apps: desired.seen_apps,
        }
    }

    /// Creates a virtual device: validates the name, persists it, and
    /// loads its `module-null-sink`. Rejects names that are
    /// empty/whitespace, contain `'` (it would break the
    /// `device.description='…'` modarg), or slugify to nothing.
    pub fn create_device(&self, name: String) -> Result<VirtualDevice, String> {
        validate_device_name(&name)?;
        let device = self.with_store(|store| store.add_device(name.trim().to_string()));
        if let Some(engine) = self.engine()
            && let Err(e) = engine.load_module(
                "module-null-sink",
                &null_sink_args(&device.name, &device.slug),
            )
        {
            // Roll the store back: the command errors, so no
            // `fio-state-changed` fires — a persisted-but-unloaded
            // device would be a ghost the UI never hears about.
            // (Persisting only after a successful load isn't possible:
            // the slug must come from `add_device`'s dedup, which IS the
            // store mutation.)
            self.with_store(|store| {
                store.remove_device(device.id);
            });
            return Err(format!("device sink failed to load: {e}"));
        }
        self.recheck_drift();
        Ok(device)
    }

    /// The delete-with-rescue flow, pinned by spec: move each live
    /// stream on the device's sink back to its recorded origin (or the
    /// current default sink when the origin is unknown — never leave a
    /// stream attached to a sink being unloaded), unload the sink's
    /// `module-null-sink`, then drop the device from the store (which
    /// also drops its routes, both directions — their loopback links are
    /// torn down first so no orphaned module keeps referencing dead
    /// endpoints).
    pub fn delete_device(&self, id: Uuid) -> Result<(), String> {
        let device = self
            .with_store(|store| store.state().devices.iter().find(|d| d.id == id).cloned())
            .ok_or_else(|| format!("no such device: {id}"))?;
        let sink_name = format!("fio-{}", device.slug);

        // Link names of routes that die with this device — resolved
        // while the device is still in desired state (`remove_device`
        // drops routes referencing it as target OR monitor source).
        let doomed_links: Vec<(String, String)> = {
            let desired = self.desired();
            desired
                .routes
                .iter()
                .filter(|r| {
                    r.target == RouteTarget::VirtualDevice(id)
                        || r.source == RouteSource::VirtualMonitor(id)
                })
                .filter_map(|r| link_names(&desired, r))
                .collect()
        };

        // Engine present but snapshot failed: bail BEFORE the store
        // mutation. Committing the removal while the sink and its
        // loopbacks stay loaded on the server would orphan them
        // invisibly — drift only reports desired-missing-from-actual.
        if let Some(engine) = self.engine() {
            let actual = engine
                .actual_state()
                .map_err(|e| format!("failed to snapshot audio server state: {e}"))?;
            self.rescue_streams(&engine, &actual, &sink_name);
            for (source, sink) in &doomed_links {
                unload_loopback(&engine, &actual, source, sink);
            }
            // Unload failure leaves the sink live while the store
            // would forget the device — orphaning it. Abort instead;
            // the user can retry.
            unload_null_sink(&engine, &actual, &sink_name)
                .map_err(|e| format!("failed to unload {sink_name}: {e}"))?;
        }

        self.with_store(|store| {
            store.remove_device(id);
        });
        self.recheck_drift();
        Ok(())
    }

    /// Moves every stream stranded on `sink_name` to its rescue target
    /// (recorded origin) or the default sink. Best-effort per stream —
    /// a stream that ended mid-delete just fails its move.
    fn rescue_streams(&self, engine: &PulseEngine, actual: &ActualState, sink_name: &str) {
        let Some(sink) = actual.sinks.iter().find(|s| s.name == sink_name) else {
            return;
        };
        let streams: Vec<StreamInfo> = actual
            .sink_inputs
            .iter()
            .filter(|s| s.sink == sink.index)
            .cloned()
            .collect();
        if streams.is_empty() {
            return;
        }

        let rescues = self.driver.rescues();
        let default_sink = engine.server_defaults().ok().and_then(|d| d.default_sink);

        for (stream_index, target_name) in
            resolve_delete_targets(&streams, &rescues, default_sink.as_deref())
        {
            // The name was picked from rescues or the default — resolve
            // it against the same snapshot; a rescue naming a since-
            // vanished sink falls back to the default.
            let target = actual
                .sinks
                .iter()
                .find(|s| s.name == target_name)
                .or_else(|| {
                    default_sink
                        .as_deref()
                        .and_then(|d| actual.sinks.iter().find(|s| s.name == d))
                });
            if let Some(sink) = target {
                let _ = engine.move_sink_input(stream_index, sink.index);
            }
            // Consumed either way: the entry is per-session bookkeeping
            // for a stream that's rescued or already gone.
            self.driver.take_rescue(stream_index);
        }
    }

    /// Creates a route: validates it, persists it, then enforces it
    /// immediately — matching live streams move (AppName rules) or the
    /// loopback link loads (capture/monitor sources). Unresolvable ends
    /// are not errors: they surface as drift, same as the startup heal
    /// treats them. Rejects unsafe endpoint names and monitor feedback
    /// loops — including cycles through other devices (see
    /// [`validate_route`]).
    pub fn create_route(&self, source: RouteSource, target: RouteTarget) -> Result<Route, String> {
        let desired = self.desired();
        validate_route(&desired, &source, &target)?;
        let route = self.with_store(|store| store.add_route(source, target));
        if let Some(engine) = self.engine()
            && let Ok(actual) = engine.actual_state()
        {
            let desired = self.desired();
            match &route.source {
                RouteSource::AppName(_) => {
                    for stream in &actual.sink_inputs {
                        if let Some(action) = plan_new_stream(stream, &desired, &actual) {
                            let _ = self.driver.apply(action);
                        }
                    }
                }
                _ => enforce_link_route(&self.driver, &desired, &actual, &route),
            }
        }
        Ok(route)
    }

    /// Deletes a route: stops enforcing it (store removal) and unloads
    /// the `module-loopback` instance carrying the route's
    /// `source=`/`sink=` pair, if loaded. Streams already moved stay
    /// where they are — they're still playing fine.
    pub fn delete_route(&self, id: Uuid) -> Result<(), String> {
        // Read-only first: a bad id reports "no such route" even when
        // the server is unreachable — and a nonexistent route needs no
        // snapshot at all.
        let route = self
            .with_store(|store| store.state().routes.iter().find(|r| r.id == id).cloned())
            .ok_or_else(|| format!("no such route: {id}"))?;

        // Snapshot BEFORE the store mutation: engine present but
        // unreachable must bail here — committing the removal while a
        // live module-loopback keeps streaming would orphan it
        // invisibly (same reason as delete_device).
        let engine = self.engine();
        let actual = engine
            .as_deref()
            .map(PulseEngine::actual_state)
            .transpose()
            .map_err(|e| format!("failed to snapshot audio server state: {e}"))?;

        self.with_store(|store| {
            store.remove_route(id);
        });
        if let (Some(engine), Some(actual)) = (engine.as_deref(), actual.as_ref()) {
            // Link names resolve fine after removal: only the route is
            // gone — the devices it referenced still exist.
            let desired = self.desired();
            if let Some((source, sink)) = link_names(&desired, &route) {
                unload_loopback(engine, actual, &source, &sink);
            }
        }
        self.recheck_drift();
        Ok(())
    }

    /// "Use as default mic": points the server's default source at the
    /// device's monitor (`fio-{slug}.monitor`).
    pub fn set_default_mic(&self, device_id: Uuid) -> Result<(), String> {
        let device = self
            .with_store(|store| {
                store
                    .state()
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .cloned()
            })
            .ok_or_else(|| format!("no such device: {device_id}"))?;
        let monitor = format!("fio-{}.monitor", device.slug);
        self.engine()
            .ok_or_else(|| "audio server unavailable".to_string())?
            .set_default_source(&monitor)
            .map_err(|e| format!("failed to set {monitor} as default source: {e}"))
    }

    /// The "restore" action behind the UI's drift banner: re-runs the
    /// startup heal — adopts or reloads missing `fio-*` sinks, re-applies
    /// matching rules to live streams, reloads missing loopback links.
    pub fn restore_drift(&self) -> Result<(), String> {
        let engine = self
            .engine()
            .ok_or_else(|| "audio server unavailable".to_string())?;
        let actual = engine.actual_state().map_err(|e| e.to_string())?;
        for action in plan_startup(&self.desired(), &actual) {
            // Best effort, same as the startup heal — a failed action is
            // reported as drift, not retried here.
            let _ = self.driver.apply(action);
        }
        self.driver.recheck_drift();
        Ok(())
    }

    fn recheck_drift(&self) {
        self.driver.recheck_drift();
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

/// Name validation for `create_device` — kept pure so the rules are
/// testable without a server. Rejects:
/// - empty / whitespace-only names,
/// - `'`, which would produce a malformed `device.description='…'` modarg
///   (slug sanitization doesn't cover the display name),
/// - control characters — an interior NUL aborts the process at the FFI
///   `CString` boundary (`panic = "abort"`),
/// - names that slugify to nothing ("!!!" → `fio-` is no sink name).
fn validate_device_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("device name must not be empty".to_string());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("device name must not contain control characters".to_string());
    }
    if name.contains('\'') {
        return Err("device name must not contain a single quote (')".to_string());
    }
    if slugify(name).is_empty() {
        return Err("device name must contain at least one letter or digit".to_string());
    }
    Ok(())
}

/// Route validation for `create_route` — kept pure so the rules are
/// testable without a server. Rejects:
/// - `CaptureDevice`/`PhysicalSink` names that would corrupt the
///   `module-loopback` arg string or its whole-token matching
///   ([`validate_link_endpoint`]),
/// - empty / control-char `AppName` rules,
/// - a `VirtualMonitor(id) → VirtualDevice(id)` self-feed, and
/// - any route whose monitor→device edge would close a feedback cycle
///   (`A.mon→B` + `B.mon→A` is two `module-loopback`s amplifying each
///   other to full-scale clipping).
fn validate_route(
    desired: &PersistedState,
    source: &RouteSource,
    target: &RouteTarget,
) -> Result<(), String> {
    match source {
        RouteSource::AppName(name) => validate_app_rule(name)?,
        RouteSource::CaptureDevice(name) => validate_link_endpoint(name)?,
        RouteSource::VirtualMonitor(_) => {}
    }
    if let RouteTarget::PhysicalSink(name) = target {
        validate_link_endpoint(name)?;
    }
    if let (RouteSource::VirtualMonitor(a), RouteTarget::VirtualDevice(b)) = (source, target) {
        if a == b {
            return Err("a device's monitor cannot feed the device itself".to_string());
        }
        // Adding a→b closes a loop exactly when b already reaches a over
        // the persisted monitor→device edges.
        if monitor_path_exists(desired, *b, *a) {
            return Err(
                "route would close a monitor feedback loop between virtual devices".to_string(),
            );
        }
    }
    Ok(())
}

/// Link endpoint names (`CaptureDevice`, `PhysicalSink`) are embedded
/// verbatim in the `module-loopback` arg string and matched back as
/// whole whitespace-separated tokens in `link_present`/`unload_loopback`
/// — whitespace or `'`/`=` would corrupt the `name=value` token and
/// orphan the loaded link, while control characters (NUL) panic the FFI
/// `CString` conversion — fatal under `panic = "abort"`.
fn validate_link_endpoint(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("route endpoint name must not be empty".to_string());
    }
    if name
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\'' | '='))
    {
        return Err(
            "route endpoint name must not contain whitespace, ', = or control characters"
                .to_string(),
        );
    }
    Ok(())
}

/// An `AppName` rule is exact-matched against stream properties — never
/// interpolated into module args — so spaces and `'` are legitimate.
/// Only emptiness and control characters are rejected.
fn validate_app_rule(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("app rule name must not be empty".to_string());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("app rule name must not contain control characters".to_string());
    }
    Ok(())
}

/// Is `to` reachable from `from` over the persisted
/// `VirtualMonitor → VirtualDevice` edges? A candidate edge `a→b` would
/// complete a feedback cycle exactly when `b` already reaches `a`.
fn monitor_path_exists(desired: &PersistedState, from: Uuid, to: Uuid) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        stack.extend(desired.routes.iter().filter_map(|r| {
            match (&r.source, &r.target) {
                (RouteSource::VirtualMonitor(a), RouteTarget::VirtualDevice(b)) if *a == id => {
                    Some(*b)
                }
                _ => None,
            }
        }));
    }
    false
}

/// Where each stream stranded on a deleted device's sink goes:
/// `rescues[stream]` — the sink the driver moved it off of — or, when
/// no origin is recorded, the current default sink (RF #4). Streams
/// with neither are skipped: there's nowhere safe to put them.
fn resolve_delete_targets(
    streams_on_vdev: &[StreamInfo],
    rescues: &HashMap<u32, String>,
    default_sink: Option<&str>,
) -> Vec<(u32, String)> {
    streams_on_vdev
        .iter()
        .filter_map(|s| {
            rescues
                .get(&s.index)
                .cloned()
                .or_else(|| default_sink.map(str::to_string))
                .map(|sink| (s.index, sink))
        })
        .collect()
}

/// Unloads every `module-null-sink` carrying `sink_name={sink_name}`.
/// Tokens are compared whole (like [`link_present`]) so `fio-game`
/// can't match `fio-game-2`. The first unload failure is returned —
/// continuing would orphan the sink once the store forgets it.
fn unload_null_sink(
    engine: &PulseEngine,
    actual: &ActualState,
    sink_name: &str,
) -> Result<(), crate::audio::EngineError> {
    let want = format!("sink_name={sink_name}");
    for module in actual.modules.iter().filter(|m| {
        m.name == "module-null-sink"
            && m.argument
                .as_deref()
                .is_some_and(|a| a.split_whitespace().any(|t| t == want))
    }) {
        engine.unload_module(module.index)?;
    }
    Ok(())
}

/// Unloads every `module-loopback` whose argument carries this exact
/// `source=`/`sink=` pair (whole-token match, same rule as
/// [`link_present`]). Best-effort per module — a link that already died
/// just isn't in `actual`.
fn unload_loopback(engine: &PulseEngine, actual: &ActualState, source: &str, sink: &str) {
    let want_source = format!("source={source}");
    let want_sink = format!("sink={sink}");
    for module in actual.modules.iter().filter(|m| {
        m.name == "module-loopback"
            && m.argument.as_deref().is_some_and(|a| {
                a.split_whitespace().any(|t| t == want_source)
                    && a.split_whitespace().any(|t| t == want_sink)
            })
    }) {
        let _ = engine.unload_module(module.index);
    }
}

/// Loads `route`'s `module-loopback` when both ends exist on the server
/// and the link isn't already there — the same conditions
/// `plan_startup` applies to link routes.
fn enforce_link_route(
    driver: &Driver,
    desired: &PersistedState,
    actual: &ActualState,
    route: &Route,
) {
    let Some((source, sink)) = link_names(desired, route) else {
        return;
    };
    let source_ok = actual.sources.iter().any(|s| s.name == source);
    let sink_ok = actual.sinks.iter().any(|s| s.name == sink);
    if source_ok && sink_ok && !link_present(actual, &source, &sink) {
        let _ = driver.apply(Action::LoadModule {
            name: "module-loopback".to_string(),
            argument: loopback_args(&source, &sink),
        });
    }
}

fn emit_state_changed(app: &AppHandle) {
    let _ = app.emit("fio-state-changed", ());
}

#[tauri::command]
pub fn ui_state(m: State<'_, Manager>) -> UiState {
    m.ui_state()
}

#[tauri::command]
pub fn create_device(
    m: State<'_, Manager>,
    app: AppHandle,
    name: String,
) -> Result<VirtualDevice, String> {
    let device = m.create_device(name)?;
    emit_state_changed(&app);
    Ok(device)
}

#[tauri::command]
pub fn delete_device(m: State<'_, Manager>, app: AppHandle, id: Uuid) -> Result<(), String> {
    m.delete_device(id)?;
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn create_route(
    m: State<'_, Manager>,
    app: AppHandle,
    source: RouteSource,
    target: RouteTarget,
) -> Result<Route, String> {
    let route = m.create_route(source, target)?;
    emit_state_changed(&app);
    Ok(route)
}

#[tauri::command]
pub fn delete_route(m: State<'_, Manager>, app: AppHandle, id: Uuid) -> Result<(), String> {
    m.delete_route(id)?;
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_default_mic(
    m: State<'_, Manager>,
    app: AppHandle,
    device_id: Uuid,
) -> Result<(), String> {
    m.set_default_mic(device_id)?;
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn restore_drift(m: State<'_, Manager>, app: AppHandle) -> Result<(), String> {
    m.restore_drift()?;
    emit_state_changed(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_stream(index: u32, sink: u32) -> StreamInfo {
        StreamInfo {
            index,
            app_name: None,
            media_name: None,
            sink,
        }
    }

    /// Held in Tauri state, called from command threads.
    #[test]
    fn manager_is_send_and_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<Manager>();
    }

    #[test]
    fn delete_uses_recorded_origin_when_available() {
        let streams = vec![mk_stream(10, 7), mk_stream(11, 7)];
        let mut rescues = HashMap::new();
        rescues.insert(10, "alsa_out".to_string());
        // 11 has no recorded origin — falls back to the default.
        let targets = resolve_delete_targets(&streams, &rescues, Some("fallback"));
        assert_eq!(
            targets,
            vec![(10, "alsa_out".to_string()), (11, "fallback".to_string()),]
        );
    }

    #[test]
    fn delete_falls_back_to_default_sink() {
        // RF #4: no recorded origin -> the current default sink.
        let streams = vec![mk_stream(10, 7)];
        let targets = resolve_delete_targets(&streams, &HashMap::new(), Some("default_sink"));
        assert_eq!(targets, vec![(10, "default_sink".to_string())]);
    }

    #[test]
    fn delete_skips_stream_with_nowhere_to_go() {
        // No rescue and no default sink (e.g. server info unavailable):
        // leave the stream — moving it somewhere arbitrary is worse.
        let streams = vec![mk_stream(10, 7)];
        assert!(resolve_delete_targets(&streams, &HashMap::new(), None).is_empty());
    }

    #[test]
    fn create_device_rejects_bad_names() {
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("   ").is_err());
        assert!(validate_device_name("it's broken").is_err());
        assert!(validate_device_name("!!!").is_err());
        assert!(validate_device_name("Game Mic").is_ok());
        assert!(validate_device_name("\"quoted\"").is_ok()); // only ' is dangerous
    }

    #[test]
    fn create_device_rejects_control_chars() {
        // An interior NUL aborts the process at the FFI CString boundary
        // under `panic = "abort"`; other control chars are just as hostile.
        assert!(validate_device_name("dev\u{0}ice").is_err());
        assert!(validate_device_name("dev\u{7}bell").is_err());
        assert!(validate_device_name("dev\tice").is_err());
        assert!(validate_device_name("dev\nice").is_err());
    }

    fn mk_vdev(slug: &str) -> VirtualDevice {
        VirtualDevice {
            id: Uuid::new_v4(),
            name: slug.to_string(),
            slug: slug.to_string(),
        }
    }

    fn mk_route(source: RouteSource, target: RouteTarget) -> Route {
        Route {
            id: Uuid::new_v4(),
            source,
            target,
        }
    }

    fn desired_with(devices: Vec<VirtualDevice>, routes: Vec<Route>) -> PersistedState {
        PersistedState {
            version: 1,
            devices,
            routes,
            seen_apps: vec![],
        }
    }

    #[test]
    fn create_route_rejects_monitor_self_feed() {
        let a = mk_vdev("a");
        let desired = desired_with(vec![a.clone()], vec![]);
        // VirtualMonitor(a) -> VirtualDevice(a): loopback of a device into
        // itself — instant feedback. The UI blocks this; the backend must too.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(a.id),
                &RouteTarget::VirtualDevice(a.id),
            )
            .is_err()
        );
    }

    #[test]
    fn create_route_rejects_two_device_monitor_cycle() {
        let a = mk_vdev("a");
        let b = mk_vdev("b");
        let desired = desired_with(
            vec![a.clone(), b.clone()],
            vec![mk_route(
                RouteSource::VirtualMonitor(a.id),
                RouteTarget::VirtualDevice(b.id),
            )],
        );
        // A.mon -> B already persisted: adding B.mon -> A closes the loop.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(b.id),
                &RouteTarget::VirtualDevice(a.id),
            )
            .is_err()
        );
        // The existing forward edge itself is fine.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(a.id),
                &RouteTarget::VirtualDevice(b.id),
            )
            .is_ok()
        );
    }

    #[test]
    fn create_route_rejects_longer_monitor_cycle() {
        let a = mk_vdev("a");
        let b = mk_vdev("b");
        let c = mk_vdev("c");
        let desired = desired_with(
            vec![a.clone(), b.clone(), c.clone()],
            vec![
                mk_route(
                    RouteSource::VirtualMonitor(a.id),
                    RouteTarget::VirtualDevice(b.id),
                ),
                mk_route(
                    RouteSource::VirtualMonitor(b.id),
                    RouteTarget::VirtualDevice(c.id),
                ),
            ],
        );
        // A.mon->B and B.mon->C exist: C.mon->A closes A->B->C->A.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(c.id),
                &RouteTarget::VirtualDevice(a.id),
            )
            .is_err()
        );
        // A.mon->C extends the chain without closing it — allowed.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(a.id),
                &RouteTarget::VirtualDevice(c.id),
            )
            .is_ok()
        );
    }

    #[test]
    fn create_route_allows_monitor_to_physical_sink() {
        let a = mk_vdev("a");
        let desired = desired_with(vec![a.clone()], vec![]);
        // Monitor feeds to real hardware can't cycle — they're leaves.
        assert!(
            validate_route(
                &desired,
                &RouteSource::VirtualMonitor(a.id),
                &RouteTarget::PhysicalSink("alsa_out".to_string()),
            )
            .is_ok()
        );
    }

    #[test]
    fn create_route_rejects_unsafe_link_endpoint_names() {
        let a = mk_vdev("a");
        let desired = desired_with(vec![a.clone()], vec![]);
        for bad in [
            "",
            "has space",
            "quo'te",
            "eq=ual",
            "nu\u{0}l",
            "tab\tbed",
            "new\nline",
            "trailing ",
            " leading",
        ] {
            assert!(
                validate_route(
                    &desired,
                    &RouteSource::CaptureDevice(bad.to_string()),
                    &RouteTarget::VirtualDevice(a.id),
                )
                .is_err(),
                "CaptureDevice({bad:?}) should be rejected"
            );
            assert!(
                validate_route(
                    &desired,
                    &RouteSource::AppName("app".to_string()),
                    &RouteTarget::PhysicalSink(bad.to_string()),
                )
                .is_err(),
                "PhysicalSink({bad:?}) should be rejected"
            );
        }
        // Normal pulse names pass.
        let ok_name = "alsa_output.pci-0000_00_1f.3.analog-stereo";
        assert!(
            validate_route(
                &desired,
                &RouteSource::CaptureDevice(ok_name.to_string()),
                &RouteTarget::VirtualDevice(a.id),
            )
            .is_ok()
        );
        assert!(
            validate_route(
                &desired,
                &RouteSource::AppName("app".to_string()),
                &RouteTarget::PhysicalSink(ok_name.to_string()),
            )
            .is_ok()
        );
    }

    #[test]
    fn create_route_appname_allows_spaces_rejects_control() {
        let a = mk_vdev("a");
        let desired = desired_with(vec![a.clone()], vec![]);
        // AppName rules are exact-matched against stream properties, never
        // interpolated into module args — spaces are legitimate.
        assert!(
            validate_route(
                &desired,
                &RouteSource::AppName("Music Player".to_string()),
                &RouteTarget::VirtualDevice(a.id),
            )
            .is_ok()
        );
        for bad in ["", "   ", "nu\u{0}l", "bell\u{7}"] {
            assert!(
                validate_route(
                    &desired,
                    &RouteSource::AppName(bad.to_string()),
                    &RouteTarget::VirtualDevice(a.id),
                )
                .is_err(),
                "AppName({bad:?}) should be rejected"
            );
        }
    }
}
