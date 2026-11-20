//! Pure desired-vs-actual planning: given what the user configured
//! ([`PersistedState`]) and what the server currently has ([`ActualState`]),
//! decide which [`Action`]s close the gap and which gaps can only be
//! reported as [`Drift`].
//!
//! Naming (binding): a virtual device's sink is `fio-{slug}` and its mic
//! source resolves to `fio-{slug}.monitor` — the monitor pipewire-pulse
//! creates for every sink. The `source_name=fio-{slug}-mic` modarg that
//! [`null_sink_args`] still emits is aspirational metadata the server
//! ignores.
//!
//! Resolution:
//! - `RouteTarget::VirtualDevice(id)` names the sink `fio-{slug}` of that
//!   device; `RouteTarget::PhysicalSink(name)` names the sink verbatim.
//!   A target *resolves* only when a sink with that name exists in
//!   [`ActualState`].
//! - `RouteSource::CaptureDevice(name)` names a source verbatim — a real
//!   source in [`ActualState`]. `RouteSource::VirtualMonitor(id)` names
//!   `fio-{slug}.monitor` of that device. `RouteSource::AppName` names no
//!   source; it matches streams via [`rule_key`] instead.
//! - A capture/monitor route's link is present when a `module-loopback`
//!   [`ModuleInfo`]'s argument carries both the resolved `source=` and
//!   `sink=` values.

use uuid::Uuid;

use crate::audio::{ActualState, StreamInfo, loopback_args, null_sink_args};
use crate::model::{PersistedState, RouteSource, RouteTarget, VirtualDevice};

/// Something the executor can do to bring actual state closer to desired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `pa_context_load_module` — `name` e.g. `"module-null-sink"`,
    /// `argument` a `pactl`-style arg string built by [`null_sink_args`] /
    /// [`loopback_args`].
    LoadModule { name: String, argument: String },
    /// `pa_context_unload_module` on a per-session module index.
    UnloadModule { index: u32 },
    /// `move_sink_input_by_index` — carries the sink *name*; the executor
    /// resolves it to the current sink index.
    MoveStream {
        stream_index: u32,
        sink_name: String,
    },
    /// `pa_context_set_default_source` — the "use as default mic" action.
    SetDefaultSource { name: String },
}

/// Desired state missing from actual state that reconcile reports rather
/// than silently fixes — enforcement never fights the user mid-session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// Our `fio-{slug}` sink is absent from [`ActualState`] (e.g. an
    /// external `pactl unload-module`).
    MissingDevice { device_id: Uuid },
    /// A route's target sink — or a link route's source — is absent from
    /// [`ActualState`], including references to deleted devices.
    MissingTarget { route_id: Uuid },
    /// Both ends of a capture/monitor route exist but no `module-loopback`
    /// instance carries its `source=`/`sink=` pair.
    MissingLink { route_id: Uuid },
}

