//! The engine boundary: a `PulseEngine` wrapping libpulse's threaded mainloop.
//!
//! [`libpulse_binding`]'s `Mainloop`/`Context`/`Introspector` are `!Send` —
//! but the threaded mainloop they wrap (`pa_threaded_mainloop`) is explicitly
//! designed for cross-thread use: its loop runs on a libpulse-owned thread and
//! `pa_threaded_mainloop_{lock,unlock,stop}` plus `pa_context_*`
//! calls may be issued from any thread that holds the mainloop lock.
//!
//! [`PulseEngine`] therefore keeps the `Mainloop` + `Context` pair behind a
//! `Mutex` and applies `unsafe impl Send`, giving a `Send + Sync` engine the
//! Tauri command layer can call from any thread.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect;
use libpulse_binding::context::subscribe::{Facility, InterestMaskSet, Operation};
use libpulse_binding::context::{self, Context, FlagSet};
use libpulse_binding::def;
use libpulse_binding::mainloop::api::MainloopInnerType;
use libpulse_binding::mainloop::threaded::Mainloop;

use super::types::{ActualState, ModuleInfo, SinkInfo, SourceInfo, StreamInfo};

/// How long [`PulseEngine::start`] waits for the context to reach `Ready`
/// before giving up. Doubles as the upper bound of [`pulse_available`], which
/// runs on the command path — it must not hang when no server is listening.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Poll interval while waiting for the context to become ready.
const READY_POLL: Duration = Duration::from_millis(10);
/// Upper bound for any single server operation's completion callback.
/// libpulse does not guarantee the callback fires when an operation is
/// canceled (e.g. the server dies mid-call), so every op-wait is bounded:
/// a dead context fails the wait within [`OP_POLL`], a wedged one within
/// `OP_TIMEOUT`. Without the bound, one hung op would hold the engine
/// mutex — and every later engine call plus engine teardown — forever.
const OP_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval while waiting for an op — bounds how quickly a failed
/// context is noticed mid-wait.
const OP_POLL: Duration = Duration::from_millis(50);
/// Number of introspection list calls in one [`ActualState`] snapshot.
const SNAPSHOT_OPS: usize = 4;

/// Errors returned by [`PulseEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    /// `pa_threaded_mainloop_new` returned null.
    CreateMainloop,
    /// `pa_context_new` returned null.
    CreateContext,
    /// `pa_context_connect` rejected the connection request outright.
    Connect,
    /// `pa_threaded_mainloop_start` failed to spawn the loop thread.
    StartMainloop,
    /// The context reached `Failed`/`Terminated` while connecting.
    ConnectFailed,
    /// No `Ready` state within [`CONNECT_TIMEOUT`] — usually means no server.
    ConnectTimeout,
    /// An operation's completion callback never fired within
    /// [`OP_TIMEOUT`] while the context stayed `Ready` — a wedged
    /// server or mainloop.
    Timeout,
    /// An operation was requested while the context is not `Ready`
    /// (e.g. the server went away), or the context left `Ready`
    /// mid-operation.
    NotReady,
    /// An introspection list call reported an error.
    ListFailed,
    /// `pa_context_load_module` reported failure (`PA_INVALID_INDEX`).
    LoadFailed,
    /// `pa_context_unload_module` reported failure.
    UnloadFailed,
    /// `pa_context_move_sink_input_by_index` reported failure (e.g. the
    /// stream ended or the sink index is unknown).
    MoveFailed,
    /// `pa_context_set_default_source` reported failure.
    SetDefaultFailed,
    /// A `sink_name` carried by a plan action resolved to no sink in the
    /// server's current state.
    NoSuchSink,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::CreateMainloop => "failed to create pulse mainloop",
            Self::CreateContext => "failed to create pulse context",
            Self::Connect => "pulse context connect request failed",
            Self::StartMainloop => "failed to start pulse mainloop thread",
            Self::ConnectFailed => "pulse connection failed",
            Self::ConnectTimeout => "timed out waiting for pulse server",
            Self::Timeout => "pulse operation timed out",
            Self::NotReady => "pulse context is not connected",
            Self::ListFailed => "pulse introspection query failed",
            Self::LoadFailed => "pulse module load failed",
            Self::UnloadFailed => "pulse module unload failed",
            Self::MoveFailed => "pulse stream move failed",
            Self::SetDefaultFailed => "pulse set-default-source failed",
            Self::NoSuchSink => "no sink with that name on the server",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for EngineError {}

