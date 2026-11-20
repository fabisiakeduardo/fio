use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::route::Route;

/// A user-created virtual audio device.
///
/// `slug` is the module-safe name WITHOUT the "fio-" prefix;
/// sink name = `format!("fio-{slug}")`, source = `format!("fio-{slug}.monitor")`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualDevice {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub devices: Vec<VirtualDevice>,
    pub routes: Vec<Route>,
    pub seen_apps: Vec<String>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: 1,
            devices: Vec::new(),
            routes: Vec::new(),
            seen_apps: Vec::new(),
        }
    }
}

/// Lowercase, `[a-z0-9-]`, collapse runs, no leading/trailing '-'.
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    // Start "in a dash run" so leading separators produce no leading '-'.
    let mut in_dash = true;
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            in_dash = false;
        } else if !in_dash {
            slug.push('-');
            in_dash = true;
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Load persisted state; missing file => default `{ version: 1, .. }`.
pub fn load_state(path: &Path) -> PersistedState {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return PersistedState::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

pub fn save_state(path: &Path, s: &PersistedState) -> io::Result<()> {
    let json = serde_json::to_string_pretty(s)?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fio-test-{}-{}", name, Uuid::new_v4()))
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Mic!"), "my-mic");
    }

    #[test]
    fn state_roundtrip() {
        let state = PersistedState {
            version: 1,
            devices: vec![VirtualDevice {
                id: Uuid::new_v4(),
                name: "My Mic".to_string(),
                slug: "my-mic".to_string(),
            }],
            routes: vec![Route {
                id: Uuid::new_v4(),
                source: crate::model::RouteSource::AppName("game".to_string()),
                target: crate::model::RouteTarget::VirtualDevice(Uuid::new_v4()),
            }],
            seen_apps: vec!["game".to_string(), "discord".to_string()],
        };

        let path = temp_path("roundtrip");
        save_state(&path, &state).expect("save_state failed");
        let loaded = load_state(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded, state);
    }

    #[test]
    fn load_missing_returns_default() {
        let path = temp_path("missing");
        let loaded = load_state(&path);

        assert_eq!(loaded.version, 1);
        assert!(loaded.devices.is_empty());
        assert!(loaded.routes.is_empty());
        assert!(loaded.seen_apps.is_empty());
    }
}
