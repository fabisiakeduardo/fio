//! The reconcile driver: owns the [`Store`], the engine's event channel,
//! and the thread that wires [`PulseEvent`]s to the pure planners.
//!
//! Enforcement is deliberately asymmetric:
//!
//! - **Heal at launch** — [`Driver::new`] runs [`plan_startup`] once and
//!   applies the whole plan (adopt or load `fio-*` sinks, move matching
//!   live streams, load missing loopbacks).
//! - **Mid-session enforce only on new streams** — a `NewSinkInput` event
//!   records the app name and runs [`plan_new_stream`], moving just that
//!   stream when a rule matches.
//! - **Everything else is drift** — module removals and changes recompute
//!   [`detect_drift`] and surface it via [`Driver::drift`]; the driver
//!   never auto-fixes something that was there and went away.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::audio::{ActualState, EngineError, PulseEngine, PulseEvent};
use crate::model::Store;

use super::{Action, Drift, detect_drift, plan_new_stream, plan_startup, rule_key};

/// How often the event loop recomputes [`detect_drift`] even when no
/// events arrive — drift surfaces within one tick rather than only on the
/// next server change.
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);

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
}

/// Owns the [`Store`] + [`PulseEngine`] pair and the event thread between
/// them. `Send + Sync` so it can sit in Tauri state next to the engine.
pub struct Driver {
    engine: Arc<PulseEngine>,
    shared: Arc<Mutex<Shared>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Driver {
    /// Takes ownership of `store`, subscribes to engine events, runs the
    /// [`plan_startup`] heal pass once, then spawns the event thread.
    ///
    /// The subscription is established *before* the heal pass runs and the
    /// thread is spawned after — events that fire mid-heal sit in the
    /// channel and are handled once the thread starts, so no new stream
    /// escapes enforcement.
    ///
    /// The heal pass is best-effort: if the server isn't answering yet,
    /// the driver still comes up and the event loop's drift tick reports
    /// the gap instead.
    pub fn new(store: Store, engine: Arc<PulseEngine>) -> Self {
        let rx = engine.subscribe();
        let mut driver = Self {
            engine: Arc::clone(&engine),
            shared: Arc::new(Mutex::new(Shared {
                store,
                rescues: HashMap::new(),
                drift: Vec::new(),
            })),
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: None,
        };

        driver.startup_heal();

        let thread = {
            let engine = Arc::clone(&engine);
            let shared = Arc::clone(&driver.shared);
            let shutdown = Arc::clone(&driver.shutdown);
            thread::Builder::new()
                .name("fio-events".to_string())
                .spawn(move || event_loop(rx, engine, shared, shutdown))
                .expect("failed to spawn fio event thread")
        };
        driver.thread = Some(thread);
        driver
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
    pub fn apply(&self, a: Action) -> Result<(), EngineError> {
        apply_action(&self.engine, &self.shared, a)
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

    /// The launch-time heal: plan against one snapshot, apply in order.
    /// `MoveStream` actions resolve their sink name at apply time, so
    /// devices loaded earlier in this same pass are already routable.
    fn startup_heal(&self) {
        let Ok(actual) = self.engine.actual_state() else {
            return;
        };
        let actions = {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            plan_startup(shared.store.state(), &actual)
        };
        for action in actions {
            // Best effort — a failed action is reported as drift, not retried.
            let _ = self.apply(action);
        }
        update_drift(&self.engine, &self.shared);
    }
}

impl Drop for Driver {
    /// Flags the event thread down and joins it. The thread is either
    /// parked in `recv_timeout` (wakes within [`RECHECK_INTERVAL`]) or
    /// mid-snapshot (bounded by the server answering). The engine may
    /// outlive the driver; the subscribe callback's `Sender` then just
    /// fails its sends into the dead channel.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The event thread's loop: dispatch each [`PulseEvent`], re-check drift
/// on a tick so changes outside the subscription mask still surface, and
/// exit when the engine's `Sender` dies (engine drop) or the driver flags
/// shutdown.
fn event_loop(
    rx: mpsc::Receiver<PulseEvent>,
    engine: Arc<PulseEngine>,
    shared: Arc<Mutex<Shared>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        match rx.recv_timeout(RECHECK_INTERVAL) {
            Ok(PulseEvent::NewSinkInput(index)) => on_new_sink_input(index, &engine, &shared),
            // RemovedModule and Other: something changed outside the heal
            // path — report it via drift, never auto-fix.
            Ok(_) => update_drift(&engine, &shared),
            Err(mpsc::RecvTimeoutError::Timeout) => update_drift(&engine, &shared),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Mid-session enforcement for one new stream: snapshot, record the app
/// name for the UI's "recently seen" list, and apply [`plan_new_stream`]'s
/// verdict — a `MoveStream` or nothing.
fn on_new_sink_input(index: u32, engine: &PulseEngine, shared: &Mutex<Shared>) {
    // A fresh snapshot, not a single-stream fetch: the index alone tells
    // us nothing about names, a short-lived stream may already be gone,
    // and the planner wants full sink context anyway.
    let Ok(actual) = engine.actual_state() else {
        return;
    };
    let Some(stream) = actual
        .sink_inputs
        .iter()
        .find(|s| s.index == index)
        .cloned()
    else {
        return;
    };
    let action = {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(app) = rule_key(&stream) {
            shared.store.record_seen_app(app);
        }
        plan_new_stream(&stream, shared.store.state(), &actual)
    };
    if let Some(action) = action {
        let _ = apply_action(engine, shared, action);
    }
}

/// Recomputes drift from a fresh snapshot; leaves the previous report in
/// place when the server isn't answering (a transient failure shouldn't
/// blank the UI's drift list).
fn update_drift(engine: &PulseEngine, shared: &Mutex<Shared>) {
    let Ok(actual) = engine.actual_state() else {
        return;
    };
    let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
    shared.drift = detect_drift(shared.store.state(), &actual);
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
