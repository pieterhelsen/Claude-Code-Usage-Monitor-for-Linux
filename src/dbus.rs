//! Session-bus service consumed by the GNOME Shell extension, and a small
//! client used by the CLI modes.
//!
//! ```text
//! name       io.github.pieterhelsen.ClaudeCodeUsageMonitor
//! path       /io/github/pieterhelsen/ClaudeCodeUsageMonitor
//! interface  io.github.pieterhelsen.ClaudeCodeUsageMonitor1
//!   GetUsage() -> s          snapshot JSON (see snapshot.rs)
//!   Refresh()                poll every enabled provider now
//!   GetSettings() -> s       settings JSON
//!   SetSettings(s) -> s      replace settings, returns the normalised JSON
//!   Quit()
//!   property Version s
//!   signal UsageChanged(s)   snapshot JSON, after every change
//! ```

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;

use crate::daemon::{Command, State};

pub const NAME: &str = "io.github.pieterhelsen.ClaudeCodeUsageMonitor";
pub const PATH: &str = "/io/github/pieterhelsen/ClaudeCodeUsageMonitor";
pub const INTERFACE: &str = "io.github.pieterhelsen.ClaudeCodeUsageMonitor1";

pub struct Monitor {
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
}

#[zbus::interface(name = "io.github.pieterhelsen.ClaudeCodeUsageMonitor1")]
impl Monitor {
    fn get_usage(&self) -> String {
        self.state
            .lock()
            .map(|state| state.snapshot_json())
            .unwrap_or_default()
    }

    fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh { force: true });
    }

    fn get_settings(&self) -> zbus::fdo::Result<String> {
        let state = self
            .state
            .lock()
            .map_err(|_| zbus::fdo::Error::Failed("daemon state unavailable".into()))?;
        serde_json::to_string(&state.settings).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    fn set_settings(&self, json: &str) -> zbus::fdo::Result<String> {
        let mut settings = crate::app_settings::decode_settings(json)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("settings JSON did not parse".into()))?;
        settings.normalize();
        crate::app_settings::save_settings(&settings).map_err(zbus::fdo::Error::Failed)?;
        let repoll = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| zbus::fdo::Error::Failed("daemon state unavailable".into()))?;
            let repoll = state.settings.enabled_providers() != settings.enabled_providers()
                || state.settings.accounts != settings.accounts
                || state.settings.poll_interval_ms != settings.poll_interval_ms;
            state.settings = settings.clone();
            repoll
        };
        let _ = self.commands.send(Command::SettingsChanged { repoll });
        serde_json::to_string(&settings).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    fn quit(&self) {
        let _ = self.commands.send(Command::Quit);
    }

    #[zbus(property)]
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    #[zbus(signal)]
    async fn usage_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;
}

/// Export the monitor object and claim the bus name. Fails with
/// `zbus::Error::NameTaken` when another daemon already owns it, instead of
/// silently waiting in the name queue.
pub fn serve(
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
) -> zbus::Result<zbus::blocking::Connection> {
    serve_on(
        zbus::blocking::connection::Builder::session()?,
        state,
        commands,
    )
}

pub fn serve_on(
    builder: zbus::blocking::connection::Builder<'_>,
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
) -> zbus::Result<zbus::blocking::Connection> {
    let connection = builder
        .serve_at(PATH, Monitor { state, commands })?
        .build()?;
    let reply =
        connection.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())?;
    match reply {
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => {
            Ok(connection)
        }
        _ => Err(zbus::Error::NameTaken),
    }
}

pub fn emit_usage_changed(connection: &zbus::blocking::Connection, json: &str) -> zbus::Result<()> {
    let emitter = SignalEmitter::new(connection.inner(), PATH)?;
    zbus::block_on(Monitor::usage_changed(&emitter, json))
}

/// Call a no-argument method on the daemon, D-Bus-activating it if a service
/// file is installed.
pub fn call(method: &str) -> zbus::Result<Option<String>> {
    let connection = zbus::blocking::Connection::session()?;
    call_on(&connection, method)
}

pub fn call_on(
    connection: &zbus::blocking::Connection,
    method: &str,
) -> zbus::Result<Option<String>> {
    let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(connection)
        .destination(NAME)?
        .path(PATH)?
        .interface(INTERFACE)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()?;
    let reply = proxy.call_method(method, &())?;
    let body = reply.body();
    if body.signature().to_string() == "s" {
        Ok(Some(body.deserialize::<String>()?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Runtime;

    /// Runs against the real session bus (CI wraps tests in `dbus-run-session`).
    /// Skips when no bus is available or a real daemon already owns the name.
    #[test]
    fn get_usage_and_set_settings_round_trip_over_the_session_bus() {
        let Ok(builder) = zbus::blocking::connection::Builder::session() else {
            eprintln!("skipping: no session bus");
            return;
        };
        let state = Arc::new(Mutex::new(State {
            settings: Default::default(),
            usage: Default::default(),
            runtime: Runtime {
                fetched_at: 42,
                ..Default::default()
            },
        }));
        let (sender, receiver) = std::sync::mpsc::channel();
        let _server = match serve_on(builder, state.clone(), sender) {
            Ok(connection) => connection,
            Err(zbus::Error::NameTaken) => {
                eprintln!("skipping: a daemon already owns {NAME}");
                return;
            }
            Err(error) => panic!("cannot serve: {error}"),
        };

        let client = zbus::blocking::Connection::session().unwrap();
        let json = call_on(&client, "GetUsage").unwrap().unwrap();
        let snapshot: crate::snapshot::Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot.fetched_at, 42);
        assert_eq!(snapshot.providers[0].id, "claude");

        call_on(&client, "Refresh").unwrap();
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(Command::Refresh { force: true })
        ));

        let proxy = zbus::blocking::Proxy::new(&client, NAME, PATH, INTERFACE).unwrap();
        let normalized: String = proxy
            .call(
                "SetSettings",
                &(r#"{"show_codex":true,"poll_interval_ms":1}"#,),
            )
            .unwrap();
        let settings = crate::app_settings::decode_settings(&normalized).unwrap();
        assert!(settings.provider_enabled(crate::providers::ProviderId::Codex));
        assert_eq!(settings.poll_interval_ms, crate::app_settings::POLL_15_MIN);
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(Command::SettingsChanged { repoll: true })
        ));
        assert!(proxy
            .call::<_, _, String>("SetSettings", &("not json",))
            .is_err());
    }
}