struct Inner {
    /// Field order matters: `context` drops first so it can disconnect while
    /// the (stopped) `mainloop` handle is still alive.
    context: Context,
    mainloop: Mainloop,
}

// SAFETY: `Inner` is only ever touched behind `PulseEngine`'s `Mutex`, or in
// `PulseEngine::drop` once no other reference can exist.
// - `pa_context_*` calls happen either on the constructing thread (before the
//   object is shared) or with the PA mainloop lock held, which is exactly the
//   calling convention `pa_threaded_mainloop` exists to provide.
// - `pa_threaded_mainloop_*` calls (`lock`/`unlock`/`wait`/`signal`/`stop`)
//   are documented as callable from any thread.
// - Teardown is only performed by `PulseEngine::drop`, which disconnects the
//   context under the PA lock and stops the loop thread before `Inner`'s
//   fields drop — so the PA thread never outlives the objects it dispatches.
// - Callbacks execute on the PA thread but never touch the `Rc`s inside
//   `Mainloop`/`Context`: the shared state they carry is `Arc`-based
//   collections and `mpsc::Sender`s — plain `Send` types, no raw pointers
//   into the mainloop at all.
unsafe impl Send for Inner {}

/// RAII guard for the PA threaded-mainloop lock.
///
/// The binding's `Mainloop::lock`/`unlock` take `&mut self`, but a `&mut`
/// held across `wait()` would block the reborrow `wait` itself needs — so
/// the guard keeps the raw `pa_threaded_mainloop` pointer and calls the C
/// functions directly (the same functions those methods wrap, obtainable via
/// the public `MainloopInnerType::get_ptr`).
///
/// Its real purpose is panic safety: if anything unwinds while the PA lock
/// is held, `Drop` still runs `pa_threaded_mainloop_unlock`. Without that, a
/// panic inside the locked section would leak the C mutex and deadlock
/// `PulseEngine::drop` running later on another thread.
struct PaLock(*mut libpulse_binding::mainloop::threaded::MainloopInternal);

impl PaLock {
    fn acquire(mainloop: &mut Mainloop) -> Self {
        let tml = mainloop._inner.get_ptr();
        // SAFETY: `tml` is a live threaded mainloop; `lock` may be called
        // from any thread except the loop thread itself (never us — callers
        // are outside callbacks). The pointer stays valid until the matching
        // `unlock` in `Drop`, which precedes any `stop`/`free` on this
        // mainloop by construction (engine teardown happens after this guard
        // is dropped).
        unsafe { libpulse_sys::pa_threaded_mainloop_lock(tml) };
        Self(tml)
    }
}

impl Drop for PaLock {
    fn drop(&mut self) {
        // SAFETY: paired with `acquire` — unlocks the lock this guard took.
        unsafe { libpulse_sys::pa_threaded_mainloop_unlock(self.0) };
    }
}

/// Server-wide defaults — the names of the current default sink and
/// default source. The device-delete flow needs the default sink as its
/// rescue fallback; the UI shows the default source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerDefaults {
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
}

