//! Settings and caches, persisted atomically under the XDG base directories.
//!
//! * `~/.config/claude-code-usage-monitor/settings.json`
//! * `~/.cache/claude-code-usage-monitor/{usage-cache,codex-credits-*}.json`
//!
//! Set `CLAUDE_CODE_USAGE_MONITOR_CONFIG_DIR` to keep both under one directory
//! instead, which is handy for trying the daemon without touching real state.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::models::{AppUsageData, CodexCreditsState};
use crate::providers::{ProviderId, ProviderSet};

#[cfg_attr(test, allow(dead_code))]
pub const APP_DIRECTORY_NAME: &str = "claude-code-usage-monitor";
#[cfg_attr(test, allow(dead_code))]
pub const DIRECTORY_OVERRIDE_ENV: &str = "CLAUDE_CODE_USAGE_MONITOR_CONFIG_DIR";

pub const POLL_1_MIN: u32 = 60 * 1_000;
pub const POLL_15_MIN: u32 = 15 * POLL_1_MIN;
/// Upper bound for a sensible refresh interval: one week.
pub const MAX_POLL_MINUTES: u32 = 7 * 24 * 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub accounts: crate::accounts::AccountSettings,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u32,
    #[serde(default = "default_true")]
    show_claude_code: bool,
    #[serde(default)]
    show_codex: bool,
    #[serde(default)]
    show_opencode: bool,
    #[serde(default)]
    show_cursor: bool,
    #[serde(default)]
    show_grok: bool,
    /// Show what is left of each allowance instead of what has been spent, so
    /// the panel counts down towards a limit rather than up from zero.
    #[serde(default)]
    pub usage_countdown: bool,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            poll_interval_ms: default_poll_interval(),
            show_claude_code: true,
            show_codex: false,
            show_opencode: false,
            show_cursor: false,
            show_grok: false,
            usage_countdown: false,
        }
    }
}

impl SettingsFile {
    pub fn normalize(&mut self) {
        self.accounts.claude.normalize();
        self.accounts.codex.normalize();
        if !(POLL_1_MIN..=MAX_POLL_MINUTES * POLL_1_MIN).contains(&self.poll_interval_ms)
            || !self.poll_interval_ms.is_multiple_of(POLL_1_MIN)
        {
            self.poll_interval_ms = default_poll_interval();
        }
        if self.enabled_providers().is_empty() {
            self.set_enabled_providers(ProviderSet::default());
        }
    }

    pub fn enabled_providers(&self) -> ProviderSet {
        ProviderSet::from_enabled(
            ProviderId::ALL
                .into_iter()
                .filter(|provider| self.provider_enabled(*provider)),
        )
    }

    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::Claude => self.show_claude_code,
            ProviderId::Codex => self.show_codex,
            ProviderId::OpenCode => self.show_opencode,
            ProviderId::Cursor => self.show_cursor,
            ProviderId::Grok => self.show_grok,
        }
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        match provider {
            ProviderId::Claude => self.show_claude_code = enabled,
            ProviderId::Codex => self.show_codex = enabled,
            ProviderId::OpenCode => self.show_opencode = enabled,
            ProviderId::Cursor => self.show_cursor = enabled,
            ProviderId::Grok => self.show_grok = enabled,
        }
    }

    pub fn set_enabled_providers(&mut self, providers: ProviderSet) {
        for provider in ProviderId::ALL {
            self.set_provider_enabled(provider, providers.contains(provider));
        }
    }

    #[cfg(test)]
    pub fn toggle_provider(&mut self, provider: ProviderId) -> bool {
        let mut providers = self.enabled_providers();
        if !providers.toggle(provider) {
            return false;
        }
        self.set_enabled_providers(providers);
        true
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCache {
    pub updated_unix: u64,
    pub poll_ok: bool,
    pub data: AppUsageData,
}

#[cfg_attr(test, allow(dead_code))]
fn directory_override() -> Option<PathBuf> {
    std::env::var_os(DIRECTORY_OVERRIDE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(test))]
pub fn config_directory() -> PathBuf {
    directory_override()
        .or_else(|| dirs::config_dir().map(|dir| dir.join(APP_DIRECTORY_NAME)))
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIRECTORY_NAME))
}

