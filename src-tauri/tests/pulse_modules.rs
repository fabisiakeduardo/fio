//! Module lifecycle: loading `module-null-sink` virtual devices and
//! `module-loopback` links, then unloading them again.

use fio_lib::audio::{
    loopback_args, null_sink_args, pulse_available, EngineError, PulseEngine,
};

/// A loaded module that unloads itself on drop — including on panic
/// unwinding — so a failed assertion mid-test doesn't leak the module in
/// the server until PipeWire restarts. Locals drop in reverse declaration
/// order, so declare the null sink before modules loaded on top of it.
struct Loaded<'a> {
    engine: &'a PulseEngine,
    index: Option<u32>,
}

impl<'a> Loaded<'a> {
    fn load(engine: &'a PulseEngine, name: &str, argument: &str) -> Self {
        Self {
            engine,
            index: Some(engine.load_module(name, argument).unwrap()),
        }
    }

    fn index(&self) -> u32 {
        self.index.unwrap()
    }

    /// Explicit unload (the happy path); suppresses the drop-time unload.
    fn unload(mut self) -> Result<(), EngineError> {
        self.engine.unload_module(self.index.take().unwrap())
    }
}

impl Drop for Loaded<'_> {
    fn drop(&mut self) {
        if let Some(index) = self.index.take() {
            let _ = self.engine.unload_module(index);
        }
    }
}

#[test]
fn null_sink_args_quotes_name() {
    assert!(null_sink_args("My Mic", "my-mic").contains("device.description='My Mic'"));
}

#[test]
fn create_and_remove_virtual_device() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let e = PulseEngine::start().unwrap();
    let idx = Loaded::load(&e, "module-null-sink", &null_sink_args("Test Mic", "test-mic"));

    // `source_name=fio-test-mic-mic` is ignored by module-null-sink on both
    // PipeWire and PulseAudio — the virtual mic end is always the sink's
    // monitor source, named `<sink>.monitor`. Assert that, and that it is a
    // monitor of *our* sink.
    let s = e.actual_state().unwrap();
    let sink = s
        .sinks
        .iter()
        .find(|k| k.name == "fio-test-mic")
        .expect("virtual sink fio-test-mic exists");
    assert!(s.sources.iter().any(|k| k.name == "fio-test-mic.monitor"
        && k.monitor_of_sink == Some(sink.index)));

    idx.unload().unwrap();
    let s = e.actual_state().unwrap();
    assert!(!s.sinks.iter().any(|k| k.name == "fio-test-mic"));
    assert!(!s.sources.iter().any(|k| k.name == "fio-test-mic.monitor"));
    e.shutdown();
}

#[test]
fn loopback_link_appears_and_removes() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let e = PulseEngine::start().unwrap();
    let v = Loaded::load(&e, "module-null-sink", &null_sink_args("LB", "lb"));

    // First real capture device — monitors have `monitor_of_sink` set.
    let def_src = e
        .actual_state()
        .unwrap()
        .sources
        .iter()
        .find(|s| s.monitor_of_sink.is_none())
        .map(|s| s.name.clone());
    let Some(def_src) = def_src else {
        eprintln!("no non-monitor source, skipping loopback leg");
        drop(v); // unloads the null sink before shutdown
        e.shutdown();
        return;
    };

    let idx = Loaded::load(&e, "module-loopback", &loopback_args(&def_src, "fio-lb"));
    assert!(e
        .actual_state()
        .unwrap()
        .modules
        .iter()
        .any(|m| m.index == idx.index()));

    // Unload in reverse load order: the loopback (depends on the sink) first.
    idx.unload().unwrap();
    v.unload().unwrap();
    e.shutdown();
}
