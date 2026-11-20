use fio_lib::audio::{pulse_available, PulseEngine};

#[test]
fn connects_and_lists() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let e = PulseEngine::start().unwrap();
    let s = e.actual_state().unwrap();
    assert!(!s.sinks.is_empty()); // dev machine always has >=1 sink
    assert!(!s.sources.is_empty());
    e.shutdown();
}

/// Dropping the engine without calling `shutdown()` must perform the same
/// safe teardown (Drop impl) — not leave the PA mainloop thread running into
/// a `pa_context_unref`. Proves teardown actually happened by reconnecting.
#[test]
fn drop_without_shutdown_tears_down() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    drop(PulseEngine::start().unwrap());
    let e = PulseEngine::start().unwrap();
    assert!(!e.actual_state().unwrap().sinks.is_empty());
    e.shutdown();
}