#[cfg(not(test))]
pub fn cache_directory() -> PathBuf {
    directory_override()
        .map(|dir| dir.join("cache"))
        .or_else(|| dirs::cache_dir().map(|dir| dir.join(APP_DIRECTORY_NAME)))
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIRECTORY_NAME).join("cache"))
}

/// Test threads get independent settings and caches, so parallel tests never
/// touch each other's files or the real user directories.
#[cfg(test)]
pub fn config_directory() -> PathBuf {
    test_root().join("config")
}

#[cfg(test)]
pub fn cache_directory() -> PathBuf {
    test_root().join("cache")
}

#[cfg(test)]
fn test_root() -> PathBuf {
    thread_local! {
        static DIRECTORY: TestAppData = TestAppData::new();
    }
    DIRECTORY.with(|directory| directory.0.clone())
}

#[cfg(test)]
struct TestAppData(PathBuf);

#[cfg(test)]
impl TestAppData {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        loop {
            let path = std::env::temp_dir().join(format!(
                "ccum-test-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test settings directory: {error}"),
            }
        }
    }
}

#[cfg(test)]
impl Drop for TestAppData {
    fn drop(&mut self) {
        // Only remove the directory this thread successfully created.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn settings_path() -> PathBuf {
    config_directory().join("settings.json")
}

pub fn usage_cache_path() -> PathBuf {
    cache_directory().join("usage-cache.json")
}

pub fn load_settings() -> SettingsFile {
    let mut settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|content| decode_settings(&content))
        .unwrap_or_default();
    settings.normalize();
    settings
}

pub fn save_settings(settings: &SettingsFile) -> Result<(), String> {
    let mut normalized = settings.clone();
    normalized.normalize();
    write_json_atomic(&settings_path(), &normalized)
}

/// Unknown keys (including those left by the Windows edition) are ignored.
pub fn decode_settings(content: &str) -> Option<SettingsFile> {
    serde_json::from_str(content).ok()
}

pub fn codex_credits_path() -> PathBuf {
    cache_directory().join("codex-credits.json")
}

pub fn load_codex_credits() -> Option<CodexCreditsState> {
    read_json(&codex_credits_path())
}

pub fn save_codex_credits(state: &CodexCreditsState) -> Result<(), String> {
    write_json_atomic(&codex_credits_path(), state)
}

pub fn load_usage_cache() -> Option<UsageCache> {
    let mut cache: UsageCache = read_json(&usage_cache_path())?;
    cache.data.invalidate_changed_credentials();
    Some(cache)
}

