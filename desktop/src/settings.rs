//! What the app remembers across restarts: a typed `Settings` struct saved
//! as JSON in `app_config_dir()/settings.json`.
//!
//! Reading never fails: a missing file gives the defaults, a field that is
//! missing or does not parse keeps its default, unknown fields are ignored,
//! and an unreadable file is set aside as `settings.json.corrupt`. Writes are
//! atomic (temp file + rename) and debounced.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use specta::Type;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The format written now. Bump it and add a step to `migrate` when a field
/// changes meaning or moves.
pub(crate) const SETTINGS_VERSION: u32 = 1;

/// How long to wait for more changes before writing the file.
const WRITE_DELAY: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Theme {
    /// Follow the system's light or dark preference.
    #[default]
    System,
    Dark,
    Light,
}

/// Defaults of the log viewer's controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub(crate) struct LogDefaults {
    /// Lines to start with: a number, or `"all"`.
    pub(crate) tail: String,
    pub(crate) timestamps: bool,
    pub(crate) wrap: bool,
}

impl Default for LogDefaults {
    fn default() -> Self {
        Self {
            tail: "200".into(),
            timestamps: false,
            wrap: false,
        }
    }
}

/// What is remembered per kubeconfig context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub(crate) struct ContextSettings {
    /// The last view: `"overview"` or a kind id (`apps/v1/deployments`).
    pub(crate) view: String,
    /// The selected namespaces (empty: all namespaces).
    pub(crate) namespaces: Vec<String>,
    /// Namespaces typed in by a user who may not list them.
    pub(crate) typed_namespaces: Vec<String>,
}

/// What is remembered per kind (by kind id).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub(crate) struct KindSettings {
    /// The column the table is sorted by (`""`: unsorted).
    pub(crate) sort_key: String,
    pub(crate) sort_desc: bool,
    /// Keys of the columns the user hid.
    pub(crate) hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) version: u32,
    /// The context to start in (`""`: the kubeconfig's current-context).
    pub(crate) last_context: String,
    pub(crate) theme: Theme,
    pub(crate) logs: LogDefaults,
    /// Check for new releases on start (used by the update check, #30).
    pub(crate) check_updates: bool,
    /// Width of the detail panel in pixels.
    pub(crate) detail_width: u32,
    /// Height of the bottom dock in pixels (`0`: its default).
    pub(crate) dock_height: u32,
    pub(crate) contexts: BTreeMap<String, ContextSettings>,
    pub(crate) kinds: BTreeMap<String, KindSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            last_context: String::new(),
            theme: Theme::System,
            logs: LogDefaults::default(),
            check_updates: true,
            detail_width: 420,
            dock_height: 0,
            contexts: BTreeMap::new(),
            kinds: BTreeMap::new(),
        }
    }
}

/// A change to the settings. Fields left out stay as they are; `contexts`
/// and `kinds` replace only the entries they name.
#[derive(Debug, Clone, Default, Deserialize, Type)]
pub(crate) struct SettingsPatch {
    #[specta(optional)]
    pub(crate) last_context: Option<String>,
    #[specta(optional)]
    pub(crate) theme: Option<Theme>,
    #[specta(optional)]
    pub(crate) logs: Option<LogDefaults>,
    #[specta(optional)]
    pub(crate) check_updates: Option<bool>,
    #[specta(optional)]
    pub(crate) detail_width: Option<u32>,
    #[specta(optional)]
    pub(crate) dock_height: Option<u32>,
    #[specta(optional)]
    pub(crate) contexts: Option<BTreeMap<String, ContextSettings>>,
    #[specta(optional)]
    pub(crate) kinds: Option<BTreeMap<String, KindSettings>>,
}