/// The startup heal pass, in executor order:
///
/// 1. Adopt-by-name: a sink already named `fio-{slug}` is kept, not
///    reloaded; only absent devices get a `module-null-sink` load.
/// 2. `MoveStream` for each live sink-input matching an `AppName` rule
///    whose target resolves — with one startup-only relaxation: a
///    `VirtualDevice` target counts as resolvable whenever the device is
///    in desired state, because step 1 (re)loads every absent `fio-*`
///    sink and `MoveStream` carries a name the executor resolves at
///    apply time. `PhysicalSink` targets stay strict.
/// 3. `module-loopback` for each capture/monitor route whose link is
///    absent. These loads come last so that ends this same plan
///    (re)creates — our `fio-*` sinks and their `.monitor` sources —
///    exist by the time the executor reaches them. Ends Fio cannot
///    conjure (a missing physical capture source or physical target
///    sink) are not linked; drift flags them instead.
pub fn plan_startup(desired: &PersistedState, actual: &ActualState) -> Vec<Action> {
    let mut actions = Vec::new();

    for device in &desired.devices {
        if !actual.sinks.iter().any(|s| s.name == sink_name(device)) {
            actions.push(Action::LoadModule {
                name: "module-null-sink".to_string(),
                argument: null_sink_args(&device.name, &device.slug),
            });
        }
    }

    for stream in &actual.sink_inputs {
        // VirtualDevice targets resolve by name here (the device being in
        // desired means its sink exists or is being loaded above) — unlike
        // `plan_new_stream`, which requires the sink in `actual` because
        // mid-session a missing device is drift, not a pending load.
        if let Some(action) = plan_stream_move(stream, desired, actual, |target| match target {
            RouteTarget::VirtualDevice(_) => target_name(desired, target),
            RouteTarget::PhysicalSink(_) => resolve_target(desired, actual, target),
        }) {
            actions.push(action);
        }
    }

    for route in &desired.routes {
        let (Some(source), Some(sink)) = (
            link_source_name(desired, &route.source),
            target_name(desired, &route.target),
        ) else {
            continue; // AppName rule, or a route referencing a deleted device.
        };
        // Ends Fio cannot conjure must already exist: an absent physical
        // capture source or physical target sink can't be linked now —
        // drift will flag it. Virtual ends are always acceptable: this
        // plan's own module-null-sink loads precede the loopback in
        // `actions`, so a missing `fio-{slug}` sink / `.monitor` source
        // exists by the time the executor applies the link.
        let source_ready = !matches!(route.source, RouteSource::CaptureDevice(_))
            || actual.sources.iter().any(|s| s.name == source);
        let target_ready = !matches!(route.target, RouteTarget::PhysicalSink(_))
            || actual.sinks.iter().any(|s| s.name == sink);
        if source_ready && target_ready && !link_present(actual, &source, &sink) {
            actions.push(Action::LoadModule {
                name: "module-loopback".to_string(),
                argument: loopback_args(&source, &sink),
            });
        }
    }

    actions
}

/// The mid-session enforcement step for one new sink-input: `MoveStream`
/// only when the stream isn't already on the right sink AND an `AppName`
/// rule matches its [`rule_key`] AND the rule's target resolves to a sink
/// that exists in `actual`. `None` = leave the stream alone.
pub fn plan_new_stream(
    stream: &StreamInfo,
    desired: &PersistedState,
    actual: &ActualState,
) -> Option<Action> {
    plan_stream_move(stream, desired, actual, |target| {
        resolve_target(desired, actual, target)
    })
}

/// Shared stream-move rule: `MoveStream` when the stream isn't already on
/// the target sink AND an `AppName` rule matches its [`rule_key`] AND
/// `resolve` yields the target's sink name. `resolve` is what differs
/// between the startup pass (virtual ends pending load count) and
/// mid-session events (strict actual-presence). A stream on a sink absent
/// from `actual` cannot already be on the target, so the presence check
/// doubles as the already-there check.
fn plan_stream_move(
    stream: &StreamInfo,
    desired: &PersistedState,
    actual: &ActualState,
    resolve: impl Fn(&RouteTarget) -> Option<String>,
) -> Option<Action> {
    let key = rule_key(stream)?;
    let route = desired
        .routes
        .iter()
        .find(|r| matches!(&r.source, RouteSource::AppName(app) if app.as_str() == key))?;
    let sink_name = resolve(&route.target)?;
    if let Some(sink) = actual.sinks.iter().find(|s| s.name == sink_name) {
        if stream.sink == sink.index {
            return None;
        }
    }
    Some(Action::MoveStream {
        stream_index: stream.index,
        sink_name,
    })
}

/// Report desired state missing from `actual`: removed devices,
/// unresolvable route ends, absent loopback links. Devices are checked
/// first, then routes in declaration order — the flag order is stable.
pub fn detect_drift(desired: &PersistedState, actual: &ActualState) -> Vec<Drift> {
    let mut drift = Vec::new();

    for device in &desired.devices {
        if !actual.sinks.iter().any(|s| s.name == sink_name(device)) {
            drift.push(Drift::MissingDevice {
                device_id: device.id,
            });
        }
    }

    for route in &desired.routes {
        let Some(sink) = resolve_target(desired, actual, &route.target) else {
            drift.push(Drift::MissingTarget { route_id: route.id });
            continue;
        };
        if matches!(route.source, RouteSource::AppName(_)) {
            continue; // App rules carry no link/source to check.
        }
        match link_source_name(desired, &route.source) {
            Some(source) if actual.sources.iter().any(|s| s.name == source) => {
                if !link_present(actual, &source, &sink) {
                    drift.push(Drift::MissingLink { route_id: route.id });
                }
            }
            // Capture source unplugged, monitor gone, or the route sources
            // a deleted device — the link's ends can't be reasoned about.
            _ => drift.push(Drift::MissingTarget { route_id: route.id }),
        }
    }

    drift
}

