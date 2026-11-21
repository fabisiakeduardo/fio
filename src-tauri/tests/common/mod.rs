//! Shared helpers for the live-server integration tests: a poll-with-
//! timeout, a `pacat` process guard, and a module-cleanup guard.
//! Included via `mod common;` from each `pulse_*.rs` test binary.

// Each test binary only uses a subset of these — silence the noise
// rather than split helpers per test.
#![allow(dead_code)]

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fio_lib::audio::PulseEngine;

pub const TIMEOUT: Duration = Duration::from_secs(5);
pub const POLL: Duration = Duration::from_millis(50);

/// Polls `f` until it yields `Some` or [`TIMEOUT`] elapses, then panics —
/// a hanging test is worse than a loud failure.
pub fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

/// A spawned `pacat` process that is killed and reaped on drop, so a
/// failed assertion can't leave a silent stream playing in the server.
pub struct Pacat(Option<Child>);

impl Pacat {
    pub fn spawn() -> Self {
        Self::spawn_file("/dev/zero")
    }

    /// `/dev/urandom` as PCM is full-scale noise — a non-zero signal for
    /// the peak-meter tests, where `/dev/zero` would read flat.
    pub fn spawn_noise() -> Self {
        Self::spawn_file("/dev/urandom")
    }

    fn spawn_file(path: &str) -> Self {
        let child = Command::new("pacat")
            .args([
                "--playback",
                "--format=s16le",
                "--rate=48000",
                "--channels=2",
                path,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pacat is installed (part of libpulse)");
        Self(Some(child))
    }

    /// Kill the current stream and start a fresh one — the new sink-input
    /// is what the driver must see and route.
    pub fn restart(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *self = Self::spawn();
    }
}

impl Drop for Pacat {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Unloads every `module-null-sink` carrying `sink_name={sink_name}` on
/// drop — looked up *then*, so a module loaded shortly before a panic is
/// still reaped. Holds the engine `Arc` so the unload always lands on a
/// live connection. Declare before the driver/manager: locals drop in
/// reverse order, so their event thread is joined before this runs.
pub struct UnloadNullSink {
    pub engine: Arc<PulseEngine>,
    pub sink_name: String,
}

impl Drop for UnloadNullSink {
    fn drop(&mut self) {
        let want = format!("sink_name={}", self.sink_name);
        let Ok(state) = self.engine.actual_state() else {
            return;
        };
        for module in state.modules.iter().filter(|m| {
            m.name == "module-null-sink"
                && m.argument
                    .as_deref()
                    .is_some_and(|a| a.split_whitespace().any(|t| t == want))
        }) {
            let _ = self.engine.unload_module(module.index);
        }
    }
}
