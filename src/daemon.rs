//! The long-running poller behind the D-Bus service.
//!
//! One thread owns polling. It sleeps until the next deadline (the refresh
//! interval, a window reset, or a credential-file check) and wakes early for
//! commands arriving from D-Bus.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::app_settings::{self, now_unix, SettingsFile};
use crate::diagnose;
use crate::models::AppUsageData;
use crate::poller::{self, CredentialWatchMode, CredentialWatchSnapshot};
use crate::providers::ProviderId;
use crate::snapshot::{Runtime, Snapshot};

/// How often credential files are checked for a fresh login.
const CREDENTIAL_WATCH_INTERVAL: Duration = Duration::from_secs(30);

pub enum Command {
    Refresh { force: bool },
    SettingsChanged { repoll: bool },
    Quit,
}

pub struct State {
    pub settings: SettingsFile,
    pub usage: AppUsageData,
    pub runtime: Runtime,
}

impl State {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::build(&self.settings, &self.usage, self.runtime)
    }

    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.snapshot()).unwrap_or_default()
    }
}

pub fn run() -> i32 {
    let settings = app_settings::load_settings();
    let cache = app_settings::load_usage_cache();
    let state = Arc::new(Mutex::new(State {
        settings,
        runtime: Runtime {
            fetched_at: cache.as_ref().map_or(0, |cache| cache.updated_unix),
            poll_ok: cache.as_ref().is_some_and(|cache| cache.poll_ok),
            ..Default::default()
        },
        usage: cache.map(|cache| cache.data).unwrap_or_default(),
    }));
    let (sender, receiver) = mpsc::channel();

    let connection = match crate::dbus::serve(state.clone(), sender) {
        Ok(connection) => connection,
        Err(zbus::Error::NameTaken) => {
            eprintln!("claude-code-usage-monitor: the daemon is already running");
            return 0;
        }
        Err(error) => {
            eprintln!("claude-code-usage-monitor: cannot register on the session bus: {error}");
            return 1;
        }
    };
    diagnose::log(format!(
        "daemon {} serving {}",
        env!("CARGO_PKG_VERSION"),
        crate::dbus::NAME
    ));

    let publish = {
        let connection = connection.clone();
        move |state: &State| {
            if let Err(error) = crate::dbus::emit_usage_changed(&connection, &state.snapshot_json())
            {
                diagnose::log_error("unable to emit UsageChanged", error);
            }
        }
    };
    poll_loop(&state, &receiver, publish);
    diagnose::log("daemon exiting");
    // Give the executor a moment to deliver the reply to a pending Quit call.
    std::thread::sleep(Duration::from_millis(200));
    drop(connection);
    0
}

fn poll_loop(state: &Mutex<State>, commands: &Receiver<Command>, publish: impl Fn(&State)) {
    let mut watched = credential_snapshots(&lock(state).settings);
    let mut next_poll = SystemTime::now();
    loop {
        let now = SystemTime::now();
        let wake = [
            next_poll,
            next_reset(&lock(state).usage).unwrap_or(next_poll),
            now + CREDENTIAL_WATCH_INTERVAL,
        ]
        .into_iter()
        .min()
        .unwrap_or(now);
        let force = match commands.recv_timeout(wake.duration_since(now).unwrap_or_default()) {
            Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Command::Refresh { force }) => force,
            Ok(Command::SettingsChanged { repoll }) => {
                let state = lock(state);
                if !repoll {
                    publish(&state);
                    continue;
                }
                watched = credential_snapshots(&state.settings);
                false
            }
            Err(RecvTimeoutError::Timeout) => {
                let due =
                    SystemTime::now() >= next_poll || poller::app_is_past_reset(&lock(state).usage);
                let current = credential_snapshots(&lock(state).settings);
                let login_changed = current != watched;
                watched = current;
                if !due && !login_changed {
                    continue;
                }
                if login_changed {
                    diagnose::log("credential files changed; polling");
                }
                false
            }
        };
        poll_now(state, force, &publish);
        let interval = Duration::from_millis(u64::from(lock(state).settings.poll_interval_ms));
        next_poll = SystemTime::now() + interval;
        {
            let mut state = lock(state);
            state.runtime.next_poll_at = unix(next_poll);
            publish(&state);
        }
    }
}

