//! The reconcile driver: owns the [`Store`], the engine's event channel,
//! and the thread that wires [`PulseEvent`]s to the pure planners.
//!
//! Enforcement is deliberately asymmetric:
//!
//! - **Heal once per connection** — the event thread runs
//!   [`plan_startup`] after every successful attach (adopt or load
//!   `fio-*` sinks, move matching live streams, load missing loopbacks).
//! - **Mid-session enforce only on new streams** — a `NewSinkInput` event
//!   records the app name and runs [`plan_new_stream`], moving just that
//!   stream when a rule matches.
//! - **Everything else is drift** — module removals and changes recompute
//!   [`detect_drift`] and surface it via [`Driver::drift`]; the driver
//!   never auto-fixes something that was there and went away.
//! - **Reconnect is automatic** — a dead context (`NotReady`/`Timeout`
//!   from engine ops) is dropped from the shared engine slot and
//!   `PulseEngine::start()` retried on a backoff inside the same event
//!   loop, so a server restart self-heals instead of leaving enforcement
//!   inert — and a degraded launch picks up the server when it appears.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{ActualState, EngineError, MeterTarget, PulseEngine, PulseEvent};
use crate::model::{PersistedState, Store};

use super::{
    Action, Drift, detect_drift, meter_plan, plan_new_stream, plan_startup, rule_key,
};

/// How often the event loop recomputes [`detect_drift`] even when no
/// events arrive — drift surfaces within one tick rather than only on
/// the next server change.
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Backoff between reconnect attempts after the engine dies — each
/// attempt itself blocks up to the engine's connect timeout.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);
/// How often the loop checks the shutdown flag while disconnected —
/// bounds event-thread join time on drop.
const SHUTDOWN_POLL: Duration = Duration::from_millis(200);
/// Minimum interval between `seen_apps` disk writes — stream events
/// update memory immediately; the drift tick flushes at this cadence
/// and [`Driver::drop`] flushes once more, instead of writing per event.
const SEEN_PERSIST_INTERVAL: Duration = Duration::from_secs(5);
/// How often the event loop drains peak streams and emits `fio-levels`
/// — ~8 Hz. Also the `recv_timeout` granularity, so this doubles as the
/// upper bound on event latency: drift still recomputes only on
/// [`RECHECK_INTERVAL`].
const LEVELS_INTERVAL: Duration = Duration::from_millis(120);

/// Shared, swappable handle to the live engine connection: `None` while
/// the audio server is unreachable.
///
/// The driver's event thread is the only writer — it clears the slot
/// when the context dies and refills it on a [`RECONNECT_BACKOFF`]
/// retry. Readers (the command layer, [`Driver::apply`]) just clone the
/// `Arc` out; the lock is never held across an engine call, so a slow
/// snapshot can't stall them.
pub type EngineSlot = Arc<Mutex<Option<Arc<PulseEngine>>>>;

/// The `fio-levels` emitter: `dev:`/`route:`/`mon:` key → peak 0..1.
type LevelEmit = Arc<dyn Fn(HashMap<String, f32>) + Send + Sync>;

/// Mutable state shared between the driver (command path) and its event
/// thread. One mutex; critical sections are brief and the engine is never
/// called while it is held.
struct Shared {
    store: Store,
    /// stream_index → name of the sink the stream sat on before the
    /// driver last moved it. Task 7's device-delete flow consumes this to
    /// "rescue" streams off a device being removed.
    rescues: HashMap<u32, String>,
    /// Latest [`detect_drift`] result, refreshed by the event loop.
    drift: Vec<Drift>,
    /// Run once when the engine dies and once when it comes back —
    /// installed from the command layer so the UI re-queries `ui_state`
    /// on connectivity changes without waiting for a mutation.
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    /// `record_seen_app` writes are memory-only; this flags pending
    /// changes the drift tick flushes on [`SEEN_PERSIST_INTERVAL`].
    seen_dirty: bool,
    last_seen_persist: Instant,
    /// Meter endpoints the engine was asked to attach — endpoint key
    /// (`src:{name}`/`in:{index}`) → target. Cleared on attach (a fresh
    /// engine has no streams) and on connection death.
    attached_meters: HashMap<String, MeterTarget>,
    /// `fio-levels` key → endpoint key, from the latest [`meter_plan`].
    meter_keys: HashMap<String, String>,
    /// Emits `fio-levels` payloads (~8 Hz) — installed from the command
    /// layer, same pattern as `notify`.
    level_emit: Option<LevelEmit>,
}