impl Settings {
    pub(crate) fn apply(&mut self, patch: SettingsPatch) {
        if let Some(v) = patch.last_context {
            self.last_context = v;
        }
        if let Some(v) = patch.theme {
            self.theme = v;
        }
        if let Some(v) = patch.logs {
            self.logs = v;
        }
        if let Some(v) = patch.check_updates {
            self.check_updates = v;
        }
        if let Some(v) = patch.detail_width {
            self.detail_width = v;
        }
        if let Some(v) = patch.dock_height {
            self.dock_height = v;
        }
        self.contexts.extend(patch.contexts.unwrap_or_default());
        self.kinds.extend(patch.kinds.unwrap_or_default());
    }
}

/// Bring a file written by an older version up to `SETTINGS_VERSION`.
/// Version 1 is the first format, so there is nothing to convert yet: a
/// file without a version (or an older one) keeps every field that still
/// parses.
fn migrate(mut obj: Map<String, Value>) -> Map<String, Value> {
    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);
    // Future steps go here, oldest first:
    // if version < 2 { /* rename or convert fields */ }
    let _ = version;
    obj.insert("version".into(), Value::from(SETTINGS_VERSION));
    obj
}

/// Parse a settings file, keeping every top-level field that parses and the
/// defaults for the rest. `None` if the text is not a JSON object at all.
pub(crate) fn parse(text: &str) -> Option<Settings> {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let obj = migrate(obj);
    if let Ok(s) = serde_json::from_value::<Settings>(Value::Object(obj.clone())) {
        return Some(s);
    }
    // One bad field must not cost the others: keep the fields that parse
    // on their own.
    let good: Map<String, Value> = obj
        .into_iter()
        .filter(|(k, v)| {
            let one = Map::from_iter([(k.clone(), v.clone())]);
            serde_json::from_value::<Settings>(Value::Object(one)).is_ok()
        })
        .collect();
    Some(serde_json::from_value(Value::Object(good)).unwrap_or_default())
}

/// Read the settings at `path`. A missing file gives the defaults; a corrupt
/// one is renamed to `<path>.corrupt` (so it is not overwritten) and gives
/// the defaults too.
pub(crate) fn load(path: &Path) -> Settings {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            eprintln!("kubeloupe: could not read {}: {e}", path.display());
            return Settings::default();
        }
    };
    if let Some(s) = parse(&text) {
        return s;
    }
    let aside = corrupt_path(path);
    eprintln!(
        "kubeloupe: {} is not valid settings JSON; moved it to {} and started with the defaults",
        path.display(),
        aside.display()
    );
    let _ = std::fs::rename(path, aside);
    Settings::default()
}

fn corrupt_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".corrupt");
    path.with_file_name(name)
}

/// Write `settings` to `path` atomically: a temp file next to it, then a
/// rename over it.
pub(crate) fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The settings in memory, written to disk shortly after each change.
pub(crate) struct SettingsStore {
    /// `None` when there is no config directory: settings last a session.
    path: Option<PathBuf>,
    current: Mutex<Settings>,
    /// Bumped on every change; a delayed write only runs if it is still the
    /// latest.
    generation: AtomicU64,
    dirty: AtomicBool,
}

impl SettingsStore {
    pub(crate) fn open(path: Option<PathBuf>) -> Arc<Self> {
        let current = path.as_deref().map(load).unwrap_or_default();
        Arc::new(Self {
            path,
            current: Mutex::new(current),
            generation: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        })
    }

