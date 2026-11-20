use std::path::PathBuf;

use uuid::Uuid;

use super::route::{Route, RouteSource, RouteTarget};
use super::state::{PersistedState, VirtualDevice, load_state, save_state, slugify};

/// Maximum number of recently-seen app names retained in `seen_apps`.
const SEEN_APPS_MAX: usize = 50;

/// Owns the persisted state and its on-disk path. Every mutation is
/// written back via `save_state` so the file always mirrors memory.
pub struct Store {
    state: PersistedState,
    path: PathBuf,
}

impl Store {
    /// Load state from `path` (missing/invalid file => default state).
    pub fn load(path: PathBuf) -> Self {
        Self {
            state: load_state(&path),
            path,
        }
    }

    /// Add a virtual device, deriving a unique slug from `name`.
    /// On collision, appends `-2`, `-3`, ... until the slug is unique.
    pub fn add_device(&mut self, name: String) -> VirtualDevice {
        let slug = self.unique_slug(&slugify(&name));
        let device = VirtualDevice {
            id: Uuid::new_v4(),
            name,
            slug,
        };
        self.state.devices.push(device.clone());
        self.persist();
        device
    }

    /// Remove the device with `id`; also drops routes targeting it AND
    /// routes sourcing from its monitor — a `VirtualMonitor` source left
    /// behind would dangle exactly like a `VirtualDevice` target.
    /// Returns the removed device, or `None` if not found.
    pub fn remove_device(&mut self, id: Uuid) -> Option<VirtualDevice> {
        let pos = self.state.devices.iter().position(|d| d.id == id)?;
        let device = self.state.devices.remove(pos);
        self.state.routes.retain(|r| {
            r.target != RouteTarget::VirtualDevice(id)
                && r.source != RouteSource::VirtualMonitor(id)
        });
        self.persist();
        Some(device)
    }

    /// Add a route; an identical (source, target) route already present
    /// is returned unchanged (same id), so routes are deduplicated.
    pub fn add_route(&mut self, source: RouteSource, target: RouteTarget) -> Route {
        if let Some(existing) = self
            .state
            .routes
            .iter()
            .find(|r| r.source == source && r.target == target)
        {
            return existing.clone();
        }
        let route = Route {
            id: Uuid::new_v4(),
            source,
            target,
        };
        self.state.routes.push(route.clone());
        self.persist();
        route
    }

    /// Remove the route with `id`. Returns it, or `None` if not found.
    pub fn remove_route(&mut self, id: Uuid) -> Option<Route> {
        let pos = self.state.routes.iter().position(|r| r.id == id)?;
        let route = self.state.routes.remove(pos);
        self.persist();
        Some(route)
    }

    /// Record a seen app name, most-recently-used first, capped at 50.
    /// Re-recording an existing name moves it to the front.
    ///
    /// Memory-only: stream events arrive too often to justify a disk
    /// write each. Callers flush on a cadence — the driver persists at
    /// most once per few seconds and once more on shutdown.
    pub fn record_seen_app(&mut self, app_name: &str) {
        self.state.seen_apps.retain(|a| a != app_name);
        self.state.seen_apps.insert(0, app_name.to_string());
        self.state.seen_apps.truncate(SEEN_APPS_MAX);
    }

    /// Persist accumulated state — the counterpart of
    /// [`Self::record_seen_app`]'s deferred write.
    pub fn flush(&self) {
        self.persist();
    }

    pub fn state(&self) -> &PersistedState {
        &self.state
    }

    fn unique_slug(&self, base: &str) -> String {
        if !self.state.devices.iter().any(|d| d.slug == base) {
            return base.to_string();
        }
        for n in 2.. {
            let candidate = format!("{base}-{n}");
            if !self.state.devices.iter().any(|d| d.slug == candidate) {
                return candidate;
            }
        }
        unreachable!()
    }

    fn persist(&self) {
        if let Err(e) = save_state(&self.path, &self.state) {
            eprintln!("fio: failed to save state to {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_store_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fio-test-store-{}-{}", name, Uuid::new_v4()))
    }

    #[test]
    fn add_device_slug_collision() {
        let path = temp_store_path("slug-collision");
        let mut store = Store::load(path.clone());

        let first = store.add_device("My Mic".to_string());
        let second = store.add_device("My Mic".to_string());

        assert_eq!(first.slug, "my-mic");
        assert_eq!(second.slug, "my-mic-2");
        assert_ne!(first.id, second.id);
        assert_eq!(store.state().devices.len(), 2);

        // Mutations are persisted: a fresh load sees both devices.
        assert_eq!(Store::load(path.clone()).state().devices.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_route_dedups() {
        let path = temp_store_path("route-dedup");
        let mut store = Store::load(path.clone());

        let source = RouteSource::AppName("game".to_string());
        let target = RouteTarget::PhysicalSink("alsa_output.pci".to_string());

        let first = store.add_route(source.clone(), target.clone());
        let second = store.add_route(source, target);

        assert_eq!(first.id, second.id);
        assert_eq!(store.state().routes.len(), 1);

        assert_eq!(Store::load(path.clone()).state().routes.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_device_drops_its_routes() {
        let path = temp_store_path("remove-device");
        let mut store = Store::load(path.clone());

        let device = store.add_device("My Mic".to_string());
        let other = store.add_device("Other".to_string());

        let doomed = store.add_route(
            RouteSource::AppName("game".to_string()),
            RouteTarget::VirtualDevice(device.id),
        );
        // A route sourcing FROM the device's monitor dangles just as hard
        // as one targeting it — removal drops both.
        let doomed_source = store.add_route(
            RouteSource::VirtualMonitor(device.id),
            RouteTarget::PhysicalSink("alsa_out".to_string()),
        );
        let kept = store.add_route(
            RouteSource::AppName("music".to_string()),
            RouteTarget::VirtualDevice(other.id),
        );

        let removed = store.remove_device(device.id);
        assert_eq!(removed.as_ref(), Some(&device));
        assert!(store.state().routes.iter().all(|r| r.id != doomed.id));
        assert!(
            store
                .state()
                .routes
                .iter()
                .all(|r| r.id != doomed_source.id)
        );
        assert!(store.state().routes.iter().any(|r| r.id == kept.id));

        // Removal is persisted too.
        let reloaded = Store::load(path.clone());
        assert!(reloaded.state().devices.iter().all(|d| d.id != device.id));
        assert_eq!(reloaded.state().routes.len(), 1);

        // Removing again returns None.
        assert_eq!(store.remove_device(device.id), None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn seen_apps_mru_capped() {
        let path = temp_store_path("seen-apps");
        let mut store = Store::load(path.clone());

        for i in 0..60 {
            store.record_seen_app(&format!("app-{i}"));
        }

        let seen = &store.state().seen_apps;
        assert_eq!(seen.len(), 50);
        assert_eq!(seen[0], "app-59");
        assert_eq!(seen[49], "app-10");

        // Re-seeing an app moves it to the front without duplicating.
        store.record_seen_app("app-59");
        let seen = &store.state().seen_apps;
        assert_eq!(seen.len(), 50);
        assert_eq!(seen[0], "app-59");
        assert_eq!(seen.iter().filter(|a| *a == "app-59").count(), 1);

        // Writes are deferred: nothing on disk until `flush` — the
        // driver's debounce cadence is what persists them.
        assert!(Store::load(path.clone()).state().seen_apps.is_empty());
        store.flush();
        assert_eq!(Store::load(path.clone()).state().seen_apps.len(), 50);

        let _ = std::fs::remove_file(&path);
    }
}