/// A server event forwarded over the channel from [`PulseEngine::subscribe`],
/// distilled from the subscribe callback's (facility, operation, index)
/// triple to the cases the driver acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PulseEvent {
    /// `Facility::SinkInput` + `Operation::New`: a playback stream
    /// appeared. Payload is its server index.
    NewSinkInput(u32),
    /// `Facility::Module` + `Operation::Removed`: a module was unloaded
    /// — possibly by an outside `pactl`, i.e. drift to report. Payload is
    /// its server index.
    RemovedModule(u32),
    /// Anything else under the subscription mask (changes, removals of
    /// other kinds). The driver treats it as "re-check drift".
    Other,
}

/// One live connection to the PulseAudio/PipeWire-pulse server.
///
/// `Send + Sync`: intended to sit in Tauri state and be called from command
/// handler threads.
pub struct PulseEngine {
    inner: Mutex<Inner>,
}

impl PulseEngine {
    /// Connects to the default server as client `"fio"`, spawning the PA
    /// mainloop thread. Waits up to [`CONNECT_TIMEOUT`] for the context to
    /// reach `Ready`; on any failure the thread is stopped and the handles
    /// dropped before the error is returned.
    pub fn start() -> Result<Self, EngineError> {
        let mut mainloop = Mainloop::new().ok_or(EngineError::CreateMainloop)?;
        let mut context = Context::new(&mainloop, "fio").ok_or(EngineError::CreateContext)?;
        context
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|_| EngineError::Connect)?;
        mainloop.start().map_err(|_| EngineError::StartMainloop)?;

        let deadline = Instant::now() + CONNECT_TIMEOUT;
        if let Err(e) = wait_ready(&mut mainloop, &context, deadline) {
            {
                let _pa_lock = PaLock::acquire(&mut mainloop);
                context.disconnect();
            }
            mainloop.stop();
            return Err(e);
        }

