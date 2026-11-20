//! The engine boundary: a `PulseEngine` wrapping libpulse's threaded mainloop.
//!
//! [`libpulse_binding`]'s `Mainloop`/`Context`/`Introspector` are `!Send` —
//! but the threaded mainloop they wrap (`pa_threaded_mainloop`) is explicitly
//! designed for cross-thread use: its loop runs on a libpulse-owned thread and
//! `pa_threaded_mainloop_{lock,unlock,wait,signal,stop}` plus `pa_context_*`
//! calls may be issued from any thread that holds the mainloop lock.
//!
//! [`PulseEngine`] therefore keeps the `Mainloop` + `Context` pair behind a
//! `Mutex` and applies `unsafe impl Send`, giving a `Send + Sync` engine the
//! Tauri command layer can call from any thread.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::{self, Context, FlagSet};
use libpulse_binding::context::introspect;
use libpulse_binding::mainloop::api::MainloopInnerType;
use libpulse_binding::mainloop::threaded::Mainloop;

use super::types::{ActualState, ModuleInfo, SinkInfo, SourceInfo, StreamInfo};

/// How long [`PulseEngine::start`] waits for the context to reach `Ready`
/// before giving up. Doubles as the upper bound of [`pulse_available`], which
/// runs on the command path — it must not hang when no server is listening.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Poll interval while waiting for the context to become ready.
const READY_POLL: Duration = Duration::from_millis(10);
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
    /// An operation was requested while the context is not `Ready`
    /// (e.g. the server went away).
    NotReady,
    /// An introspection list call reported an error.
    ListFailed,
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
            Self::NotReady => "pulse context is not connected",
            Self::ListFailed => "pulse introspection query failed",
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
//   `Mainloop`/`Context`: shared state is `Arc`-based, and the only mainloop
//   handle they use is the raw `pa_threaded_mainloop` pointer, passed to the
//   `pa_threaded_mainloop_signal` FFI function — a plain C call with no Rust
//   aliasing.
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
        let mut context =
            Context::new(&mainloop, "fio").ok_or(EngineError::CreateContext)?;
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
    /// then parks in `mainloop.wait()` — which releases the lock — so the
    /// callbacks can run on the mainloop thread. Each callback appends items
    /// to a shared [`ActualState`]; on end-of-list the last one signals the
    /// wait via `pa_threaded_mainloop_signal`.
    pub fn actual_state(&self) -> Result<ActualState, EngineError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let acc = Arc::new(Mutex::new(ActualState::default()));
        let pending = Arc::new(AtomicUsize::new(SNAPSHOT_OPS));
        let failed = Arc::new(AtomicBool::new(false));

        // RAII PA lock: `wait()` below re-acquires the lock internally, and
        // the guard's `Drop` guarantees `pa_threaded_mainloop_unlock` runs if
        // a panic unwinds out of this section — otherwise the C mutex would
        // stay held and `PulseEngine::drop` on another thread would deadlock.
        let pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            drop(pa_lock);
            return Err(EngineError::NotReady);
        }

        // Raw pointer to the C `pa_threaded_mainloop`, handed to each
        // callback purely for `pa_threaded_mainloop_signal`. Stays valid for
        // the callbacks' whole lifetime: they are all destroyed on the
        // mainloop thread when they report end-of-list, and we do not release
        // `inner` or stop the mainloop until `pending` hits zero below.
        let tml = inner.mainloop._inner.get_ptr();

        let introspector = inner.context.introspect();
        // NOTE: the returned `Operation`s must stay alive until their
        // end-of-list callback has fired — dropping one early detaches the
        // operation from our view of completion.
        let op_sinks = introspector.get_sink_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            move |r: ListResult<&introspect::SinkInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().sinks.push(SinkInfo {
                        index: i.index,
                        name: i.name.as_deref().unwrap_or_default().to_owned(),
                        description: i.description.as_deref().map(str::to_owned),
                        module: i.owner_module,
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, tml),
                ListResult::Error => list_done(&pending, &failed, true, tml),
            }
        });
        let op_sources = introspector.get_source_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
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
                ListResult::End => list_done(&pending, &failed, false, tml),
                ListResult::Error => list_done(&pending, &failed, true, tml),
            }
        });
        let op_inputs = introspector.get_sink_input_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            move |r: ListResult<&introspect::SinkInputInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().sink_inputs.push(StreamInfo {
                        index: i.index,
                        app_name: i.proplist.get_str("application.name"),
                        media_name: i.proplist.get_str("media.name"),
                        sink: i.sink,
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, tml),
                ListResult::Error => list_done(&pending, &failed, true, tml),
            }
        });
        let op_modules = introspector.get_module_info_list({
            let acc = Arc::clone(&acc);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&failed);
            move |r: ListResult<&introspect::ModuleInfo>| match r {
                ListResult::Item(i) => {
                    acc.lock().unwrap().modules.push(ModuleInfo {
                        index: i.index,
                        name: i.name.as_deref().unwrap_or_default().to_owned(),
                        argument: i.argument.as_deref().map(str::to_owned),
                    });
                }
                ListResult::End => list_done(&pending, &failed, false, tml),
                ListResult::Error => list_done(&pending, &failed, true, tml),
            }
        });

        // `wait` releases the PA lock while parked, letting the queued list
        // callbacks run on the mainloop thread; the last one to finish calls
        // `pa_threaded_mainloop_signal`, waking us. `wait` may also wake
        // spuriously — hence the loop re-checking `pending`.
        while pending.load(Ordering::SeqCst) > 0 {
            inner.mainloop.wait();
        }

        // All four callbacks reached end-of-list and have been destroyed by
        // the binding's list proxy on the mainloop thread; dropping the
        // `Operation`s now only unrefs the C objects.
        drop(op_sinks);
        drop(op_sources);
        drop(op_inputs);
        drop(op_modules);
        drop(pa_lock);

        if failed.load(Ordering::SeqCst) {
            return Err(EngineError::ListFailed);
        }
        let state = Arc::try_unwrap(acc)
            .expect("no other ActualState refs after ops complete")
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        Ok(state)
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

/// Runs on the PA mainloop thread when one of the snapshot list ops reports
/// `End`/`Error`. Decrements the pending counter and, once all ops are done,
/// signals the `mainloop.wait()` in [`PulseEngine::actual_state`].
fn list_done(
    pending: &AtomicUsize,
    failed: &AtomicBool,
    had_error: bool,
    tml: *mut libpulse_binding::mainloop::threaded::MainloopInternal,
) {
    if had_error {
        failed.store(true, Ordering::SeqCst);
    }
    if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
        // SAFETY: `tml` is the engine's threaded mainloop, which outlives
        // this callback — `actual_state` does not return (and `shutdown`
        // cannot proceed on another thread) until every pending op has run
        // this function. Signalling a threaded mainloop from within one of
        // its own callbacks is the documented way to wake a `wait()`ing
        // thread.
        unsafe { libpulse_sys::pa_threaded_mainloop_signal(tml, 0) };
    }
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
