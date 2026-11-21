//! Live peak metering — pavucontrol's trick.
//!
//! Each metered endpoint gets one record stream created with
//! [`stream::FlagSet::PEAK_DETECT`]: the server computes peaks itself
//! and pushes tiny mono-f32 fragments (~25 Hz, one `float` each), so Fio
//! never touches real PCM. Two endpoint shapes:
//!
//! - [`MeterTarget::Source(name)`] — `connect_record(name)` against a
//!   capture source or a sink's `.monitor` source (the engine's own
//!   `fio-{slug}.monitor` included — that is the device output level).
//! - [`MeterTarget::SinkInput(index)`] — [`Stream::set_monitor_stream`]
//!   before connect taps one app's playback, the way pavucontrol meters
//!   per-app. `index` is a per-session sink-input index; the driver
//!   re-resolves it on every drift tick.
//!
//! There is no read callback: the driver's fast tick calls
//! [`PulseEngine::peaks`], which drains `peek()` on every attached
//! stream under the PA lock. `peek`/`discard` are plain API calls —
//! the read callback is only a notification mechanism, and polling on
//! the emit tick drains whatever arrived since last time.
//!
//! Lock discipline is `engine.rs`'s: `inner` mutex → `PaLock` → calls,
//! never the reverse. Every method here runs in that order, all on the
//! caller's thread.

use std::collections::HashMap;

use libpulse_binding::context;
use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::{self, PeekResult, Stream};

use super::engine::{Inner, PaLock, PulseEngine};

/// What a peak meter taps. Keyed into `Inner::meters` by the driver's
/// endpoint string (`src:{source name}` / `in:{sink-input index}`),
/// which also encodes the target — an `in:` key embeds its index, so a
/// stream-index change is naturally a key change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeterTarget {
    /// `connect_record` on this source name — a capture device or any
    /// sink's `{name}.monitor`.
    Source(String),
    /// `set_monitor_stream` on this sink-input index, then
    /// `connect_record(None)` — one app's audio.
    SinkInput(u32),
}

/// One attached peak stream plus its latest reading.
pub(crate) struct Meter {
    pub(crate) stream: Stream,
    pub(crate) target: MeterTarget,
    /// Latest peak, 0.0..=1.0 — decays each [`PulseEngine::peaks`] poll
    /// so a stalled stream fades instead of freezing mid-deflection.
    peak: f32,
}

/// Multiplier applied to the cached peak on every poll *before* taking
/// the max with fresh data — VU-style fall-off (~0.4 s to silence at the
/// driver's 120 ms emit tick).
const PEAK_DECAY: f32 = 0.75;

/// Peak fragments are little-endian f32 samples; the *last* sample in a
/// fragment is the most recent reading (pavucontrol reads it the same
/// way). `None` for empty/short data and non-finite values.
fn last_peak(data: &[u8]) -> Option<f32> {
    // Whole samples only — a fragment is always a multiple of frame
    // size, but a trailing partial is dropped rather than misparsed.
    let whole = data.get(..data.len() / 4 * 4)?;
    let v = f32::from_le_bytes(*whole.last_chunk::<4>()?);
    v.is_finite().then(|| v.clamp(0.0, 1.0))
}