/// Owns the [`Store`] and the event thread between it and the engine
/// slot. `Send + Sync` so it can sit in Tauri state next to the slot.
pub struct Driver {
    slot: EngineSlot,
    shared: Arc<Mutex<Shared>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Driver {
    /// Takes ownership of `store` and spawns the event thread, which owns
    /// the whole connection lifecycle: attach (or retry) → subscribe →
    /// heal → dispatch. The subscription is established *before* each
    /// heal pass — events that fire mid-heal sit in the channel and are
    /// handled right after, so no new stream escapes enforcement.
    ///
    /// When the slot is empty (no server at launch) the thread retries
    /// `PulseEngine::start()` on a [`RECONNECT_BACKOFF`] — the driver
    /// always runs, so commands persist mutations regardless and the
    /// first successful attach heals them.
    pub fn new(store: Store, slot: EngineSlot) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            store,
            rescues: HashMap::new(),
            drift: Vec::new(),
            notify: None,
            seen_dirty: false,
            last_seen_persist: Instant::now(),
            attached_meters: HashMap::new(),
            meter_keys: HashMap::new(),
            level_emit: None,
        }));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread = {
            let slot = Arc::clone(&slot);
            let shared = Arc::clone(&shared);
            let shutdown = Arc::clone(&shutdown);
            thread::Builder::new()
                .name("fio-events".to_string())
                .spawn(move || event_loop(&slot, &shared, &shutdown))
                .expect("failed to spawn fio event thread")
        };
        Self {
            slot,
            shared,
            shutdown,
            thread: Some(thread),
        }
    }

    /// The live engine, or `None` while the server is unreachable —
    /// degraded start or a dead context awaiting reconnect.
    fn engine(&self) -> Option<Arc<PulseEngine>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Installs the callback the event thread runs on engine connect /
    /// death — the command layer uses it to emit `fio-state-changed` so
    /// the UI reflects connectivity changes without a user mutation.
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .notify = Some(Arc::new(f));
    }

    /// Installs the `fio-levels` emitter the event thread calls ~8×/s
    /// with the latest peak map — levels are emitted, never persisted
    /// or folded into `UiState`, so the `ui_state` contract is untouched.
    pub fn set_level_emit(&self, f: impl Fn(HashMap<String, f32>) + Send + Sync + 'static) {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .level_emit = Some(Arc::new(f));
    }

    /// The latest drift report — recomputed on every module-removal /
    /// change event and on a [`RECHECK_INTERVAL`] tick.
    pub fn drift(&self) -> Vec<Drift> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drift
            .clone()
    }

    /// Executes one planned [`Action`] against the server. `MoveStream`
    /// resolves `sink_name` against a fresh snapshot — the name→index
    /// mapping is per-session and may have shifted since planning — and,
    /// on success, records the stream's previous sink in `rescues`.
    /// [`EngineError::NotReady`] while no engine is connected.
    pub fn apply(&self, a: Action) -> Result<(), EngineError> {
        let engine = self.engine().ok_or(EngineError::NotReady)?;
        apply_action(&engine, &self.shared, a)
    }

    /// Removes and returns the previous-sink name recorded for a stream
    /// the driver moved, if any. Consumed by Task 7's delete flow.
    pub fn take_rescue(&self, stream_index: u32) -> Option<String> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rescues
            .remove(&stream_index)
    }

    /// A snapshot of the whole stream→previous-sink rescue map — the
    /// delete flow resolves every stranded stream's target in one pass
    /// rather than taking entries one by one.
    pub fn rescues(&self) -> HashMap<u32, String> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rescues
            .clone()
    }

    /// Runs `f` against the store under the shared lock — the command
    /// layer's mutation/read path. Critical sections stay brief; `f` must
    /// never call back into the engine (the event loop applies actions
    /// only after releasing the lock).
    pub fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut shared.store)
    }

    /// A clone of the persisted desired state (devices, routes, seen
    /// apps) — what `ui_state` reports and plans run against.
    pub fn desired(&self) -> PersistedState {
        self.with_store(|store| store.state().clone())
    }

    /// Recomputes drift from a fresh snapshot right now. Called after
    /// command-path mutations so the UI's drift list reflects them
    /// immediately instead of on the next event tick. No-op while the
    /// server is unreachable.
    pub fn recheck_drift(&self) {
        if let Some(engine) = self.engine() {
            let _ = update_drift(&engine, &self.shared);
        }
    }
}

