//! Delete-with-rescue against a live server: a `pacat` stream routed onto
//! a virtual device must land back on the sink it came from — the
//! driver's rescue bookkeeping — when `Manager::delete_device` runs, and
//! the device's `module-null-sink` must be gone.

mod common;

use std::sync::Arc;

use fio_lib::audio::{PulseEngine, pulse_available};
use fio_lib::commands::Manager;
use fio_lib::model::{RouteSource, RouteTarget, Store};
use uuid::Uuid;

use common::{Pacat, UnloadNullSink, wait_for};

/// The null-sink `sink_name` the test's virtual device gets on the server.
const SINK_NAME: &str = "fio-delete-test";

#[test]
fn delete_device_moves_streams_back_to_origin() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let engine = Arc::new(PulseEngine::start().unwrap());

    // Spawn pacat first and learn its stream's rule key and origin sink —
    // the rescue target the delete flow must restore.
    let baseline: Vec<u32> = engine
        .actual_state()
        .unwrap()
        .sink_inputs
        .iter()
        .map(|s| s.index)
        .collect();
    let pacat = Pacat::spawn();
    let stream = wait_for("pacat stream to appear", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| !baseline.contains(&s.index))
            .cloned()
    });
    let key = stream
        .app_name
        .clone()
        .or(stream.media_name.clone())
        .expect("pacat stream reports an application.name or media.name");
    let origin_sink = stream.sink;

    // Desired state: device `fio-delete-test` + a rule routing the pacat
    // stream onto it. The manager's driver heals at construction: loads
    // the null-sink and moves the live stream — recording the rescue.
    let store_path = std::env::temp_dir().join(format!("fio-test-delete-{}", Uuid::new_v4()));
    let mut store = Store::load(store_path.clone());
    let device = store.add_device("Delete Test".to_string());
    assert_eq!(device.slug, "delete-test");
    store.add_route(
        RouteSource::AppName(key),
        RouteTarget::VirtualDevice(device.id),
    );
    drop(store); // the manager loads the same file into its own store.

    // Safety net: if anything below panics before delete_device runs, the
    // module still gets reaped. Declared before `manager` so the manager's
    // event thread is joined first (reverse drop order).
    let unload = UnloadNullSink {
        engine: Arc::clone(&engine),
        sink_name: SINK_NAME.to_string(),
    };
    let manager = Manager::at_path(store_path.clone());

    let vdev_sink = wait_for("fio-delete-test sink to appear", || {
        engine
            .actual_state()
            .unwrap()
            .sinks
            .iter()
            .find(|s| s.name == SINK_NAME)
            .map(|s| s.index)
    });
    wait_for("pacat stream to be moved onto fio-delete-test", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| s.index == stream.index && s.sink == vdev_sink)
            .map(|_| ())
    });

    manager.delete_device(device.id).unwrap();

    // Rescue: the stream is back on the exact sink it came from.
    wait_for("pacat stream back on its origin sink", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| s.index == stream.index && s.sink == origin_sink)
            .map(|_| ())
    });
    // The module and its sink are gone.
    let actual = engine.actual_state().unwrap();
    assert!(
        actual.sinks.iter().all(|s| s.name != SINK_NAME),
        "fio-delete-test sink still present after delete: {:?}",
        actual.sinks
    );
    let want = format!("sink_name={SINK_NAME}");
    assert!(
        actual.modules.iter().all(|m| {
            !(m.name == "module-null-sink"
                && m.argument
                    .as_deref()
                    .is_some_and(|a| a.split_whitespace().any(|t| t == want)))
        }),
        "module-null-sink for {SINK_NAME} still loaded"
    );
    // And the store forgot the device and its route.
    assert!(manager.desired().devices.is_empty());
    assert!(manager.desired().routes.is_empty());

    drop(manager);
    drop(unload);
    drop(engine);
    drop(pacat);
    let _ = std::fs::remove_file(&store_path);
}