/// The value `AppName` rules match against: `application.name`, falling
/// back to `media.name` when it is absent. `None` when the stream has
/// neither — such streams can never match a rule.
pub fn rule_key(s: &StreamInfo) -> Option<&str> {
    s.app_name.as_deref().or(s.media_name.as_deref())
}

/// Our name for a device's sink.
fn sink_name(device: &VirtualDevice) -> String {
    format!("fio-{}", device.slug)
}

/// Our name for a device's mic source — the monitor pipewire-pulse
/// creates for its sink.
fn monitor_name(device: &VirtualDevice) -> String {
    format!("{}.monitor", sink_name(device))
}

/// The sink name a target points at, computable purely from desired
/// state. `None` when a `VirtualDevice` id references a deleted device.
fn target_name(desired: &PersistedState, target: &RouteTarget) -> Option<String> {
    match target {
        RouteTarget::VirtualDevice(id) => {
            desired.devices.iter().find(|d| &d.id == id).map(sink_name)
        }
        RouteTarget::PhysicalSink(name) => Some(name.clone()),
    }
}

/// A target *resolves* only when its sink exists in `actual` — the
/// pinned check for stream moves and drift.
fn resolve_target(
    desired: &PersistedState,
    actual: &ActualState,
    target: &RouteTarget,
) -> Option<String> {
    let name = target_name(desired, target)?;
    actual.sinks.iter().any(|s| s.name == name).then_some(name)
}

/// The source name a link route captures from, computable purely from
/// desired state. `None` for `AppName` rules (no link) and for
/// `VirtualMonitor` ids referencing a deleted device.
fn link_source_name(desired: &PersistedState, source: &RouteSource) -> Option<String> {
    match source {
        RouteSource::AppName(_) => None,
        RouteSource::CaptureDevice(name) => Some(name.clone()),
        RouteSource::VirtualMonitor(id) => desired
            .devices
            .iter()
            .find(|d| &d.id == id)
            .map(monitor_name),
    }
}