        Ok(Self {
            inner: Mutex::new(Inner { context, mainloop }),
        })
    }

    /// Snapshot of the server's actual state: sinks, sources, live playback
    /// streams and loaded modules.
    ///
    /// Issues the four `Introspector` list calls while holding the PA lock,
    /// then releases it and parks on a completion channel so the callbacks
    /// can run on the mainloop thread. Each callback appends items to a
    /// shared [`ActualState`]; on end-of-list the last one sends on the
    /// channel — see [`list_done`]. The wait is bounded by [`wait_op`]:
    /// a dead context fails it promptly, a wedged one times out.
    pub fn actual_state(&self) -> Result<ActualState, EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let acc = Arc::new(Mutex::new(ActualState::default()));
        let pending = Arc::new(AtomicUsize::new(SNAPSHOT_OPS));
        let failed = Arc::new(AtomicBool::new(false));
        // Sent exactly once — by the last list op to report end-of-list.
        let (tx, rx) = mpsc::channel();

        // RAII PA lock: ops are issued while it is held; the guard's `Drop`
        // guarantees `pa_threaded_mainloop_unlock` runs even if a panic
        // unwinds out of this section — otherwise the C mutex would stay
        // held and `PulseEngine::drop` on another thread would deadlock.
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let introspector = inner.context.introspect();
        // NOTE: the returned `Operation`s must stay alive until their
        // end-of-list callback has fired — dropping one early detaches the
        // operation from our view of completion.
        let op_sinks = introspector.get_sink_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            let tx = tx.clone();
            move |r: ListResult<&introspect::SinkInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().sinks.push(SinkInfo {
                        index: i.index,
                        name: i.name.as_deref().unwrap_or_default().to_owned(),
                        description: i.description.as_deref().map(str::to_owned),
                        module: i.owner_module,
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, &tx),
                ListResult::Error => list_done(&pending, &failed, true, &tx),
            }
        });
        let op_sources = introspector.get_source_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            let tx = tx.clone();
            move |r: ListResult<&introspect::SourceInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().sources.push(SourceInfo {
                        index: i.index,
                        name: i.name.as_deref().unwrap_or_default().to_owned(),
                        description: i.description.as_deref().map(str::to_owned),
                        module: i.owner_module,
                        monitor_of_sink: i.monitor_of_sink,
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, &tx),
                ListResult::Error => list_done(&pending, &failed, true, &tx),
            }
        });
        let op_inputs = introspector.get_sink_input_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            let tx = tx.clone();
            move |r: ListResult<&introspect::SinkInputInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().sink_inputs.push(StreamInfo {
                        index: i.index,
                        app_name: i.proplist.get_str("application.name"),
                        media_name: i.proplist.get_str("media.name"),
                        sink: i.sink,
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, &tx),
                ListResult::Error => list_done(&pending, &failed, true, &tx),
            }
        });
        let op_modules = introspector.get_module_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            let tx = tx.clone();
            move |r: ListResult<&introspect::ModuleInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().modules.push(ModuleInfo {
                        index: i.index,
                        name: i.name.as_deref().unwrap_or_default().to_owned(),
                        argument: i.argument.as_deref().map(str::to_owned),
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, &tx),
                ListResult::Error => list_done(&pending, &failed, true, &tx),
            }
        });
        drop(pa_lock);

        // Releasing the PA lock above lets the queued list callbacks run on
        // the mainloop thread; the last one to finish sends on `rx`.
        let wait = wait_op(&mut inner, &rx);

        // Ops are dropped back under the PA lock: on success all four
        // callbacks reached end-of-list and were destroyed by the binding's
        // list proxy, so dropping only unrefs the C objects; on error this
        // detaches any still-pending ops — their callbacks, if they ever
        // fire, just send on a dead channel.
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op_sinks);
            drop(op_sources);
            drop(op_inputs);
            drop(op_modules);
        }
        wait?;

        if failed.load(Ordering::SeqCst) {
            return Err(EngineError::ListFailed);
        }
        // `mem::take`, not `Arc::try_unwrap`: `list_done` sends inside the
        // callback, so the receiver can wake while the last callback's
        // `acc` clone is still alive on the mainloop thread — a
        // `try_unwrap` would race its destruction.
        Ok(std::mem::take(
            &mut *acc.lock().unwrap_or_else(|e| e.into_inner()),
        ))
    }

    /// Loads a module into the server and returns its module index.
    ///
    /// `argument` is a `pactl`-style arg string; [`null_sink_args`] and
    /// [`loopback_args`] build the strings for the modules Fio manages.
    /// The index identifies the module to [`Self::unload_module`] and in
    /// `ActualState::modules` — it is per-session, never persist it.
    ///
    /// Same discipline as [`Self::actual_state`]: PA lock → issue → release
    /// → wait on a completion channel ([`wait_op`]) → collect. The index
    /// callback fires exactly once on the mainloop thread — with
    /// [`def::INVALID_INDEX`] on failure, surfaced here as
    /// [`EngineError::LoadFailed`] — then sends completion.
    ///
    /// # Panics
    /// Panics if libpulse refuses the request itself (a null operation) —
    /// in practice only if `name` or `argument` contain interior NULs.
    pub fn load_module(&self, name: &str, argument: &str) -> Result<u32, EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let result = Arc::new(Mutex::new(None::<u32>));
        let (tx, rx) = mpsc::channel();

        let op = inner.context.introspect().load_module(name, argument, {
            let result = Arc::clone(&result);
            move |index: u32| {
                *result.lock().unwrap() = Some(index);
                op_done(&tx);
            }
        });
        drop(pa_lock);

        let wait = wait_op(&mut inner, &rx);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        wait?;

        let index = result
            .lock()
            .unwrap()
            .expect("load_module callback runs exactly once before send");
        match index {
            def::INVALID_INDEX => Err(EngineError::LoadFailed),
            index => Ok(index),
        }
    }

    /// Unloads the module with the given index (from [`Self::load_module`]
    /// or `ActualState::modules`). Returns [`EngineError::UnloadFailed`]
    /// when the server reports failure (e.g. unknown index).
    ///
    /// Same lock → issue → wait-on-channel → collect pattern as
    /// [`Self::load_module`]; the success callback fires exactly once on
    /// the mainloop thread.
    pub fn unload_module(&self, index: u32) -> Result<(), EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let ok = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();

        let op = inner.context.introspect().unload_module(index, {
            let ok = Arc::clone(&ok);
            move |success: bool| {
                ok.store(success, Ordering::SeqCst);
                op_done(&tx);
            }
        });
        drop(pa_lock);

        let wait = wait_op(&mut inner, &rx);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        wait?;

        match ok.load(Ordering::SeqCst) {
            true => Ok(()),
            false => Err(EngineError::UnloadFailed),
        }
    }

    /// Subscribes to sink-input and module events and returns the channel
    /// each event is forwarded to as a [`PulseEvent`].
    ///
    /// The subscribe callback runs on the PA mainloop thread, so it does
    /// exactly one thing: map the event and `Sender::send` it — `send`
    /// never blocks and never takes the PA lock or `self.inner` (a
    /// callback re-locking either would deadlock the loop thread).
    ///
    /// Only the most recent receiver is live: calling this again replaces
    /// the context's callback, dropping the earlier `Sender` and
    /// disconnecting the earlier receiver. When the engine is dropped the
    /// callback dies with the context, which disconnects the receiver and
    /// lets a blocking consumer exit. If the context is not `Ready` the
    /// returned receiver simply never yields events.
    pub fn subscribe(&self) -> mpsc::Receiver<PulseEvent> {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return rx;
        }

        inner
            .context
            .set_subscribe_callback(Some(Box::new(move |facility, operation, index| {
                let event = match (facility, operation) {
                    (Some(Facility::SinkInput), Some(Operation::New)) => {
                        PulseEvent::NewSinkInput(index)
                    }
                    (Some(Facility::Module), Some(Operation::Removed)) => {
                        PulseEvent::RemovedModule(index)
                    }
                    _ => PulseEvent::Other,
                };
                // A disconnected receiver is not an error — e.g. the
                // driver was dropped while the engine lives on.
                let _ = tx.send(event);
            })));

        // Wait for the subscribe op itself so a caller can rely on events
        // being delivered the moment this returns. Best-effort: a wedged
        // server yields a receiver that simply sees no events, which the
        // driver already tolerates.
        let (tx_done, rx_done) = mpsc::channel::<()>();
        let op = inner
            .context
            .subscribe(InterestMaskSet::SINK_INPUT | InterestMaskSet::MODULE, {
                move |_success: bool| op_done(&tx_done)
            });
        drop(pa_lock);

        let _ = wait_op(&mut inner, &rx_done);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        rx
    }

    /// Moves a playback stream (sink-input) onto a different sink, both by
    /// server index. [`EngineError::MoveFailed`] when the server reports
    /// failure — e.g. the stream ended or the index is stale.
    ///
    /// Same lock → issue → wait-on-channel → collect pattern as
    /// [`Self::load_module`]; the success callback fires exactly once on
    /// the mainloop thread.
    pub fn move_sink_input(&self, stream_index: u32, sink_index: u32) -> Result<(), EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let ok = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let op = inner.context.introspect().move_sink_input_by_index(
            stream_index,
            sink_index,
            Some(Box::new({
                let ok = Arc::clone(&ok);
                move |success: bool| {
                    ok.store(success, Ordering::SeqCst);
                    op_done(&tx);
                }
            })),
        );
        drop(pa_lock);

        let wait = wait_op(&mut inner, &rx);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        wait?;

        match ok.load(Ordering::SeqCst) {
            true => Ok(()),
            false => Err(EngineError::MoveFailed),
        }
    }

    /// The server's current default sink and source names.
    ///
    /// `get_server_info`'s callback fires exactly once on the mainloop
    /// thread — same lock → issue → wait-on-channel → collect pattern as
    /// [`Self::load_module`].
    pub fn server_defaults(&self) -> Result<ServerDefaults, EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let info = Arc::new(Mutex::new(None::<ServerDefaults>));
        let (tx, rx) = mpsc::channel();
        let op = inner.context.introspect().get_server_info({
            let info = Arc::clone(&info);
            move |i: &introspect::ServerInfo| {
                *info.lock().unwrap() = Some(ServerDefaults {
                    default_sink: i.default_sink_name.as_deref().map(str::to_owned),
                    default_source: i.default_source_name.as_deref().map(str::to_owned),
                });
                op_done(&tx);
            }
        });
        drop(pa_lock);

        let wait = wait_op(&mut inner, &rx);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        wait?;

        info.lock().unwrap().take().ok_or(EngineError::ListFailed)
    }

    /// Makes `name` the server's default source (the "use as default mic"
    /// action). [`EngineError::SetDefaultFailed`] when the server reports
    /// failure, e.g. no such source.
    ///
    /// Same lock → issue → wait-on-channel → collect pattern as
    /// [`Self::load_module`].
    ///
    /// # Panics
    /// Panics if `name` contains an interior NUL (the binding unwraps its
    /// `CString` conversion).
    pub fn set_default_source(&self, name: &str) -> Result<(), EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        let ok = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let op = inner.context.set_default_source(name, {
            let ok = Arc::clone(&ok);
            move |success: bool| {
                ok.store(success, Ordering::SeqCst);
                op_done(&tx);
            }
        });
        drop(pa_lock);

        let wait = wait_op(&mut inner, &rx);
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            drop(op);
        }
        wait?;

        match ok.load(Ordering::SeqCst) {
            true => Ok(()),
            false => Err(EngineError::SetDefaultFailed),
        }
    }

    /// Disconnects the context and stops the mainloop thread.
    ///
    /// Equivalent to letting the engine go out of scope; kept as a named
    /// method because the intent reads better at call sites. The actual
    /// teardown happens in [`Drop::drop`] — see there for why it cannot be
    /// skipped.
    pub fn shutdown(self) {
        // `Drop::drop` performs lock → disconnect → unlock → stop.
    }
}

