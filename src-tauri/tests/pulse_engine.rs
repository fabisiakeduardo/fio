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
