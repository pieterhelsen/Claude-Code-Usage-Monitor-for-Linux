//! The usage snapshot published over D-Bus and printed by `--json`.
//!
//! This is the contract between the daemon and every front end (the GNOME
//! Shell extension, waybar, scripts). Bump [`SCHEMA_VERSION`] on any
//! incompatible change; adding optional fields is compatible.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app_settings::SettingsFile;
use crate::models::{AccountUsage, AppUsageData, CreditsSection, UsageData, UsageSection};
use crate::poller::PollError;
use crate::providers::ProviderId;

pub const SCHEMA_VERSION: u32 = 1;
pub const WARN_THRESHOLD: f64 = 70.0;
pub const CRITICAL_THRESHOLD: f64 = 90.0;

/// Daemon bookkeeping that is not part of the usage data itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Runtime {
    pub fetched_at: u64,
    pub next_poll_at: u64,
    pub polling: bool,
    pub poll_ok: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub daemon_version: String,
    /// Unix seconds of the last completed poll; 0 before the first one.
    pub fetched_at: u64,
    /// Unix seconds of the next scheduled poll; 0 when unknown.
    pub next_poll_at: u64,
    pub polling: bool,
    pub poll_ok: bool,
    pub poll_interval_ms: u32,
    /// The user prefers to see what is left rather than what is used.
    pub usage_countdown: bool,
    pub providers: Vec<ProviderSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Stale,
    Error,
    NoData,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorSnapshot {
    pub code: String,
    pub message: String,
    pub http_status: Option<u16>,
    pub is_auth: bool,
}