impl Drop for PulseEngine {
    /// Tears the connection down in the only safe order: hold the PA lock,
    /// `pa_context_disconnect`, release the lock, then `pa_threaded_mainloop_stop`
    /// to join the event-loop thread. Only after that may `Inner`'s fields
    /// drop (`context` first — a `pa_context_unref` while the dispatch thread
    /// is still running would race live events, which is exactly what this
    /// ordering prevents; `mainloop` last frees the joined mainloop object).
    ///
    /// This runs for every teardown path — `shutdown`, `Drop` on managed
    /// state removal, and panic unwinding — so no caller can leave the PA
    /// thread running into an unref.
    fn drop(&mut self) {
        // `get_mut` cannot block (unique ownership) but reports the poison
        // flag; recover the contents anyway — the C handles are still valid
        // and teardown must still happen.
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            inner.context.disconnect();
        }
        inner.mainloop.stop();
    }
}

/// Polls `context.get_state()` until `Ready`, `Failed`/`Terminated`, or the
/// deadline passes. The PA lock is taken for each read and released while
/// sleeping so the mainloop thread can advance the connection.
fn wait_ready(
    mainloop: &mut Mainloop,
    context: &Context,
    deadline: Instant,
) -> Result<(), EngineError> {
    loop {
        mainloop.lock();
        let state = context.get_state();
        mainloop.unlock();
        match state {
            context::State::Ready => return Ok(()),
            context::State::Failed | context::State::Terminated => {
                return Err(EngineError::ConnectFailed);
            }
            _ if Instant::now() >= deadline => {
                return Err(EngineError::ConnectTimeout);
            }
            _ => thread::sleep(READY_POLL),
        }
    }
}

