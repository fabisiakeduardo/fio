//! Pure meter-set planning: which endpoints the engine should attach
//! peak streams to, and which `fio-levels` event keys map onto them.
//!
//! Two key spaces:
//!
//! - **Endpoint keys** — `src:{source name}` / `in:{sink-input index}`.
//!   They identify the peak stream on the engine and dedupe naturally:
//!   two routes reading the same mic (or two `AppName` rules matching
//!   the same live stream) share one endpoint, and an `in:` key embeds
//!   its index so a re-resolved app stream is a new endpoint — the
//!   driver's add/remove diff handles the swap.
//! - **UI keys** — the `fio-levels` payload: `dev:{id}` is a virtual
//!   device's output level (its `fio-{slug}.monitor` source);
//!   `route:{id}` is the audio a route carries (capture source,
//!   source-device monitor, or matching app's stream); `mon:{sink}` is
//!   a monitor feed's *destination* level — the target sink's own
//!   `.monitor` — so monitor cards show what actually lands on the
//!   output.
//!
//! Anything missing from the plan (a capture source that's unplugged, an
//! `AppName` rule with no matching stream) simply produces no key — the
//! UI treats absent keys as flat.

use std::collections::HashMap;

use crate::audio::{ActualState, MeterTarget};
use crate::model::{PersistedState, RouteSource};

use super::{monitor_name, rule_key, target_name};

/// What [`meter_plan`] computes from one desired/actual pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeterPlan {
    /// Endpoint key → the [`MeterTarget`] the engine should have
    /// attached. Compared against the driver's attached set to diff
    /// adds/removes.
    pub endpoints: HashMap<String, MeterTarget>,
    /// `fio-levels` payload key → endpoint key the level is read from.
    /// UI keys whose endpoint never materializes stay out of the map —
    /// the payload omits them and the UI renders flat.
    pub ui_keys: HashMap<String, String>,
}

