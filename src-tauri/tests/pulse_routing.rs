//! Live rule enforcement: a real playback stream (a `pacat` playing
//! `/dev/zero`) must be moved onto the sink of a virtual device the moment
//! it appears, driven by the driver's `NewSinkInput` event path — not by
//! any test-side move call.

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fio_lib::audio::{PulseEngine, pulse_available};
use fio_lib::model::{RouteSource, RouteTarget, Store};
use fio_lib::reconcile::Driver;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);
/// The null-sink `sink_name` the test's virtual device gets on the server.
const SINK_NAME: &str = "fio-route-test";

/// Polls `f` until it yields `Some` or [`TIMEOUT`] elapses, then panics —
/// a hanging test is worse than a loud failure.
fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
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
struct Pacat(Option<Child>);

impl Pacat {
    fn spawn() -> Self {
        let child = Command::new("pacat")
            .args([
                "--playback",
                "--format=s16le",
                "--rate=48000",
                "--channels=2",
                "/dev/zero",
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
    fn restart(&mut self) {
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

/// Unloads every `module-null-sink` carrying `sink_name={SINK_NAME}` on
/// drop — looked up *then*, so a module loaded shortly before a panic is
/// still reaped. Holds the engine `Arc` so the unload always lands on a
/// live connection. Declare before `driver`: locals drop in reverse order,
/// so the driver's event thread is joined before this runs.
struct UnloadOnDrop {
    engine: Arc<PulseEngine>,
}

impl Drop for UnloadOnDrop {
    fn drop(&mut self) {
        let want = format!("sink_name={SINK_NAME}");
        let Ok(state) = self.engine.actual_state() else {
            return;
        };
        for module in state.modules.iter().filter(|m| {
            m.name == "module-null-sink"
                && m.argument.as_deref().is_some_and(|a| a.contains(&want))
        }) {
            let _ = self.engine.unload_module(module.index);
        }
    }
}

#[test]
fn new_stream_gets_routed_by_rule() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let engine = Arc::new(PulseEngine::start().unwrap());

    // Baseline stream indices, so the pacat we spawn can be told apart
    // from anything already playing on this machine.
    let baseline: Vec<u32> = engine
        .actual_state()
        .unwrap()
        .sink_inputs
        .iter()
        .map(|s| s.index)
        .collect();

    let mut pacat = Pacat::spawn();
    let first = wait_for("pacat stream to appear", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| !baseline.contains(&s.index))
            .cloned()
    });
    // Whatever the server reported — application.name preferred,
    // media.name as fallback — becomes the rule's key.
    let key = first
        .app_name
        .clone()
        .or(first.media_name.clone())
        .expect("pacat stream reports an application.name or media.name");

    // Desired state: one virtual device (sink `fio-route-test`) and an
    // AppName rule keyed on the discovered stream name. The Driver takes
    // ownership of the store; its startup heal loads the device.
    let store_path = std::env::temp_dir().join(format!("fio-test-routing-{}", Uuid::new_v4()));
    let mut store = Store::load(store_path.clone());
    let device = store.add_device("Route Test".to_string());
    assert_eq!(device.slug, "route-test");
    store.add_route(
        RouteSource::AppName(key.clone()),
        RouteTarget::VirtualDevice(device.id),
    );

    // Reaps the null-sink module on success AND on panic — declared before
    // `driver` so it drops after the event thread is joined.
    let unload = UnloadOnDrop {
        engine: Arc::clone(&engine),
    };
    let driver = Driver::new(store, Arc::clone(&engine));

    let vdev_sink = wait_for("fio-route-test sink to appear", || {
        engine
            .actual_state()
            .unwrap()
            .sinks
            .iter()
            .find(|s| s.name == SINK_NAME)
            .map(|s| s.index)
    });

    // Fresh stream, fresh index: the only way it can land on
    // `fio-route-test` is the driver reacting to the NewSinkInput event.
    pacat.restart();
    wait_for("new stream to be routed to fio-route-test", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| {
                s.index != first.index
                    && s.sink == vdev_sink
                    && s.app_name.as_deref().or(s.media_name.as_deref()) == Some(key.as_str())
            })
            .map(|_| ())
    });

    // Explicit cleanup order: driver first (joins the event thread), then
    // the module guard (engine still alive), then the engine itself.
    drop(driver);
    drop(unload);
    drop(engine);
    let _ = std::fs::remove_file(&store_path);
}