/// Is a `module-loopback` carrying this exact `source=`+`sink=` pair
/// loaded? Argument tokens are compared whole — `split_whitespace`
/// keeps `source=alsa_in` from matching `source=alsa_in_extra`.
fn link_present(actual: &ActualState, source: &str, sink: &str) -> bool {
    let want_source = format!("source={source}");
    let want_sink = format!("sink={sink}");
    actual.modules.iter().any(|m| {
        m.name == "module-loopback"
            && m.argument.as_deref().is_some_and(|args| {
                args.split_whitespace().any(|t| t == want_source)
                    && args.split_whitespace().any(|t| t == want_sink)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ModuleInfo, SinkInfo, SourceInfo};
    use crate::model::Route;

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

    fn mk_stream(index: u32, app: Option<&str>, media: Option<&str>, sink: u32) -> StreamInfo {
        StreamInfo {
            index,
            app_name: app.map(str::to_string),
            media_name: media.map(str::to_string),
            sink,
        }
    }

    fn mk_module(index: u32, name: &str, argument: Option<String>) -> ModuleInfo {
        ModuleInfo {
            index,
            name: name.to_string(),
            argument,
        }
    }

    /// One device "Game"/`fio-game` plus an `AppName("game")` rule
    /// targeting it.
    fn mk_state() -> PersistedState {
        let device = mk_device("Game", "game");
        let route = mk_route(
            RouteSource::AppName("game".to_string()),
            RouteTarget::VirtualDevice(device.id),
        );
        PersistedState {
            version: 1,
            devices: vec![device],
            routes: vec![route],
            seen_apps: vec![],
        }
    }

    /// The `fio-game` sink + its monitor already loaded, plus a physical
    /// sink `alsa_out` and capture source `alsa_in`. No streams, no
    /// modules.
    fn mk_actual() -> ActualState {
        ActualState {
            sinks: vec![mk_sink(0, "fio-game"), mk_sink(1, "alsa_out")],
            sources: vec![mk_source(2, "fio-game.monitor"), mk_source(3, "alsa_in")],
            sink_inputs: vec![],
            modules: vec![],
        }
    }

    #[test]
    fn startup_loads_missing_device_only() {
        // `fio-game` already exists -> adopted, not reloaded; the absent
        // `fio-chat` is the only LoadModule emitted.
        let mut desired = mk_state();
        desired.devices.push(mk_device("Chat", "chat"));
        let actual = mk_actual();

        assert_eq!(
            plan_startup(&desired, &actual),
            vec![Action::LoadModule {
                name: "module-null-sink".to_string(),
                argument: null_sink_args("Chat", "chat"),
            }],
        );
    }

    #[test]
    fn startup_moves_matching_live_stream() {
        // Rule AppName("game") + live stream with app_name "game" sitting
        // on `alsa_out` -> MoveStream to `fio-game`.
        let desired = mk_state();
        let mut actual = mk_actual();
        actual
            .sink_inputs
            .push(mk_stream(10, Some("game"), None, 1));

        assert_eq!(
            plan_startup(&desired, &actual),
            vec![Action::MoveStream {
                stream_index: 10,
                sink_name: "fio-game".to_string(),
            }],
        );
    }

    #[test]
    fn startup_ignores_unmatching_stream() {
        let desired = mk_state();
        let mut actual = mk_actual();
        actual
            .sink_inputs
            .push(mk_stream(10, Some("music"), Some("a track"), 1));

        assert_eq!(plan_startup(&desired, &actual), Vec::<Action>::new());
    }

    #[test]
    fn new_stream_matching_rule_moves() {
        let desired = mk_state();
        let actual = mk_actual();

        // application.name match.
        let stream = mk_stream(10, Some("game"), None, 1);
        assert_eq!(
            plan_new_stream(&stream, &desired, &actual),
            Some(Action::MoveStream {
                stream_index: 10,
                sink_name: "fio-game".to_string(),
            }),
        );
        // media.name fallback when application.name is absent.
        let stream = mk_stream(11, None, Some("game"), 1);
        assert_eq!(
            plan_new_stream(&stream, &desired, &actual),
            Some(Action::MoveStream {
                stream_index: 11,
                sink_name: "fio-game".to_string(),
            }),
        );
        // Already on the right sink -> nothing to do.
        let stream = mk_stream(12, Some("game"), None, 0);
        assert_eq!(plan_new_stream(&stream, &desired, &actual), None);
    }

    #[test]
    fn new_stream_no_name_never_matches() {
        // Review Focus #1: a stream with neither name must not crash
        // matching and must not match every rule.
        let desired = mk_state();
        let mut actual = mk_actual();
        let stream = mk_stream(10, None, None, 1);

        assert_eq!(plan_new_stream(&stream, &desired, &actual), None);

        actual.sink_inputs.push(stream);
        assert!(
            !plan_startup(&desired, &actual)
                .iter()
                .any(|a| matches!(a, Action::MoveStream { .. }))
        );
    }

    #[test]
    fn drift_flags_externally_removed_device() {
        // Review Focus #5: device in desired, sink absent -> MissingDevice,
        // never a silent reload.
        let desired = mk_state();
        let device_id = desired.devices[0].id;
        let route_id = desired.routes[0].id;
        let mut actual = mk_actual();
        actual.sinks.retain(|s| s.name != "fio-game");
        actual.sources.retain(|s| s.name != "fio-game.monitor");

        assert_eq!(
            detect_drift(&desired, &actual),
            vec![
                Drift::MissingDevice { device_id },
                // The rule's target sink is gone too — the route is broken.
                Drift::MissingTarget { route_id },
            ],
        );
    }

    #[test]
    fn drift_flags_missing_physical_target() {
        // Review Focus #3: target "alsa_x" absent -> MissingTarget, no
        // panic and no silent fallback to a wrong sink.
        let mut desired = mk_state();
        let route = mk_route(
            RouteSource::AppName("chat".to_string()),
            RouteTarget::PhysicalSink("alsa_x".to_string()),
        );
        let route_id = route.id;
        desired.routes.push(route);
        let actual = mk_actual();

        assert_eq!(
            detect_drift(&desired, &actual),
            vec![Drift::MissingTarget { route_id }],
        );
    }

    #[test]
    fn drift_flags_absent_loopback_link() {
        // Capture route, both ends present, but no module-loopback loaded.
        let mut desired = mk_state();
        let route = mk_route(
            RouteSource::CaptureDevice("alsa_in".to_string()),
            RouteTarget::VirtualDevice(desired.devices[0].id),
        );
        let route_id = route.id;
        desired.routes.push(route);
        let actual = mk_actual();

        assert_eq!(
            detect_drift(&desired, &actual),
            vec![Drift::MissingLink { route_id }],
        );
    }

    #[test]
    fn no_drift_when_all_present() {
        // App rule + capture route + monitor route; every end exists and
        // both loopback links are loaded.
        let mut desired = mk_state();
        let dev_id = desired.devices[0].id;
        desired.routes.push(mk_route(
            RouteSource::CaptureDevice("alsa_in".to_string()),
            RouteTarget::VirtualDevice(dev_id),
        ));
        desired.routes.push(mk_route(
            RouteSource::VirtualMonitor(dev_id),
            RouteTarget::PhysicalSink("alsa_out".to_string()),
        ));
        let mut actual = mk_actual();
        actual.modules.push(mk_module(
            7,
            "module-loopback",
            Some(loopback_args("alsa_in", "fio-game")),
        ));
        actual.modules.push(mk_module(
            8,
            "module-loopback",
            Some(loopback_args("fio-game.monitor", "alsa_out")),
        ));

        assert_eq!(detect_drift(&desired, &actual), Vec::<Drift>::new());
    }

    // Supplementary to the pinned nine: the heal side of
    // `drift_flags_absent_loopback_link` — ends present, link absent.
    #[test]
    fn startup_loads_absent_loopback_link() {
        let mut desired = mk_state();
        desired.routes.push(mk_route(
            RouteSource::CaptureDevice("alsa_in".to_string()),
            RouteTarget::VirtualDevice(desired.devices[0].id),
        ));
        let actual = mk_actual();

        assert_eq!(
            plan_startup(&desired, &actual),
            vec![Action::LoadModule {
                name: "module-loopback".to_string(),
                argument: loopback_args("alsa_in", "fio-game"),
            }],
        );
    }

    // Supplementary: pins the review fix — the same relaxation applies to
    // stream moves. On cold start a live stream matching a rule to a
    // VirtualDevice whose sink is absent still moves: this plan's own
    // null-sink load precedes the MoveStream, and the action carries a
    // name the executor resolves at apply time.
    #[test]
    fn startup_moves_stream_to_device_being_loaded() {
        let desired = mk_state();
        let mut actual = mk_actual();
        actual.sinks.retain(|s| s.name != "fio-game");
        actual.sources.retain(|s| s.name != "fio-game.monitor");
        actual
            .sink_inputs
            .push(mk_stream(10, Some("game"), None, 1));

        assert_eq!(
            plan_startup(&desired, &actual),
            vec![
                Action::LoadModule {
                    name: "module-null-sink".to_string(),
                    argument: null_sink_args("Game", "game"),
                },
                Action::MoveStream {
                    stream_index: 10,
                    sink_name: "fio-game".to_string(),
                },
            ],
        );
    }

    // Supplementary: pins the deliberate asymmetry — a virtual end that is
    // absent from `actual` still gets its link planned, because this plan's
    // own module-null-sink load precedes the loopback in the action list.
    #[test]
    fn startup_plans_link_to_device_being_loaded() {
        let mut desired = mk_state();
        let dev_id = desired.devices[0].id;
        desired.routes.push(mk_route(
            RouteSource::VirtualMonitor(dev_id),
            RouteTarget::PhysicalSink("alsa_out".to_string()),
        ));
        let mut actual = mk_actual();
        actual.sinks.retain(|s| s.name != "fio-game");
        actual.sources.retain(|s| s.name != "fio-game.monitor");

        assert_eq!(
            plan_startup(&desired, &actual),
            vec![
                Action::LoadModule {
                    name: "module-null-sink".to_string(),
                    argument: null_sink_args("Game", "game"),
                },
                Action::LoadModule {
                    name: "module-loopback".to_string(),
                    argument: loopback_args("fio-game.monitor", "alsa_out"),
                },
            ],
        );
    }
}