impl PulseEngine {
    /// Attaches a peak meter under `key`, or replaces it when `target`
    /// changed / the stream died. Idempotent: same target on a live
    /// stream is a no-op, which is what lets the driver call this for
    /// every desired endpoint on every reconcile — a meter whose stream
    /// failed (source vanished, `connect_record` rejected) is recreated
    /// here instead of staying dead until the set changes.
    ///
    /// Failures are logged, not returned: a meter that can't attach is a
    /// flat meter, not a broken engine.
    pub fn set_meter(&self, key: &str, target: &MeterTarget) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            return;
        }
        if let Some(meter) = inner.meters.get(key)
            && meter.target == *target
            && !matches!(
                meter.stream.get_state(),
                stream::State::Failed | stream::State::Terminated
            )
        {
            return;
        }
        remove_meter_locked(&mut inner, key);

        // pavucontrol's spec: 25 Hz mono f32 — the server downmixes and
        // peaks for us. fragsize=4 asks for one sample per fragment.
        let spec = Spec {
            format: Format::F32le,
            rate: 25,
            channels: 1,
        };
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: 4,
        };
        let Some(mut stream) = Stream::new(&mut inner.context, "fio peak meter", &spec, None)
        else {
            eprintln!("fio: peak meter {key}: failed to create stream");
            return;
        };
        if let MeterTarget::SinkInput(index) = target {
            let _ = stream.set_monitor_stream(*index);
        }
        let flags = stream::FlagSet::PEAK_DETECT | stream::FlagSet::DONT_MOVE;
        let dev = match target {
            MeterTarget::Source(name) => Some(name.as_str()),
            MeterTarget::SinkInput(_) => None,
        };
        match stream.connect_record(dev, Some(&attr), flags) {
            Ok(()) => {
                inner.meters.insert(
                    key.to_string(),
                    Meter {
                        stream,
                        target: target.clone(),
                        peak: 0.0,
                    },
                );
            }
            Err(e) => {
                eprintln!("fio: peak meter {key}: connect_record failed: {e}");
                let _ = stream.disconnect();
            }
        }
    }

    /// Removes the meter under `key` — disconnects its stream under the
    /// PA lock and drops it. Unknown keys are a no-op.
    pub fn remove_meter(&self, key: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _pa_lock = PaLock::acquire(&mut inner.mainloop);
        remove_meter_locked(&mut inner, key);
    }

    /// Drains every attached stream's pending peak data and returns the
    /// latest peak per endpoint key (0.0..=1.0). Called on the driver's
    /// ~120 ms emit tick while the engine lives. Streams still
    /// `Creating` report their decayed cache; `Failed`/`Terminated`
    /// report 0 — the next reconcile's [`Self::set_meter`] recreates
    /// them if they're still wanted.
    pub fn peaks(&self) -> HashMap<String, f32> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let _pa_lock = PaLock::acquire(&mut inner.mainloop);
        if inner.context.get_state() != context::State::Ready {
            return HashMap::new();
        }
        let mut out = HashMap::with_capacity(inner.meters.len());
        for (key, meter) in inner.meters.iter_mut() {
            let mut latest = meter.peak * PEAK_DECAY;
            match meter.stream.get_state() {
                stream::State::Ready => loop {
                    // Each peek() yields one fragment; drain until Empty.
                    let next = match meter.stream.peek() {
                        Ok(PeekResult::Data(data)) => last_peak(data),
                        // A hole is dropped data — discard and keep going.
                        Ok(PeekResult::Hole(_)) => None,
                        Ok(PeekResult::Empty) | Err(_) => break,
                    };
                    let _ = meter.stream.discard();
                    if let Some(v) = next {
                        latest = latest.max(v);
                    }
                },
                stream::State::Failed | stream::State::Terminated => {
                    latest = 0.0;
                }
                _ => {} // Unconnected/Creating: nothing to drain yet.
            }
            meter.peak = latest;
            out.insert(key.clone(), latest);
        }
        out
    }
}

/// `set_meter`/`remove_meter`'s shared tail — caller holds `inner` and
/// the PA lock. Dropping the `Stream` unrefs it; the binding does not
/// disconnect for us, so disconnect first.
fn remove_meter_locked(inner: &mut Inner, key: &str) {
    if let Some(mut meter) = inner.meters.remove(key) {
        let _ = meter.stream.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32le(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn last_peak_reads_final_sample() {
        // Multi-sample fragment: the last f32 wins.
        let data: Vec<u8> = [f32le(0.2), f32le(0.8), f32le(0.4)].concat();
        assert_eq!(last_peak(&data), Some(0.4));
    }

    #[test]
    fn last_peak_clamps_and_rejects_garbage() {
        assert_eq!(last_peak(&f32le(1.7)), Some(1.0));
        assert_eq!(last_peak(&f32le(-0.3)), Some(0.0));
        assert_eq!(last_peak(&f32le(f32::NAN)), None);
        assert_eq!(last_peak(&[]), None);
        // A trailing partial sample is ignored.
        let mut data = f32le(0.5).to_vec();
        data.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!(last_peak(&data), Some(0.5));
    }
}
