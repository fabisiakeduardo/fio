use std::io;
use std::path::{Path, PathBuf};

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
/// A file that exists but doesn't parse is renamed to `<name>.bak` —
/// keeping it would re-warn on every launch while the user edits it by
/// hand — and default state is returned.
pub fn load_state(path: &Path) -> PersistedState {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return PersistedState::default();
    };
    match serde_json::from_str(&contents) {
        Ok(state) => state,
        Err(e) => {
            eprintln!(
                "fio: corrupt state file {} ({e}); resetting to defaults",
                path.display()
            );
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            let _ = std::fs::rename(path, PathBuf::from(bak));
            PersistedState::default()
        }
    }
}

/// Persist state to `path`, creating the parent directory if needed.
/// Writes go to a sibling `<name>.tmp` renamed over the target — a crash
/// mid-write can't leave a truncated `state.json`.
pub fn save_state(path: &Path, s: &PersistedState) -> io::Result<()> {
    let json = serde_json::to_string_pretty(s)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
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

    #[test]
    fn load_corrupt_resets_and_preserves_bak() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{ not json").unwrap();

        let loaded = load_state(&path);

        assert_eq!(loaded, PersistedState::default());
        // The corrupt file is preserved aside for debugging, and the load
        // path is clear so the next save starts fresh.
        assert!(!path.exists());
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        let bak = PathBuf::from(bak);
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "{ not json");

        let _ = std::fs::remove_file(&bak);
    }

    #[test]
    fn save_creates_missing_parent_dirs() {
        let dir = temp_path("nested");
        let path = dir.join("deep").join("state.json");
        save_state(&path, &PersistedState::default()).expect("save_state failed");

        assert!(path.exists());
        assert_eq!(load_state(&path), PersistedState::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_tmp_file() {
        let path = temp_path("atomic");
        save_state(&path, &PersistedState::default()).expect("save_state failed");

        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        assert!(!PathBuf::from(tmp).exists());

        let _ = std::fs::remove_file(&path);
    }
}