/// Derives the full meter set for the current connection. Only sources
/// present in `actual` get an endpoint — no point attaching a record
/// stream to a name the server doesn't have; drift already flags the
/// missing end.
pub fn meter_plan(desired: &PersistedState, actual: &ActualState) -> MeterPlan {
    let mut plan = MeterPlan::default();
    let source_exists = |name: &str| actual.sources.iter().any(|s| s.name == name);

    // Registers `name` as a `src:` endpoint if the server has it;
    // returns the endpoint key for `ui_keys`.
    let src_endpoint = |name: &str, plan: &mut MeterPlan| -> Option<String> {
        if !source_exists(name) {
            return None;
        }
        let key = format!("src:{name}");
        plan.endpoints
            .insert(key.clone(), MeterTarget::Source(name.to_string()));
        Some(key)
    };

    for device in &desired.devices {
        if let Some(ep) = src_endpoint(&monitor_name(device), &mut plan) {
            plan.ui_keys.insert(format!("dev:{}", device.id), ep);
        }
    }

    for route in &desired.routes {
        let ui_key = format!("route:{}", route.id);
        match &route.source {
            RouteSource::CaptureDevice(name) => {
                if let Some(ep) = src_endpoint(name, &mut plan) {
                    plan.ui_keys.insert(ui_key, ep);
                }
            }
            RouteSource::VirtualMonitor(id) => {
                // The level on the wire into the target is the source
                // device's own monitor output.
                if let Some(source_dev) = desired.devices.iter().find(|d| &d.id == id)
                    && let Some(ep) = src_endpoint(&monitor_name(source_dev), &mut plan)
                {
                    plan.ui_keys.insert(ui_key, ep);
                }
                // Monitor cards meter the feed's destination: the target
                // sink's `.monitor` source, when it exists.
                if let Some(sink) = target_name(desired, &route.target)
                    && let Some(ep) = src_endpoint(&format!("{sink}.monitor"), &mut plan)
                {
                    plan.ui_keys.insert(format!("mon:{sink}"), ep);
                }
            }
            RouteSource::AppName(app) => {
                // First live sink-input matching the rule — re-resolved
                // every reconcile since stream indices churn.
                if let Some(stream) = actual
                    .sink_inputs
                    .iter()
                    .find(|s| rule_key(s) == Some(app.as_str()))
                {
                    let ep = format!("in:{}", stream.index);
                    plan.endpoints
                        .insert(ep.clone(), MeterTarget::SinkInput(stream.index));
                    plan.ui_keys.insert(ui_key, ep);
                }
            }
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{SinkInfo, SourceInfo, StreamInfo};
    use crate::model::{Route, RouteSource, RouteTarget, VirtualDevice};
    use uuid::Uuid;

    fn mk_device(name: &str, slug: &str) -> VirtualDevice {
        VirtualDevice {
            id: Uuid::new_v4(),
            name: name.to_string(),
            slug: slug.to_string(),
        }
    }

    fn mk_route(source: RouteSource, target: RouteTarget) -> Route {
        Route {
            id: Uuid::new_v4(),
            source,
            target,
        }
    }

    fn mk_sink(index: u32, name: &str) -> SinkInfo {
        SinkInfo {
            index,
            name: name.to_string(),
            description: None,
            module: None,
        }
    }

    fn mk_source(index: u32, name: &str) -> SourceInfo {
        SourceInfo {
            index,
            name: name.to_string(),
            description: None,
            module: None,
            monitor_of_sink: None,
        }
    }

    fn mk_stream(index: u32, app: Option<&str>, sink: u32) -> StreamInfo {
        StreamInfo {
            index,
            app_name: app.map(str::to_string),
            media_name: None,
            sink,
        }
    }

    /// Devices Game/Chat (sinks + monitors live), capture source
    /// `alsa_in`, physical sink `alsa_out` + its monitor, one live
    /// "spotify" stream.
    fn fixture() -> (PersistedState, ActualState) {
        let game = mk_device("Game", "game");
        let chat = mk_device("Chat", "chat");
        let desired = PersistedState {
            version: 1,
            devices: vec![game.clone(), chat.clone()],
            routes: vec![
                mk_route(
                    RouteSource::AppName("spotify".to_string()),
                    RouteTarget::VirtualDevice(game.id),
                ),
                mk_route(
                    RouteSource::CaptureDevice("alsa_in".to_string()),
                    RouteTarget::VirtualDevice(game.id),
                ),
                mk_route(
                    RouteSource::VirtualMonitor(game.id),
                    RouteTarget::PhysicalSink("alsa_out".to_string()),
                ),
            ],
            seen_apps: vec![],
        };
        let actual = ActualState {
            sinks: vec![
                mk_sink(0, "fio-game"),
                mk_sink(1, "fio-chat"),
                mk_sink(2, "alsa_out"),
            ],
            sources: vec![
                mk_source(3, "fio-game.monitor"),
                mk_source(4, "fio-chat.monitor"),
                mk_source(5, "alsa_in"),
                mk_source(6, "alsa_out.monitor"),
            ],
            sink_inputs: vec![mk_stream(42, Some("spotify"), 0)],
            modules: vec![],
        };
        (desired, actual)
    }

    #[test]
    fn devices_and_routes_produce_ui_keys() {
        let (desired, actual) = fixture();
        let plan = meter_plan(&desired, &actual);
        let game = &desired.devices[0];

        // Device output meter + one key per route.
        assert_eq!(
            plan.ui_keys[&format!("dev:{}", game.id)],
            "src:fio-game.monitor"
        );
        for route in &desired.routes {
            assert!(plan.ui_keys.contains_key(&format!("route:{}", route.id)));
        }
        // Monitor feed also registers its destination's monitor.
        assert_eq!(plan.ui_keys["mon:alsa_out"], "src:alsa_out.monitor");
    }

    #[test]
    fn endpoints_dedupe_shared_sources() {
        let (mut desired, actual) = fixture();
        let chat = desired.devices[1].clone();
        // A second route onto the same capture source must not create a
        // second endpoint — one stream, two UI keys.
        desired.routes.push(mk_route(
            RouteSource::CaptureDevice("alsa_in".to_string()),
            RouteTarget::VirtualDevice(chat.id),
        ));
        let plan = meter_plan(&desired, &actual);

        let src_count = plan
            .endpoints
            .values()
            .filter(|t| **t == MeterTarget::Source("alsa_in".to_string()))
            .count();
        assert_eq!(src_count, 1);
        // Same for the device monitor: dev key + monitor-route key share
        // one `src:fio-game.monitor` endpoint.
        assert_eq!(
            plan.endpoints
                .values()
                .filter(|t| **t == MeterTarget::Source("fio-game.monitor".to_string()))
                .count(),
            1
        );
    }

    #[test]
    fn app_route_uses_matching_stream_index() {
        let (desired, actual) = fixture();
        let route_id = desired.routes[0].id;
        let plan = meter_plan(&desired, &actual);
        assert_eq!(plan.ui_keys[&format!("route:{route_id}")], "in:42");
        assert_eq!(plan.endpoints["in:42"], MeterTarget::SinkInput(42));
    }

    #[test]
    fn re_resolved_app_stream_is_a_new_endpoint() {
        // The stream index churns between reconciles: `in:` embeds it,
        // so the diff removes `in:42` and adds `in:99` — no stale peak
        // carried across streams.
        let (desired, mut actual) = fixture();
        actual.sink_inputs[0].index = 99;
        let plan = meter_plan(&desired, &actual);
        assert!(plan.endpoints.contains_key("in:99"));
        assert!(!plan.endpoints.contains_key("in:42"));
    }

    #[test]
    fn missing_sources_and_streams_yield_no_keys() {
        let (mut desired, mut actual) = fixture();
        // Unplug the capture source and kill the app stream.
        actual.sources.retain(|s| s.name != "alsa_in");
        actual.sink_inputs.clear();
        // Rule for an app that isn't playing.
        desired.routes.push(mk_route(
            RouteSource::AppName("slack".to_string()),
            RouteTarget::VirtualDevice(desired.devices[0].id),
        ));
        let plan = meter_plan(&desired, &actual);

        let cap_route = &desired.routes[1];
        assert!(!plan.ui_keys.contains_key(&format!("route:{}", cap_route.id)));
        assert!(!plan.ui_keys.contains_key("route:slack-never"));
        assert!(
            !plan
                .endpoints
                .values()
                .any(|t| matches!(t, MeterTarget::SinkInput(_)))
        );
        // Devices and the monitor feed still meter fine.
        assert!(plan.ui_keys.contains_key("mon:alsa_out"));
    }

    #[test]
    fn monitor_route_to_virtual_device_meters_its_monitor() {
        let (mut desired, actual) = fixture();
        let game = desired.devices[0].clone();
        let chat = desired.devices[1].clone();
        desired.routes.push(mk_route(
            RouteSource::VirtualMonitor(chat.id),
            RouteTarget::VirtualDevice(game.id),
        ));
        let plan = meter_plan(&desired, &actual);
        assert_eq!(plan.ui_keys["mon:fio-game"], "src:fio-game.monitor");
    }
}