impl Drop for Driver {
    /// Flags the event thread down and joins it. The thread is either
    /// parked in `recv_timeout`/`sleep` (wakes within a poll interval),
    /// mid-snapshot (bounded by the engine's op timeout), or mid-connect
    /// (bounded by the connect timeout). A slot engine may outlive the
    /// driver; the subscribe callback's `Sender` then just fails its
    /// sends into the dead channel.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Final `seen_apps` flush — the debounce tick no longer runs.
        let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if shared.seen_dirty {
            shared.store.flush();
        }
    }
}

/// A live engine plus the receiver of its event subscription.
struct Conn {
    engine: Arc<PulseEngine>,
    rx: mpsc::Receiver<PulseEvent>,
}

/// The event thread's loop: dispatch each [`PulseEvent`], re-check drift
/// on a tick so changes outside the subscription mask still surface,
/// and — the resilience half — detect a dead context, drop it from the
/// slot, and reconnect on a [`RECONNECT_BACKOFF`].
fn event_loop(slot: &EngineSlot, shared: &Mutex<Shared>, shutdown: &AtomicBool) {
    // Attach subscribes + heals when the slot already holds an engine
    // (launch); on an empty slot it doubles as the first reconnect try.
    let mut conn = attach(slot, shared);
    let mut retry_at = Instant::now() + RECONNECT_BACKOFF;
    let mut last_drift = Instant::now();
    while !shutdown.load(Ordering::SeqCst) {
        match &conn {
            Some(c) => {
                // The recv timeout is the fast levels tick; drift keeps
                // its own 1 s cadence via `last_drift`.
                let result = match c.rx.recv_timeout(LEVELS_INTERVAL) {
                    Ok(PulseEvent::NewSinkInput(index)) => {
                        on_new_sink_input(index, &c.engine, shared)
                    }
                    // RemovedModule and Other: something changed outside
                    // the heal path — report it via drift, never auto-fix.
                    Ok(_) => {
                        last_drift = Instant::now();
                        update_drift(&c.engine, shared)
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if last_drift.elapsed() >= RECHECK_INTERVAL {
                            last_drift = Instant::now();
                            update_drift(&c.engine, shared)
                        } else {
                            Ok(())
                        }
                    }
                    // The subscription's Sender died — only possible if
                    // the engine was dropped or never connected.
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        Err(EngineError::NotReady)
                    }
                };
                emit_levels(&c.engine, shared);
                if let Err(e) = result
                    && is_dead(&e)
                {
                    // The connection is unrecoverable: drop it from the
                    // slot (commands now report "audio server
                    // unavailable") and back off.
                    clear_slot(slot, &c.engine);
                    conn = None;
                    retry_at = Instant::now() + RECONNECT_BACKOFF;
                    clear_meters(shared);
                    notify(shared);
                }
            }
            None => {
                if Instant::now() < retry_at {
                    thread::sleep(SHUTDOWN_POLL.min(retry_at - Instant::now()));
                    continue;
                }
                conn = attach(slot, shared);
                if conn.is_some() {
                    notify(shared);
                } else {
                    retry_at = Instant::now() + RECONNECT_BACKOFF;
                }
            }
        }
    }
}