/// Parks until an op's completion arrives on `rx`, the context leaves
/// `Ready`, or [`OP_TIMEOUT`] passes. Called with `self.inner` held (ops
/// stay serialized) but *not* the PA lock — callbacks run on the mainloop
/// thread while this waits; the lock is only re-taken for each brief
/// `get_state` poll.
///
/// `Err(NotReady)`: the context failed or terminated mid-op — the server
/// died or is going away, the op will never complete. `Err(Timeout)`:
/// the context stayed `Ready` but the callback never came — wedged, the
/// op is abandoned (dropping its `Operation` detaches it). This bound is
/// what keeps a dead server from hanging every engine caller — and
/// `PulseEngine::drop` via the `inner` mutex — forever: libpulse does
/// not guarantee the op callback fires when an operation is canceled.
fn wait_op(inner: &mut Inner, rx: &mpsc::Receiver<()>) -> Result<(), EngineError> {
    let deadline = Instant::now() + OP_TIMEOUT;
    loop {
        match rx.recv_timeout(OP_POLL) {
            Ok(()) => return Ok(()),
            // The completion sender died without sending — only possible
            // if the op's callback was torn down with a dead context.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(EngineError::NotReady);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let state = {
            let _pa_lock = PaLock::acquire(&mut inner.mainloop);
            inner.context.get_state()
        };
        if state != context::State::Ready {
            return Err(EngineError::NotReady);
        }
        if Instant::now() >= deadline {
            return Err(EngineError::Timeout);
        }
    }
}