/// Poll every enabled provider, publishing partial results as they arrive.
fn poll_now(state: &Mutex<State>, force: bool, publish: &impl Fn(&State)) {
    let (settings, previous) = {
        let mut state = lock(state);
        state.runtime.polling = true;
        publish(&state);
        (state.settings.clone(), state.usage.clone())
    };
    let (usage, ok) = poll_once(&settings, &previous, force, |partial| {
        let mut state = lock(state);
        state.usage = partial.clone();
        publish(&state);
    });
    if let Err(error) = app_settings::save_usage_cache(&usage, ok) {
        diagnose::log_error("unable to save the usage cache", error);
    }
    let mut state = lock(state);
    state.usage = usage;
    state.runtime.polling = false;
    state.runtime.poll_ok = ok;
    state.runtime.fetched_at = now_unix();
}

/// One complete poll cycle, independent of D-Bus so `--json` can use it too.
/// Failed providers keep their previous reading, marked stale.
pub fn poll_once(
    settings: &SettingsFile,
    previous: &AppUsageData,
    force: bool,
    mut on_progress: impl FnMut(&AppUsageData),
) -> (AppUsageData, bool) {
    let enabled = settings.enabled_providers();
    let mut working = previous.clone();
    let result = poller::poll(
        enabled,
        &settings.accounts,
        Some(previous),
        force,
        |update| {
            working = poller::merge_poll_progress(update, &working, &settings.accounts);
            on_progress(&working);
        },
    );
    let (mut usage, ok) = match result {
        Ok(fresh) => (
            poller::carry_forward_failures(fresh, previous, enabled),
            true,
        ),
        Err(failure) => {
            diagnose::log(format!(
                "poll failed: {} {:?}",
                failure.provider.descriptor().display_name,
                failure.error
            ));
            working
                .errors
                .entry(failure.provider)
                .or_insert(failure.error);
            (working, false)
        }
    };
    usage.select_accounts(&settings.accounts);
    // Errors only explain providers that have no current reading.
    let readings: Vec<(ProviderId, bool)> = ProviderId::ALL
        .into_iter()
        .map(|provider| (provider, usage.get(provider).is_some_and(|u| !u.stale)))
        .collect();
    usage.errors.retain(|provider, _| {
        enabled.contains(*provider)
            && !readings
                .iter()
                .any(|(p, current)| p == provider && *current)
    });
    (usage, ok)
}

fn credential_snapshots(settings: &SettingsFile) -> Vec<CredentialWatchSnapshot> {
    settings
        .enabled_providers()
        .iter()
        .map(|provider| {
            poller::credential_watch_snapshot(CredentialWatchMode::AllSources(provider))
        })
        .collect()
}

/// The earliest future reset of any window, plus a second of slack so the
/// provider has rolled the window over by the time we ask.
fn next_reset(usage: &AppUsageData) -> Option<SystemTime> {
    let now = SystemTime::now();
    usage
        .all_usage()
        .filter(|usage| !usage.stale)
        .flat_map(|usage| usage.sections())
        .filter_map(|section| section.resets_at)
        .filter(|reset| *reset > now)
        .min()
        .map(|reset| reset + Duration::from_secs(1))
}

fn unix(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    // A panicking poll thread must not take the D-Bus interface down with it.
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};
    use std::time::UNIX_EPOCH;

    #[test]
    fn next_reset_skips_past_and_stale_windows() {
        let soon = SystemTime::now() + Duration::from_secs(600);
        let later = SystemTime::now() + Duration::from_secs(6_000);
        let usage = |reset, stale| UsageData {
            session: UsageSection {
                available: true,
                percentage: 1.0,
                resets_at: Some(reset),
            },
            stale,
            ..Default::default()
        };
        let data = AppUsageData::from_iter([
            (ProviderId::Claude, usage(UNIX_EPOCH, false)),
            (ProviderId::Codex, usage(soon, true)),
            (ProviderId::Grok, usage(later, false)),
        ]);
        assert_eq!(next_reset(&data), Some(later + Duration::from_secs(1)));
        assert_eq!(next_reset(&AppUsageData::default()), None);
    }
}