/// Builds a live-connection pair: the engine the slot already holds, or
/// a fresh `PulseEngine::start()` that gets stored back into the slot.
/// The subscription is established *before* the heal pass runs — events
/// that fire mid-heal sit in the channel and are handled once `attach`
/// returns, so no new stream escapes enforcement. The heal itself is
/// best-effort: a snapshot failure surfaces as drift.
fn attach(slot: &EngineSlot, shared: &Mutex<Shared>) -> Option<Conn> {
    let engine = {
        let held = slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match held {
            Some(engine) => engine,
            None => Arc::new(PulseEngine::start().ok()?),
        }
    };
    // Publish before subscribing so command-path readers only ever see a
    // slot engine that is fully wired.
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&engine));
    let rx = engine.subscribe();
    {
        // Any attach is a fresh meter world — even on the rare reused
        // engine, clearing just makes the next reconcile's `set_meter`
        // re-check stream liveness (a no-op for live ones).
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.attached_meters.clear();
        shared.meter_keys.clear();
    }
    startup_heal(&engine, shared);
    Some(Conn { engine, rx })
}

/// Clears `slot` only if it still points at `engine` — belt and
/// suspenders against clearing a newer connection (the event thread is
/// the only writer, so the check is redundant but cheap).
fn clear_slot(slot: &EngineSlot, engine: &Arc<PulseEngine>) {
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_some_and(|e| Arc::ptr_eq(e, engine)) {
        *held = None;
    }
}

/// Runs the installed connectivity callback, if any — cloned out first
/// so the shared lock isn't held while outside code runs.
fn notify(shared: &Mutex<Shared>) {
    let f = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .notify
        .clone();
    if let Some(f) = f {
        f();
    }
}

/// Whether an engine-op failure means the connection itself is dead:
/// `NotReady` — the context left `Ready` (server gone); `Timeout` — the
/// context claims `Ready` but stopped answering ops. `ListFailed` and
/// friends are transient server answers — the engine stays.
fn is_dead(e: &EngineError) -> bool {
    matches!(e, EngineError::NotReady | EngineError::Timeout)
}

/// The launch/reconnect heal: plan against one snapshot, apply in order.
/// `MoveStream` actions resolve their sink name at apply time, so
/// devices loaded earlier in this same pass are already routable.
fn startup_heal(engine: &PulseEngine, shared: &Mutex<Shared>) {
    let Ok(actual) = engine.actual_state() else {
        return;
    };
    let actions = {
        let shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        plan_startup(shared.store.state(), &actual)
    };
    for action in actions {
        // Best effort — a failed action is reported as drift, not retried.
        let _ = apply_action(engine, shared, action);
    }
    let _ = update_drift(engine, shared);
}

/// Mid-session enforcement for one new stream: snapshot, record the app
/// name for the UI's "recently seen" list, and apply [`plan_new_stream`]'s
/// verdict — a `MoveStream` or nothing. The snapshot error propagates so
/// the event loop can tell a dead connection from a quiet one.
fn on_new_sink_input(
    index: u32,
    engine: &PulseEngine,
    shared: &Mutex<Shared>,
) -> Result<(), EngineError> {
    // A fresh snapshot, not a single-stream fetch: the index alone tells
    // us nothing about names, a short-lived stream may already be gone,
    // and the planner wants full sink context anyway.
    let actual = engine.actual_state()?;
    let Some(stream) = actual
        .sink_inputs
        .iter()
        .find(|s| s.index == index)
        .cloned()
    else {
        return Ok(());
    };
    let action = {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(app) = rule_key(&stream) {
            shared.store.record_seen_app(app);
            shared.seen_dirty = true;
        }
        plan_new_stream(&stream, shared.store.state(), &actual)
    };
    if let Some(action) = action {
        let _ = apply_action(engine, shared, action);
    }
    Ok(())
}