/// Runs on the PA mainloop thread when one of the snapshot list ops reports
/// `End`/`Error`. Records the error flag, decrements the pending counter,
/// and — once all ops are done — sends completion to the `wait_op`
/// receiver in [`PulseEngine::actual_state`]. `send` never blocks, so the
/// callback never stalls the loop thread; a dead receiver just means the
/// caller already gave up.
fn list_done(
    pending: &AtomicUsize,
    failed: &AtomicBool,
    had_error: bool,
    tx: &mpsc::Sender<()>,
) {
    if had_error {
        failed.store(true, Ordering::SeqCst);
    }
    if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
        let _ = tx.send(());
    }
}

/// Runs on the PA mainloop thread when a single-shot operation callback
/// (module load/unload, stream move, etc.) fires: sends completion to the
/// caller's `wait_op` receiver. See [`list_done`] — `send` never blocks
/// and a dead receiver is harmless.
fn op_done(tx: &mpsc::Sender<()>) {
    let _ = tx.send(());
}

/// `pactl`-style argument string for `module-null-sink`: a virtual sink
/// named `fio-{slug}`, labelled `{name}` in the UI. The pinned
/// `source_name=fio-{slug}-mic` arg is aspirational — pipewire-pulse
/// ignores it; the device's mic end is the sink's monitor source, which
/// resolves to `fio-{slug}.monitor`.
pub fn null_sink_args(name: &str, slug: &str) -> String {
    format!(
        "sink_name=fio-{slug} source_name=fio-{slug}-mic \
         sink_properties=device.description='{name}' \
         source_properties=device.description='{name}'"
    )
}

/// `pactl`-style argument string for `module-loopback`: forwards audio
/// captured from `source_name` into `sink_name`. `source_dont_move` and
/// `sink_dont_move` pin both ends so default-device changes can't drag the
/// link onto another device.
pub fn loopback_args(source_name: &str, sink_name: &str) -> String {
    format!("source={source_name} sink={sink_name} source_dont_move=1 sink_dont_move=1")
}

/// `true` if a PulseAudio-compatible server answers within
/// [`CONNECT_TIMEOUT`]. Used by tests to skip on headless machines and by the
/// command layer to produce a friendly error instead of a connect hang.
pub fn pulse_available() -> bool {
    match PulseEngine::start() {
        Ok(engine) => {
            engine.shutdown();
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine is held in Tauri state and called from command threads.
    #[test]
    fn engine_is_send_and_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<PulseEngine>();
    }
}