pub fn save_usage_cache(data: &AppUsageData, poll_ok: bool) -> Result<(), String> {
    write_json_atomic(
        &usage_cache_path(),
        &UsageCache {
            updated_unix: now_unix(),
            poll_ok,
            data: data.clone(),
        },
    )
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write to a sibling temporary file, flush it, then rename it into place so
/// readers only ever see a complete file.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Unable to replace {}: {error}", path.display()));
    }
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn default_true() -> bool {
    true
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_files_stay_inside_the_test_directory() {
        let root = test_root();
        assert!(root.starts_with(std::env::temp_dir()));
        for path in [settings_path(), usage_cache_path(), codex_credits_path()] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
        save_settings(&SettingsFile::default()).unwrap();
        assert!(settings_path().is_file());
    }

    #[test]
    fn atomic_writes_leave_no_temporary_files() {
        save_settings(&SettingsFile::default()).unwrap();
        save_settings(&SettingsFile::default()).unwrap();
        let names: Vec<_> = std::fs::read_dir(config_directory())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["settings.json".to_string()]);
    }

    #[test]
    fn parallel_test_threads_have_independent_settings_and_clean_up() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = [7, 11]
            .into_iter()
            .map(|minutes| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let settings = SettingsFile {
                        poll_interval_ms: minutes * POLL_1_MIN,
                        ..Default::default()
                    };
                    save_settings(&settings).unwrap();
                    barrier.wait();
                    assert_eq!(load_settings().poll_interval_ms, minutes * POLL_1_MIN);
                    test_root()
                })
            })
            .collect();
        let paths: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_ne!(paths[0], paths[1]);
        assert!(paths.iter().all(|path| !path.exists()));
    }

    #[test]
    fn custom_poll_minutes_and_presets_survive_settings_round_trip() {
        for minutes in [1, 2, 5, 7, 15, 60, 120, 1_440, MAX_POLL_MINUTES] {
            let interval = minutes * POLL_1_MIN;
            let mut decoded =
                decode_settings(&format!(r#"{{"poll_interval_ms":{interval}}}"#)).unwrap();
            decoded.normalize();
            assert_eq!(decoded.poll_interval_ms, interval);
            let json = serde_json::to_string(&decoded).unwrap();
            let mut reloaded = decode_settings(&json).unwrap();
            reloaded.normalize();
            assert_eq!(reloaded.poll_interval_ms, interval);
        }
    }

    #[test]
    fn invalid_poll_intervals_fall_back_to_the_default() {
        for interval in [
            0,
            POLL_1_MIN - 1,
            POLL_1_MIN + 1,
            (MAX_POLL_MINUTES + 1) * POLL_1_MIN,
            u32::MAX,
        ] {
            let mut decoded =
                decode_settings(&format!(r#"{{"poll_interval_ms":{interval}}}"#)).unwrap();
            decoded.normalize();
            assert_eq!(decoded.poll_interval_ms, default_poll_interval());
        }
    }

    #[test]
    fn windows_edition_settings_keys_are_ignored() {
        let settings = decode_settings(
            r#"{
                "tray_offset": 144,
                "taskbar_index": 2,
                "widget_visible": false,
                "show_antigravity": true,
                "active_theme_path": "theme.json",
                "placement_override": {"nest": "floating"},
                "poll_interval_ms": 300000,
                "show_codex": true
            }"#,
        )
        .unwrap();
        assert_eq!(settings.poll_interval_ms, 5 * POLL_1_MIN);
        assert!(settings.provider_enabled(ProviderId::Codex));
        let json = serde_json::to_value(&settings).unwrap();
        assert!(json.get("tray_offset").is_none());
        assert!(json.get("show_antigravity").is_none());
    }

    #[test]
    fn named_accounts_round_trip_without_changing_provider_preferences() {
        let old = decode_settings(r#"{"show_claude_code":true,"show_codex":true}"#).unwrap();
        assert_eq!(old.accounts, crate::accounts::AccountSettings::default());
        let mut settings = old;
        settings.accounts.codex.add();
        settings.accounts.codex.profiles[1].config_dir = "/home/test/.codex-work".into();
        settings.accounts.codex.profiles[1].enabled = true;
        settings.accounts.codex.selected = "account_1".into();
        let decoded = decode_settings(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded.accounts, settings.accounts);
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn settings_never_disable_every_provider() {
        let mut settings = SettingsFile {
            show_claude_code: false,
            show_codex: false,
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn provider_selection_keeps_the_existing_settings_keys() {
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::from_enabled([
            ProviderId::Codex,
            ProviderId::OpenCode,
            ProviderId::Cursor,
            ProviderId::Grok,
        ]));

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["show_claude_code"], false);
        assert_eq!(json["show_codex"], true);
        assert_eq!(json["show_opencode"], true);
        assert_eq!(json["show_cursor"], true);
        assert_eq!(json["show_grok"], true);

        let decoded = decode_settings(&json.to_string()).unwrap();
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn provider_toggle_keeps_the_last_provider_enabled() {
        let mut settings = SettingsFile::default();
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn usage_direction_defaults_to_counting_up_and_round_trips() {
        let settings = SettingsFile::default();
        assert!(!settings.usage_countdown);
        let counting_down = decode_settings(r#"{"usage_countdown":true}"#).unwrap();
        assert!(counting_down.usage_countdown);
        assert_eq!(
            serde_json::to_value(&counting_down).unwrap()["usage_countdown"],
            true
        );
    }
}