/// Recomputes drift from a fresh snapshot and prunes `rescues` entries
/// for streams that no longer exist — dead indices would otherwise
/// accumulate in-session. The snapshot error propagates so the event
/// loop can mark a dead connection; the previous drift report stays in
/// place on failure (a transient failure shouldn't blank the UI's
/// drift list).
fn update_drift(engine: &PulseEngine, shared: &Mutex<Shared>) -> Result<(), EngineError> {
    let actual = engine.actual_state()?;
    let (endpoints, removes) = {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.drift = detect_drift(shared.store.state(), &actual);
        shared
            .rescues
            .retain(|index, _| actual.sink_inputs.iter().any(|s| s.index == *index));
        // Deferred `seen_apps` persistence: one disk write per interval
        // rather than per stream event.
        if shared.seen_dirty && shared.last_seen_persist.elapsed() >= SEEN_PERSIST_INTERVAL {
            shared.store.flush();
            shared.seen_dirty = false;
            shared.last_seen_persist = Instant::now();
        }
        // Reconcile the meter set against the same snapshot: endpoints
        // no longer planned are removed below; everything planned is
        // (re)applied — `set_meter` no-ops on live same-target streams
        // and recreates dead ones.
        let plan = meter_plan(shared.store.state(), &actual);
        let removes: Vec<String> = shared
            .attached_meters
            .keys()
            .filter(|k| !plan.endpoints.contains_key(*k))
            .cloned()
            .collect();
        shared.attached_meters = plan.endpoints.clone();
        shared.meter_keys = plan.ui_keys;
        (plan.endpoints, removes)
    };
    // Engine calls after the shared lock is released — ops must never
    // run under it.
    for key in removes {
        engine.remove_meter(&key);
    }
    for (key, target) in endpoints {
        engine.set_meter(&key, &target);
    }
    Ok(())
}

/// Drains the engine's peak cache and fires the installed `fio-levels`
/// emitter with the `dev:`/`route:`/`mon:` payload. Runs on every
/// [`LEVELS_INTERVAL`] tick while a connection lives. Peaks for keys
/// whose endpoint never attached surface as 0 via the missing-`get`
/// fallback — the payload still names every planned UI key so the UI
/// can rely on key presence rather than guessing.
fn emit_levels(engine: &PulseEngine, shared: &Mutex<Shared>) {
    let peaks = engine.peaks();
    let work = {
        let shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.level_emit.clone().map(|emit| {
            let payload: HashMap<String, f32> = shared
                .meter_keys
                .iter()
                .map(|(ui_key, ep)| (ui_key.clone(), peaks.get(ep).copied().unwrap_or(0.0)))
                .collect();
            (emit, payload)
        })
    };
    // Callback runs outside the shared lock — it emits over IPC.
    if let Some((emit, payload)) = work {
        emit(payload);
    }
}

/// Clears meter bookkeeping and flattens the UI when the connection
/// dies — the dead engine's streams went with it, and stale keys would
/// otherwise freeze the last readings on screen.
fn clear_meters(shared: &Mutex<Shared>) {
    let emit = {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.attached_meters.clear();
        shared.meter_keys.clear();
        shared.level_emit.clone()
    };
    if let Some(emit) = emit {
        emit(HashMap::new());
    }
}

/// Executes one [`Action`]; shared between [`Driver::apply`] (command
/// path) and the event loop. Must not be called while holding `shared`.
fn apply_action(
    engine: &PulseEngine,
    shared: &Mutex<Shared>,
    action: Action,
) -> Result<(), EngineError> {
    match action {
        Action::LoadModule { name, argument } => engine.load_module(&name, &argument).map(|_| ()),
        Action::UnloadModule { index } => engine.unload_module(index),
        Action::SetDefaultSource { name } => engine.set_default_source(&name),
        Action::MoveStream {
            stream_index,
            sink_name,
        } => {
            // Resolve the name against a fresh snapshot: it was chosen at
            // plan time, and per-session indices shift as devices appear
            // and disappear.
            let actual = engine.actual_state()?;
            let move_plan = resolve_move(&actual, stream_index, &sink_name)?;
            engine.move_sink_input(stream_index, move_plan.sink_index)?;
            if let Some(prev) = move_plan.rescue {
                shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .rescues
                    .insert(stream_index, prev);
            }
            Ok(())
        }
    }
}