    pub(crate) fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    /// Apply `patch` now; the file follows after `WRITE_DELAY` without
    /// further changes.
    pub(crate) fn update(self: &Arc<Self>, patch: SettingsPatch) -> Settings {
        let next = {
            let mut cur = self.current.lock().unwrap();
            cur.apply(patch);
            cur.clone()
        };
        self.dirty.store(true, Ordering::SeqCst);
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let store = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(WRITE_DELAY).await;
            if store.generation.load(Ordering::SeqCst) == gen {
                store.flush();
            }
        });
        next
    }

    /// Write pending changes now (also called when the app exits).
    pub(crate) fn flush(&self) {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let Some(path) = &self.path else { return };
        let settings = self.get();
        if let Err(e) = save(path, &settings) {
            eprintln!("kubeloupe: could not save {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kubeloupe-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let s = parse(r#"{"version":1,"theme":"light"}"#).unwrap();
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.detail_width, 420);
        assert_eq!(s.logs, LogDefaults::default());
        assert!(s.check_updates);
        assert!(s.contexts.is_empty());

        let s = parse(r#"{"contexts":{"kind":{"view":"v1/pods"}}}"#).unwrap();
        assert_eq!(s.contexts["kind"].view, "v1/pods");
        assert!(s.contexts["kind"].namespaces.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s = parse(
            r#"{"version":1,"theme":"dark","fromTheFuture":[1,2],"logs":{"wrap":true,"x":1}}"#,
        )
        .unwrap();
        assert_eq!(s.theme, Theme::Dark);
        assert!(s.logs.wrap);
        assert_eq!(s.logs.tail, "200");
    }

    #[test]
    fn a_bad_field_keeps_the_others() {
        let s = parse(r#"{"theme":"purple","detail_width":"wide","last_context":"prod"}"#).unwrap();
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.detail_width, 420);
        assert_eq!(s.last_context, "prod");
    }

    #[test]
    fn older_versions_are_migrated() {
        // No version (or 0): read what parses and write the current version.
        let s = parse(r#"{"last_context":"kind-test","kinds":{"v1/pods":{"sort_key":"age","sort_desc":true}}}"#)
            .unwrap();
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.last_context, "kind-test");
        assert_eq!(s.kinds["v1/pods"].sort_key, "age");
        assert!(s.kinds["v1/pods"].sort_desc);
        assert_eq!(parse(r#"{"version":0}"#).unwrap().version, SETTINGS_VERSION);
    }

    #[test]
    fn not_json_is_rejected() {
        assert!(parse("{not json").is_none());
        assert!(parse("[1,2]").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn a_corrupt_file_is_set_aside() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{\"theme\": ").unwrap();
        assert_eq!(load(&path), Settings::default());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(corrupt_path(&path)).unwrap(),
            "{\"theme\": "
        );
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let path = temp_path("missing");
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = temp_path("roundtrip");
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            theme: Some(Theme::Light),
            detail_width: Some(600),
            contexts: Some(BTreeMap::from([(
                "kind".to_string(),
                ContextSettings {
                    view: "apps/v1/deployments".into(),
                    namespaces: vec!["demo".into()],
                    typed_namespaces: vec!["team-a".into()],
                },
            )])),
            ..Default::default()
        });
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        assert!(!path.with_file_name("settings.json.tmp").exists());
    }

    #[test]
    fn patches_merge_map_entries() {
        let mut s = Settings::default();
        let kind = |key: &str| KindSettings {
            sort_key: key.into(),
            ..Default::default()
        };
        s.apply(SettingsPatch {
            kinds: Some(BTreeMap::from([
                ("a".into(), kind("name")),
                ("b".into(), kind("age")),
            ])),
            ..Default::default()
        });
        s.apply(SettingsPatch {
            kinds: Some(BTreeMap::from([("a".into(), kind("status"))])),
            last_context: Some("prod".into()),
            ..Default::default()
        });
        assert_eq!(s.kinds["a"].sort_key, "status");
        assert_eq!(s.kinds["b"].sort_key, "age");
        assert_eq!(s.last_context, "prod");
        assert_eq!(s.theme, Theme::System);
    }

    #[test]
    fn the_patch_accepts_partial_json() {
        let p: SettingsPatch = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(p.theme, Some(Theme::Dark));
        assert!(p.kinds.is_none());
    }

    #[test]
    fn flush_writes_only_when_changed() {
        let path = temp_path("flush");
        let store = SettingsStore::open(Some(path.clone()));
        store.flush();
        assert!(!path.exists());
        store.current.lock().unwrap().theme = Theme::Dark;
        store.dirty.store(true, Ordering::SeqCst);
        store.flush();
        assert_eq!(load(&path).theme, Theme::Dark);
    }
}