impl From<PollError> for ErrorSnapshot {
    fn from(error: PollError) -> Self {
        Self {
            code: error.code().into(),
            message: error.message(),
            http_status: error.http_status(),
            is_auth: error.is_auth(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    FiveHour,
    Weekly,
    Monthly,
    Credits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Headline {
    pub used_percent: f64,
    pub remaining_percent: f64,
    /// The window the figure came from.
    pub source: WindowKind,
    pub resets_at_unix: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub kind: WindowKind,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at_unix: Option<u64>,
    /// Currency amounts, for the credits window only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_amount: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub model: Option<String>,
    pub is_active: bool,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at_unix: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub stale: bool,
    pub headline: Option<Headline>,
    pub windows: Vec<Window>,
    pub limits: Vec<Limit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub id: String,
    pub name: String,
    pub selected: bool,
    pub status: Status,
    pub error: Option<ErrorSnapshot>,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub enabled: bool,
    pub status: Status,
    pub error: Option<ErrorSnapshot>,
    /// Usage of the selected account (or the provider's only source).
    pub usage: Option<Usage>,
    pub selected_account: Option<String>,
    /// Named accounts, for providers that support several logins.
    pub accounts: Vec<AccountSnapshot>,
}

impl Snapshot {
    pub fn build(settings: &SettingsFile, data: &AppUsageData, runtime: Runtime) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            fetched_at: runtime.fetched_at,
            next_poll_at: runtime.next_poll_at,
            polling: runtime.polling,
            poll_ok: runtime.poll_ok,
            poll_interval_ms: settings.poll_interval_ms,
            usage_countdown: settings.usage_countdown,
            providers: ProviderId::ALL
                .into_iter()
                .map(|provider| provider_snapshot(provider, settings, data))
                .collect(),
        }
    }

    /// The provider a single-figure front end should show: the first enabled
    /// provider with usage, in registry order.
    pub fn primary(&self) -> Option<&ProviderSnapshot> {
        self.providers
            .iter()
            .find(|provider| provider.enabled && provider.usage.is_some())
            .or_else(|| self.providers.iter().find(|provider| provider.enabled))
    }
}

fn provider_snapshot(
    provider: ProviderId,
    settings: &SettingsFile,
    data: &AppUsageData,
) -> ProviderSnapshot {
    let descriptor = provider.descriptor();
    let enabled = settings.provider_enabled(provider);
    let mut snapshot = ProviderSnapshot {
        id: descriptor.key.into(),
        display_name: descriptor.display_name.into(),
        description: descriptor.settings_description.into(),
        enabled,
        status: Status::Disabled,
        error: None,
        usage: None,
        selected_account: None,
        accounts: Vec::new(),
    };
    if !enabled {
        return snapshot;
    }

    let accounts: Vec<&AccountUsage> = data
        .accounts
        .iter()
        .filter(|account| account.provider == provider)
        .collect();
    snapshot.accounts = accounts
        .iter()
        .map(|account| AccountSnapshot {
            id: account.profile.id.clone(),
            name: account.profile.name.clone(),
            selected: account.selected,
            status: status(account.usage.as_ref(), account.error),
            error: account.error.map(ErrorSnapshot::from),
            usage: account.usage.as_ref().map(usage),
        })
        .collect();
    let selected = accounts.iter().find(|account| account.selected);
    snapshot.selected_account = selected.map(|account| account.profile.id.clone());

    let error = match selected {
        Some(account) => account.error,
        None => data.errors.get(&provider).copied(),
    };
    let reading = data.get(provider);
    snapshot.status = status(reading, error);
    snapshot.error = error.map(ErrorSnapshot::from);
    snapshot.usage = reading.map(usage);
    snapshot
}

fn status(usage: Option<&UsageData>, error: Option<PollError>) -> Status {
    match (usage, error) {
        (Some(usage), _) if usage.stale => Status::Stale,
        (Some(_), _) => Status::Ok,
        (None, Some(_)) => Status::Error,
        (None, None) => Status::NoData,
    }
}

pub fn usage(data: &UsageData) -> Usage {
    let mut windows = Vec::new();
    if data.session.available {
        windows.push(window(WindowKind::FiveHour, "5h", &data.session));
    }
    if data.weekly.available {
        let label = data.weekly_label.as_deref().unwrap_or("7d");
        windows.push(window(WindowKind::Weekly, label, &data.weekly));
    }
    if let Some(monthly) = data.monthly.as_ref().filter(|monthly| monthly.available) {
        windows.push(window(WindowKind::Monthly, "30d", monthly));
    }
    if let Some(credits) = &data.credits {
        windows.push(credits_window(credits));
    }
    Usage {
        stale: data.stale,
        headline: headline(data),
        windows,
        limits: data
            .limits
            .iter()
            .filter(|limit| limit.usage.available)
            .map(|limit| Limit {
                key: limit.key.clone(),
                kind: limit.kind.clone(),
                label: limit.label.clone(),
                model: limit.model.clone(),
                is_active: limit.is_active,
                used_percent: clamp_percent(limit.usage.percentage),
                remaining_percent: 100.0 - clamp_percent(limit.usage.percentage),
                resets_at_unix: unix(limit.usage.resets_at),
            })
            .collect(),
    }
}

fn window(kind: WindowKind, label: &str, section: &UsageSection) -> Window {
    let used = clamp_percent(section.percentage);
    Window {
        kind,
        label: label.to_string(),
        used_percent: used,
        remaining_percent: 100.0 - used,
        resets_at_unix: unix(section.resets_at),
        remaining_amount: None,
        total_amount: None,
    }
}

fn credits_window(credits: &CreditsSection) -> Window {
    let used = clamp_percent(credits.percentage);
    Window {
        kind: WindowKind::Credits,
        label: "Credits".into(),
        used_percent: used,
        remaining_percent: 100.0 - used,
        resets_at_unix: None,
        remaining_amount: Some(credits.remaining),
        total_amount: Some(credits.total),
    }
}

/// The single figure a badge should show: whatever is closest to its limit.
/// Credits, once in play, override the ordinary windows (same rule as the
/// Windows edition's `headline` theme binding).
fn headline(data: &UsageData) -> Option<Headline> {
    if let Some(credits) = &data.credits {
        let used = clamp_percent(credits.percentage);
        return Some(Headline {
            used_percent: used,
            remaining_percent: 100.0 - used,
            source: WindowKind::Credits,
            resets_at_unix: None,
        });
    }
    [
        (WindowKind::FiveHour, Some(&data.session)),
        (WindowKind::Weekly, Some(&data.weekly)),
        (WindowKind::Monthly, data.monthly.as_ref()),
    ]
    .into_iter()
    .filter_map(|(kind, section)| section.filter(|s| s.available).map(|s| (kind, s)))
    // Ties go to the shorter window, which resets sooner.
    .fold(
        None,
        |best: Option<(WindowKind, &UsageSection)>, (kind, section)| match best {
            Some((_, current)) if current.percentage >= section.percentage => best,
            _ => Some((kind, section)),
        },
    )
    .map(|(kind, section)| {
        let used = clamp_percent(section.percentage);
        Headline {
            used_percent: used,
            remaining_percent: 100.0 - used,
            source: kind,
            resets_at_unix: unix(section.resets_at),
        }
    })
}

fn clamp_percent(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn unix(time: Option<SystemTime>) -> Option<u64> {
    time?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Compact countdown such as `45m`, `3h` or `2d`, matching the panel label.
pub fn format_countdown(seconds: u64) -> String {
    match seconds {
        0..=59 => "<1m".into(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Longer countdown for tooltips and menus, such as `3h 12m`.
pub fn format_countdown_long(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    match (days, hours) {
        (0, 0) => format!("{}m", minutes.max(if seconds > 0 { 1 } else { 0 })),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: &'static str,
    pub percentage: u8,
}

/// Output for a waybar `custom` module with `"return-type": "json"`.
pub fn waybar(snapshot: &Snapshot, now: u64) -> WaybarOutput {
    let shown = |used: f64| {
        if snapshot.usage_countdown {
            100.0 - used
        } else {
            used
        }
    };
    let Some(provider) = snapshot.primary() else {
        return WaybarOutput {
            text: "—".into(),
            tooltip: "No provider enabled".into(),
            class: "error",
            percentage: 0,
        };
    };
    let headline = provider
        .usage
        .as_ref()
        .and_then(|usage| usage.headline.as_ref());
    let text = match headline {
        Some(headline) => {
            let mut text = format!("{:.0}%", shown(headline.used_percent));
            if let Some(reset) = headline.resets_at_unix.filter(|reset| *reset > now) {
                text.push_str(" · ");
                text.push_str(&format_countdown(reset - now));
            }
            text
        }
        None if provider.error.is_some() => "!".into(),
        None => "…".into(),
    };
    let class = match (headline, provider.status) {
        (_, Status::Error) => "error",
        (Some(headline), _) if headline.used_percent >= CRITICAL_THRESHOLD => "critical",
        (Some(headline), _) if headline.used_percent >= WARN_THRESHOLD => "warn",
        (_, Status::Stale) => "stale",
        _ => "ok",
    };
    WaybarOutput {
        text,
        tooltip: tooltip(snapshot, now),
        class,
        // Matches the text: remaining when counting down, used otherwise.
        percentage: headline
            .map(|headline| shown(headline.used_percent).round() as u8)
            .unwrap_or(0),
    }
}

/// Multi-line plain-text summary of every enabled provider.
pub fn tooltip(snapshot: &Snapshot, now: u64) -> String {
    let verb = if snapshot.usage_countdown {
        "left"
    } else {
        "used"
    };
    let mut lines = Vec::new();
    for provider in snapshot.providers.iter().filter(|p| p.enabled) {
        let mut title = provider.display_name.clone();
        if let Some(account) = provider
            .accounts
            .iter()
            .find(|account| account.selected && provider.accounts.len() > 1)
        {
            title.push_str(&format!(" ({})", account.name));
        }
        if provider.status == Status::Stale {
            title.push_str(" [stale]");
        }
        lines.push(title);
        match &provider.usage {
            Some(usage) if !usage.windows.is_empty() => {
                for window in &usage.windows {
                    let figure = if snapshot.usage_countdown {
                        window.remaining_percent
                    } else {
                        window.used_percent
                    };
                    let mut line = format!("  {:<7} {figure:>3.0}% {verb}", window.label);
                    if let Some(reset) = window.resets_at_unix.filter(|reset| *reset > now) {
                        line.push_str(&format!(
                            ", resets in {}",
                            format_countdown_long(reset - now)
                        ));
                    }
                    if let (Some(left), Some(total)) =
                        (window.remaining_amount, window.total_amount)
                    {
                        line.push_str(&format!(" ({left:.2} of {total:.2} left)"));
                    }
                    lines.push(line);
                }
            }
            Some(_) => lines.push("  No usage windows reported".into()),
            None => {}
        }
        if let Some(error) = &provider.error {
            lines.push(format!("  {}", error.message));
        } else if provider.usage.is_none() {
            lines.push("  Waiting for first poll".into());
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn section(percentage: f64, reset: Option<u64>) -> UsageSection {
        UsageSection {
            available: true,
            percentage,
            resets_at: reset.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    fn claude_usage() -> UsageData {
        UsageData {
            session: section(42.0, Some(1_000_000 + 3 * 3_600 + 60)),
            weekly: section(12.0, Some(1_000_000 + 4 * 86_400)),
            ..Default::default()
        }
    }

    fn settings_with(providers: &[ProviderId]) -> SettingsFile {
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(crate::providers::ProviderSet::from_enabled(
            providers.iter().copied(),
        ));
        settings
    }

    #[test]
    fn headline_takes_the_fullest_window_unless_credits_are_in_play() {
        let usage = claude_usage();
        let top = headline(&usage).unwrap();
        assert_eq!(top.source, WindowKind::FiveHour);
        assert_eq!(top.used_percent, 42.0);

        let weekly_heavy = UsageData {
            session: section(10.0, None),
            weekly: section(80.0, Some(5)),
            ..Default::default()
        };
        assert_eq!(headline(&weekly_heavy).unwrap().source, WindowKind::Weekly);
        assert_eq!(headline(&weekly_heavy).unwrap().resets_at_unix, Some(5));

        let with_credits = UsageData {
            credits: Some(CreditsSection {
                percentage: 27.0,
                remaining: 36.5,
                total: 50.0,
            }),
            ..claude_usage()
        };
        let credits = headline(&with_credits).unwrap();
        assert_eq!(credits.source, WindowKind::Credits);
        assert_eq!(credits.used_percent, 27.0);
    }

    #[test]
    fn unavailable_windows_are_omitted_and_labels_follow_the_provider() {
        let usage = usage(&UsageData {
            session: UsageSection::default(),
            weekly: section(5.0, None),
            weekly_label: Some("API".into()),
            monthly: Some(section(60.0, None)),
            ..Default::default()
        });
        let kinds: Vec<_> = usage
            .windows
            .iter()
            .map(|w| (w.kind, w.label.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![(WindowKind::Weekly, "API"), (WindowKind::Monthly, "30d")]
        );
        assert_eq!(usage.headline.unwrap().source, WindowKind::Monthly);
        assert!(super::usage(&UsageData::default()).headline.is_none());
    }

    #[test]
    fn providers_report_disabled_pending_error_and_stale_states() {
        let settings = settings_with(&[ProviderId::Claude, ProviderId::Cursor, ProviderId::Grok]);
        let mut data = AppUsageData::from_iter([(
            ProviderId::Grok,
            UsageData {
                stale: true,
                ..claude_usage()
            },
        )]);
        data.errors
            .insert(ProviderId::Cursor, PollError::NoCredentials);
        let snapshot = Snapshot::build(&settings, &data, Runtime::default());
        let by_id = |id: &str| snapshot.providers.iter().find(|p| p.id == id).unwrap();
        assert_eq!(snapshot.providers.len(), ProviderId::ALL.len());
        assert_eq!(by_id("claude").status, Status::NoData);
        assert_eq!(by_id("codex").status, Status::Disabled);
        assert_eq!(by_id("cursor").status, Status::Error);
        assert_eq!(
            by_id("cursor").error.as_ref().unwrap().code,
            "no_credentials"
        );
        assert_eq!(by_id("grok").status, Status::Stale);
    }

    #[test]
    fn selected_account_drives_provider_status() {
        let settings = settings_with(&[ProviderId::Claude]);
        let mut data = AppUsageData::default();
        data.accounts.push(AccountUsage {
            provider: ProviderId::Claude,
            profile: crate::accounts::AccountProfile::default(),
            source_signature: String::new(),
            source_path: None,
            usage: None,
            error: Some(PollError::HttpStatus(401)),
            selected: true,
        });
        let snapshot = Snapshot::build(&settings, &data, Runtime::default());
        let claude = &snapshot.providers[0];
        assert_eq!(claude.status, Status::Error);
        assert_eq!(claude.selected_account.as_deref(), Some("default"));
        let error = claude.error.as_ref().unwrap();
        assert_eq!(
            (error.code.as_str(), error.http_status, error.is_auth),
            ("http_status", Some(401), true)
        );
        assert_eq!(claude.accounts.len(), 1);
    }

    #[test]
    fn json_shape_is_stable() {
        let settings = settings_with(&[ProviderId::Claude]);
        let data = AppUsageData::from_iter([(ProviderId::Claude, claude_usage())]);
        let runtime = Runtime {
            fetched_at: 1_000_000,
            next_poll_at: 1_000_900,
            polling: false,
            poll_ok: true,
        };
        let json = serde_json::to_value(Snapshot::build(&settings, &data, runtime)).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["poll_interval_ms"], 900_000);
        let claude = &json["providers"][0];
        assert_eq!(claude["id"], "claude");
        assert_eq!(claude["status"], "ok");
        assert_eq!(claude["usage"]["headline"]["source"], "five_hour");
        assert_eq!(claude["usage"]["windows"][0]["kind"], "five_hour");
        assert_eq!(claude["usage"]["windows"][0]["label"], "5h");
        assert_eq!(claude["usage"]["windows"][1]["remaining_percent"], 88.0);
        assert!(claude["usage"]["windows"][0].get("total_amount").is_none());
        assert_eq!(json["providers"][1]["status"], "disabled");
    }

    #[test]
    fn snapshots_round_trip_through_json() {
        let settings = settings_with(&[ProviderId::Claude]);
        let data = AppUsageData::from_iter([(ProviderId::Claude, claude_usage())]);
        let snapshot = Snapshot::build(&settings, &data, Runtime::default());
        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), snapshot);
    }

    #[test]
    fn countdowns_use_the_largest_whole_unit() {
        assert_eq!(format_countdown(30), "<1m");
        assert_eq!(format_countdown(45 * 60), "45m");
        assert_eq!(format_countdown(3 * 3_600 + 600), "3h");
        assert_eq!(format_countdown(2 * 86_400 + 5), "2d");
        assert_eq!(format_countdown_long(3 * 3_600 + 12 * 60), "3h 12m");
        assert_eq!(format_countdown_long(86_400 + 3_600 * 5), "1d 5h");
        assert_eq!(format_countdown_long(20), "1m");
    }

    #[test]
    fn waybar_shows_headline_countdown_and_threshold_class() {
        let settings = settings_with(&[ProviderId::Claude]);
        let data = AppUsageData::from_iter([(ProviderId::Claude, claude_usage())]);
        let snapshot = Snapshot::build(&settings, &data, Runtime::default());
        let output = waybar(&snapshot, 1_000_000);
        assert_eq!(output.text, "42% · 3h");
        assert_eq!(output.class, "ok");
        assert_eq!(output.percentage, 42);
        assert!(output
            .tooltip
            .starts_with("Claude Code\n  5h       42% used, resets in 3h 1m"));

        let mut countdown = settings.clone();
        countdown.usage_countdown = true;
        let snapshot = Snapshot::build(&countdown, &data, Runtime::default());
        let remaining = waybar(&snapshot, 1_000_000);
        assert_eq!(remaining.text, "58% · 3h");
        assert_eq!(remaining.percentage, 58);
        // Thresholds still follow usage spent.
        assert_eq!(remaining.class, "ok");

        let hot = AppUsageData::from_iter([(
            ProviderId::Claude,
            UsageData {
                weekly: section(95.0, None),
                ..claude_usage()
            },
        )]);
        let snapshot = Snapshot::build(&settings, &hot, Runtime::default());
        assert_eq!(waybar(&snapshot, 1_000_000).class, "critical");
    }
}