/// The resolved form of a `MoveStream` against one actual-state snapshot.
struct MovePlan {
    /// Server index of the sink named by the action.
    sink_index: u32,
    /// Name of the sink the stream sat on — the `rescues` value — present
    /// only when the stream is known and was somewhere *other* than the
    /// target (a move onto the current sink records nothing).
    rescue: Option<String>,
}

/// Resolves a `MoveStream`'s `sink_name` to its current index and works
/// out the rescue value in one pass. Pure — the only seam of `apply` that
/// needs no server.
fn resolve_move(
    actual: &ActualState,
    stream_index: u32,
    sink_name: &str,
) -> Result<MovePlan, EngineError> {
    let sink = actual
        .sinks
        .iter()
        .find(|s| s.name == sink_name)
        .ok_or(EngineError::NoSuchSink)?;
    let rescue = actual
        .sink_inputs
        .iter()
        .find(|s| s.index == stream_index)
        .and_then(|stream| {
            actual
                .sinks
                .iter()
                .find(|s| s.index == stream.sink)
                .map(|s| s.name.clone())
        })
        .filter(|name| name != sink_name);
    Ok(MovePlan {
        sink_index: sink.index,
        rescue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{SinkInfo, StreamInfo};

    fn mk_sink(index: u32, name: &str) -> SinkInfo {
        SinkInfo {
            index,
            name: name.to_string(),
            description: None,
            module: None,
        }
    }

    /// One physical sink + our virtual sink, and a stream playing on the
    /// physical one.
    fn mk_actual() -> ActualState {
        ActualState {
            sinks: vec![mk_sink(1, "alsa_out"), mk_sink(7, "fio-game")],
            sources: vec![],
            sink_inputs: vec![StreamInfo {
                index: 42,
                app_name: Some("game".to_string()),
                media_name: None,
                sink: 1,
            }],
            modules: vec![],
        }
    }

    #[test]
    fn driver_is_send_and_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<Driver>();
    }

    #[test]
    fn only_connection_failures_mark_engine_dead() {
        assert!(is_dead(&EngineError::NotReady));
        assert!(is_dead(&EngineError::Timeout));
        assert!(!is_dead(&EngineError::ListFailed));
        assert!(!is_dead(&EngineError::MoveFailed));
    }

    #[test]
    fn resolve_move_returns_index_and_previous_sink() {
        let actual = mk_actual();
        let plan = resolve_move(&actual, 42, "fio-game").unwrap();
        assert_eq!(plan.sink_index, 7);
        assert_eq!(plan.rescue.as_deref(), Some("alsa_out"));
    }

    #[test]
    fn resolve_move_unknown_sink_name_is_an_error() {
        let actual = mk_actual();
        assert!(matches!(
            resolve_move(&actual, 42, "fio-missing"),
            Err(EngineError::NoSuchSink)
        ));
    }

    #[test]
    fn resolve_move_unknown_stream_records_no_rescue() {
        // The stream ended between planning and apply: the move is still
        // attempted (and will MoveFailed on the server), but there is no
        // previous location to rescue back to.
        let actual = mk_actual();
        let plan = resolve_move(&actual, 99, "fio-game").unwrap();
        assert_eq!(plan.sink_index, 7);
        assert_eq!(plan.rescue, None);
    }

    #[test]
    fn resolve_move_already_on_target_records_no_rescue() {
        // Rescue means "where to put it back" — a stream already on the
        // target has nothing to restore to.
        let mut actual = mk_actual();
        actual.sink_inputs[0].sink = 7;
        let plan = resolve_move(&actual, 42, "fio-game").unwrap();
        assert_eq!(plan.rescue, None);
    }
}
