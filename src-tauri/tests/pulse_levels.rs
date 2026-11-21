//! Live peak metering: `PEAK_DETECT` record streams must attach against
//! the real server and read non-zero peaks while signal flows — a
//! `pacat` playing `/dev/urandom` (full-scale noise) provides it. Both
//! meter shapes are exercised: a sink-input index (per-app, pavucontrol
//! style) and a monitor source by name.

mod common;

use fio_lib::audio::{MeterTarget, PulseEngine, pulse_available};

use common::{Pacat, wait_for};

/// Any peak reading above this clears "attached but flat" — noise peaks
/// near 1.0.
const SIGNAL: f32 = 0.01;

#[test]
fn sink_input_meter_reads_live_peak() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let engine = PulseEngine::start().unwrap();

    // Baseline so our pacat can be told from streams already playing.
    let baseline: Vec<u32> = engine
        .actual_state()
        .unwrap()
        .sink_inputs
        .iter()
        .map(|s| s.index)
        .collect();
    let _noise = Pacat::spawn_noise();
    let index = wait_for("pacat sink-input", || {
        engine
            .actual_state()
            .unwrap()
            .sink_inputs
            .iter()
            .find(|s| !baseline.contains(&s.index))
            .map(|s| s.index)
    });

    let key = format!("in:{index}");
    engine.set_meter(&key, &MeterTarget::SinkInput(index));
    wait_for("non-zero peak on the app's stream", || {
        engine.peaks().get(&key).copied().filter(|p| *p > SIGNAL)
    });

    // Removing the meter drops the stream — the key leaves the map.
    engine.remove_meter(&key);
    assert!(!engine.peaks().contains_key(&key));
    engine.shutdown();
}

#[test]
fn monitor_source_meter_reads_live_peak() {
    if !pulse_available() {
        eprintln!("no pulse, skipping");
        return;
    }
    let engine = PulseEngine::start().unwrap();

    let baseline: Vec<u32> = engine
        .actual_state()
        .unwrap()
        .sink_inputs
        .iter()
        .map(|s| s.index)
        .collect();
    let _noise = Pacat::spawn_noise();
    // The sink the noise plays to — its `.monitor` source carries the
    // signal back out.
    let sink_name = wait_for("pacat stream's sink", || {
        let actual = engine.actual_state().unwrap();
        let stream = actual
            .sink_inputs
            .iter()
            .find(|s| !baseline.contains(&s.index))?;
        actual
            .sinks
            .iter()
            .find(|s| s.index == stream.sink)
            .map(|s| s.name.clone())
    });
    let monitor = format!("{sink_name}.monitor");

    let key = format!("src:{monitor}");
    engine.set_meter(&key, &MeterTarget::Source(monitor));
    wait_for("non-zero peak on the sink monitor", || {
        engine.peaks().get(&key).copied().filter(|p| *p > SIGNAL)
    });

    // set_meter with the same target is a no-op — the live stream and
    // its reading survive.
    engine.set_meter(&key, &MeterTarget::Source(format!("{sink_name}.monitor")));
    assert!(engine.peaks().contains_key(&key));
    engine.shutdown();
}
