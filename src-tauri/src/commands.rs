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
//! returns `Err("audio server unavailable")`.

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
    Action, Drift, Driver, detect_drift, link_names, link_present, plan_new_stream, plan_startup,
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

/// Owns the app's backend for the duration of the Tauri runtime: one
/// [`PulseEngine`] connection and the [`Driver`] that reconciles the
/// store against it. Managed via `.manage()` in `run()`.
///
/// `engine`/`driver`/`store` are tri-state by construction: a live
/// driver owns the store, so `store` here is `Some` only in the degraded
/// (no-server) mode where commands still persist mutations.
pub struct Manager {
    engine: Option<Arc<PulseEngine>>,
    driver: Option<Driver>,
    store: Option<Mutex<Store>>,
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
        match PulseEngine::start() {
            Ok(engine) => {
                let engine = Arc::new(engine);
                let driver = Driver::new(Store::load(path), Arc::clone(&engine));
                Self {
                    engine: Some(engine),
                    driver: Some(driver),
                    store: None,
                }
            }
            Err(e) => {
                eprintln!(
                    "fio: audio server unavailable ({e}); starting degraded — \
                     changes are saved but nothing is applied or enforced"
                );
                Self {
                    engine: None,
                    driver: None,
                    store: Some(Mutex::new(Store::load(path))),
                }
            }
        }
    }

    /// Runs `f` against the store — via the driver when live, via the
    /// manager's own mutex in degraded mode. `f` must never call the
    /// engine.
    fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        match (&self.driver, &self.store) {
            (Some(driver), _) => driver.with_store(f),
            (None, Some(store)) => {
                let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
                f(&mut store)
            }
            (None, None) => unreachable!("manager always holds a store or a driver"),
        }
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
        let actual = self
            .engine
            .as_deref()
            .and_then(|e| e.actual_state().ok())
            .unwrap_or_default();
        let default_source = self
            .engine
            .as_deref()
            .and_then(|e| e.server_defaults().ok())
            .and_then(|d| d.default_source);
        let drift = match &self.driver {
            Some(driver) => driver.drift(),
            // No driver: report drift ourselves — with no server,
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
        if let Some(engine) = self.engine.as_deref()
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
        if let Some(engine) = self.engine.as_deref() {
            let actual = engine
                .actual_state()
                .map_err(|e| format!("failed to snapshot audio server state: {e}"))?;
            self.rescue_streams(engine, &actual, &sink_name);
            for (source, sink) in &doomed_links {
                unload_loopback(engine, &actual, source, sink);
            }
            // Unload failure leaves the sink live while the store
            // would forget the device — orphaning it. Abort instead;
            // the user can retry.
            unload_null_sink(engine, &actual, &sink_name)
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

        let rescues = self
            .driver
            .as_ref()
            .map(Driver::rescues)
            .unwrap_or_default();
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
            if let Some(driver) = &self.driver {
                driver.take_rescue(stream_index);
            }
        }
    }

    /// Creates a route: persists it, then enforces it immediately —
    /// matching live streams move (AppName rules) or the loopback link
    /// loads (capture/monitor sources). Unresolvable ends are not
    /// errors: they surface as drift, same as the startup heal treats
    /// them.
    pub fn create_route(&self, source: RouteSource, target: RouteTarget) -> Result<Route, String> {
        let route = self.with_store(|store| store.add_route(source, target));
        if let (Some(engine), Some(driver)) = (self.engine.as_deref(), self.driver.as_ref())
            && let Ok(actual) = engine.actual_state()
        {
            let desired = self.desired();
            match &route.source {
                RouteSource::AppName(_) => {
                    for stream in &actual.sink_inputs {
                        if let Some(action) = plan_new_stream(stream, &desired, &actual) {
                            let _ = driver.apply(action);
                        }
                    }
                }
                _ => enforce_link_route(driver, &desired, &actual, &route),
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
        let actual = self
            .engine
            .as_deref()
            .map(PulseEngine::actual_state)
            .transpose()
            .map_err(|e| format!("failed to snapshot audio server state: {e}"))?;

        self.with_store(|store| {
            store.remove_route(id);
        });
        if let (Some(engine), Some(actual)) = (self.engine.as_deref(), actual.as_ref()) {
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
        self.engine
            .as_deref()
            .ok_or_else(|| "audio server unavailable".to_string())?
            .set_default_source(&monitor)
            .map_err(|e| format!("failed to set {monitor} as default source: {e}"))
    }

    /// The "restore" action behind the UI's drift banner: re-runs the
    /// startup heal — adopts or reloads missing `fio-*` sinks, re-applies
    /// matching rules to live streams, reloads missing loopback links.
    pub fn restore_drift(&self) -> Result<(), String> {
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| "audio server unavailable".to_string())?;
        let engine = self.engine.as_deref().expect("driver implies engine");
        let actual = engine.actual_state().map_err(|e| e.to_string())?;
        for action in plan_startup(&self.desired(), &actual) {
            // Best effort, same as the startup heal — a failed action is
            // reported as drift, not retried here.
            let _ = driver.apply(action);
        }
        driver.recheck_drift();
        Ok(())
    }

    fn recheck_drift(&self) {
        if let Some(driver) = &self.driver {
            driver.recheck_drift();
        }
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
/// - names that slugify to nothing ("!!!" → `fio-` is no sink name).
fn validate_device_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("device name must not be empty".to_string());
    }
    if name.contains('\'') {
        return Err("device name must not contain a single quote (')".to_string());
    }
    if slugify(name).is_empty() {
        return Err("device name must contain at least one letter or digit".to_string());
    }
    Ok(())
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
}
