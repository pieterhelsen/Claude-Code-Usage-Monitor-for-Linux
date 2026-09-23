use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{
    build_agent, get_header_f64, get_header_i64, parse_iso8601, unix_to_system_time, HttpResponse,
    PollError,
};
use crate::diagnose;
use crate::models::{CreditsSection, UsageData};

mod limits;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
// Keep header probes on the low-cost Haiku tier. This API alias follows 4.5
// snapshots, but still needs updating when the Haiku 4.5 generation retires.
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-haiku-4-5"];

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
    spend: Option<SpendResponse>,
    limits: Option<Vec<serde_json::Value>>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Paid credits that carry the account past its plan limits. Amounts are
/// minor units with their own exponent, so the currency is self-describing.
#[derive(Deserialize)]
struct SpendResponse {
    #[serde(default)]
    enabled: bool,
    used: Option<SpendAmount>,
    limit: Option<SpendAmount>,
}

#[derive(Deserialize)]
struct SpendAmount {
    amount_minor: f64,
    #[serde(default)]
    exponent: u32,
}

impl SpendAmount {
    fn major(&self) -> f64 {
        self.amount_minor / 10f64.powi(self.exponent as i32)
    }
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

/// Where a token came from, so a refresh re-reads the same file instead of
/// switching to another account's login.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CredentialSource {
    File(PathBuf),
}

pub(super) fn poll_claude_code() -> Result<UsageData, PollError> {
    let creds = match read_first_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let creds = refresh_credentials(creds)?;

    fetch_usage_with_fallback(&creds.access_token)
}

/// Explicit profiles are pinned to one source, including when refresh fails.
pub(super) fn poll_account(path: &Path) -> Result<UsageData, PollError> {
    let mut credentials = read_credentials_from_source(&CredentialSource::File(path.to_path_buf()))
        .ok_or(PollError::NoCredentials)?;

    let source = credentials.source.clone();
    if is_token_expired(credentials.expires_at) {
        cli_refresh_token(&source);
        credentials = read_credentials_from_source(&source).ok_or(PollError::TokenExpired)?;
        if is_token_expired(credentials.expires_at) {
            return Err(PollError::TokenExpired);
        }
    }
    fetch_usage_with_fallback(&credentials.access_token)
}

pub(super) fn account_watch_signature(path: &Path) -> String {
    crate::accounts::file_signature(path)
}

pub(super) fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    if let Some(data) = try_usage_endpoint(token)? {
        // If reset timers are missing, fill them in from the Messages API
        if (data.session.available && data.session.resets_at.is_none())
            || (data.weekly.available && data.weekly.resets_at.is_none())
        {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                merged.session.available |= fallback.session.available;
                merged.weekly.available |= fallback.weekly.available;
                if merged.session.resets_at.is_none() {
                    merged.session.resets_at = fallback.session.resets_at;
                }
                if merged.weekly.resets_at.is_none() {
                    merged.weekly.resets_at = fallback.weekly.resets_at;
                }
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

pub(super) fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let mut resp = match agent
        .get(USAGE_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(error) => match classify_usage_failure(&error) {
            UsageEndpointFailure::Auth => {
                diagnose::log(format!(
                    "usage endpoint returned an auth error ({error}); re-login required"
                ));
                return Err(usage_request_error(&error));
            }
            UsageEndpointFailure::Transient => {
                diagnose::log(format!("usage endpoint temporarily unavailable ({error})"));
                return Err(usage_request_error(&error));
            }
            UsageEndpointFailure::Unsupported => {
                diagnose::log(format!(
                    "usage endpoint unavailable for this account ({error}); trying the Messages API"
                ));
                return Ok(None);
            }
        },
    };

    parse_usage_body(resp.body_mut()).map(Some)
}

fn parse_usage_body(body: &mut ureq::Body) -> Result<UsageData, PollError> {
    let response: UsageResponse = match body.read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unexpected Claude usage response", error);
            return Err(PollError::UnexpectedResponse);
        }
    };
    validated_usage_from_response(response)
}

fn validated_usage_from_response(response: UsageResponse) -> Result<UsageData, PollError> {
    let data = usage_from_response(response);
    if !data.sections().any(|section| section.available) {
        diagnose::log("unexpected Claude usage response: no usable usage limits");
        return Err(PollError::UnexpectedResponse);
    }
    Ok(data)
}

fn usage_from_response(response: UsageResponse) -> UsageData {
    let mut data = UsageData {
        limits: limits::parse(
            response.limits.as_deref().unwrap_or_default(),
            &response.extra,
        ),
        ..Default::default()
    };

    if let Some(bucket) = &response.five_hour {
        data.session.available = true;
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.available = true;
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    // New-format responses may omit the legacy fields. Scoped quotas never
    // replace the all-model session or weekly values used by built-in themes.
    for limit in &data.limits {
        if limit.scope.is_none() {
            match limit.kind.as_str() {
                "session" if !data.session.available => data.session = limit.usage.clone(),
                "weekly_all" if !data.weekly.available => data.weekly = limit.usage.clone(),
                _ => {}
            }
        }
    }

    data.credits = response
        .spend
        .as_ref()
        .and_then(|spend| claude_credits(spend, &data));

    data
}

/// What a failed call to the usage endpoint actually tells us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageEndpointFailure {
    /// The credentials were rejected.
    Auth,
    /// Rate limited, a server-side fault, or the network. Retrying later is
    /// the right move. Asking the Messages API instead would spend real quota
    /// on a request whose only purpose is to read headers, and during a rate
    /// limit it would add to the load that caused it.
    Transient,
    /// The endpoint is not usable on this account, which is what the Messages
    /// API fallback exists for.
    Unsupported,
}

fn classify_usage_failure(error: &ureq::Error) -> UsageEndpointFailure {
    match error {
        ureq::Error::StatusCode(401 | 403) => UsageEndpointFailure::Auth,
        ureq::Error::StatusCode(429) => UsageEndpointFailure::Transient,
        ureq::Error::StatusCode(code) if *code >= 500 => UsageEndpointFailure::Transient,
        ureq::Error::StatusCode(_) => UsageEndpointFailure::Unsupported,
        _ => UsageEndpointFailure::Transient,
    }
}

fn usage_request_error(error: &ureq::Error) -> PollError {
    match error {
        ureq::Error::StatusCode(status) => PollError::HttpStatus(*status),
        _ => PollError::NetworkError,
    }
}

/// Unlike Codex, the plan states its own ceiling, so the gauge needs no
/// history: `used` is already the spend against the current cap, and a
/// non-zero figure is the same "credits are in play" observation that the
/// Codex balance gives by falling. Accounts with extra usage switched off
/// report it disabled and get no gauge rather than an empty one.
fn claude_credits(spend: &SpendResponse, data: &UsageData) -> Option<CreditsSection> {
    let used = spend.used.as_ref()?.major();
    let total = spend.limit.as_ref()?.major();
    if !spend.enabled || !total.is_finite() || total <= 0.0 {
        return None;
    }

    // Hold the ordinary windows until one of them is spent and credits have
    // started covering the overflow.
    let limit_reached = data.session.percentage >= 100.0 || data.weekly.percentage >= 100.0;
    if !limit_reached || used <= 0.0 {
        return None;
    }

    Some(CreditsSection {
        percentage: ((used / total) * 100.0).clamp(0.0, 100.0),
        remaining: (total - used).max(0.0),
        total,
    })
}

pub(super) fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let mut last_error = PollError::RequestFailed;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .header("Authorization", &format!("Bearer {token}"))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(error) => {
                last_error = usage_request_error(&error);
                continue;
            }
        };

        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            diagnose::log(format!(
                "messages endpoint returned auth error status {status}; re-login required"
            ));
            return Err(PollError::HttpStatus(status));
        }

        let h5 = response
            .headers()
            .get("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response
            .headers()
            .get("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.headers().get("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
        last_error = if response.status().is_client_error() || response.status().is_server_error() {
            PollError::HttpStatus(status)
        } else {
            PollError::RequestFailed
        };
    }

    Err(last_error)
}

pub(super) fn parse_rate_limit_headers(response: &HttpResponse) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));
    data.session.available = data.session.resets_at.is_some()
        || response
            .headers()
            .contains_key("anthropic-ratelimit-unified-5h-utilization");
    data.weekly.available = data.weekly.resets_at.is_some()
        || response
            .headers()
            .contains_key("anthropic-ratelimit-unified-7d-utilization");

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");
    let claim = response
        .headers()
        .get("anthropic-ratelimit-unified-representative-claim")
        .and_then(|value| value.to_str().ok());
    data.session.available |= claim == Some("five_hour");
    data.weekly.available |= claim == Some("seven_day");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response
            .headers()
            .get("anthropic-ratelimit-unified-status")
            .and_then(|value| value.to_str().ok());
        if status == Some("rejected") {
            match claim {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
            // Retain the legacy reset binding, but a shared reset alone does
            // not establish that the five-hour window exists.
        }
    }

    data
}

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let sources = if all_sources {
        all_known_credential_sources()
    } else {
        read_first_credentials()
            .map(|credentials| vec![credentials.source])
            .unwrap_or_else(all_known_credential_sources)
    };

    let mut snapshot: Vec<String> = sources
        .into_iter()
        .filter_map(|source| credential_watch_signature(&source))
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn refresh_credentials(credentials: Credentials) -> Result<Credentials, PollError> {
    if !is_token_expired(credentials.expires_at) {
        return Ok(credentials);
    }
    let source = credentials.source;
    cli_refresh_token(&source);
    // An expired login is still a selected account. Do not replace it with
    // another account's login when its refresh fails.
    read_credentials_from_source(&source)
        .filter(|credentials| !is_token_expired(credentials.expires_at))
        .ok_or(PollError::TokenExpired)
}

fn cli_refresh_token(source: &CredentialSource) {
    let CredentialSource::File(path) = source;
    // The CLI only owns this filename. A custom export is read-only.
    if path
        .file_name()
        .is_some_and(|name| name == ".credentials.json")
    {
        if let Some(directory) = path.parent() {
            cli_refresh_in(directory);
        }
    }
}

/// A one-token prompt makes the Claude CLI renew its OAuth login in place.
fn cli_refresh_in(directory: &Path) {
    let Some(claude) = super::cli::find_executable("claude") else {
        diagnose::log("Claude token expired and the claude CLI was not found on PATH");
        return;
    };
    diagnose::log(format!(
        "attempting Claude token refresh via {}",
        claude.display()
    ));
    // Any prompt renews the OAuth login. Keep it cheap and inert: no tools,
    // no settings files (so no hooks), no MCP servers, nothing saved.
    let mut command = super::cli::command(
        &claude,
        &[
            "-p",
            ".",
            "--model",
            "claude-haiku-4-5",
            "--tools",
            "",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
        ],
    );
    command.env("CLAUDE_CONFIG_DIR", directory);
    match command.spawn() {
        Ok(mut child) => {
            super::cli::wait_for(&mut child, Duration::from_secs(30));
        }
        Err(error) => diagnose::log_error("unable to spawn Claude token refresh", error),
    }
}

fn read_first_credentials() -> Option<Credentials> {
    credential_source().and_then(|source| read_credentials_from_source(&source))
}

fn read_credentials_file(path: &Path) -> Option<Credentials> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read Claude credentials at {}", path.display()),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::File(path.to_path_buf()))
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    let CredentialSource::File(path) = source;
    read_credentials_file(path)
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    Some(Credentials {
        access_token: oauth
            .get("accessToken")?
            .as_str()
            .filter(|token| !token.trim().is_empty())?
            .to_string(),
        expires_at: oauth.get("expiresAt").and_then(|value| value.as_i64()),
        source,
    })
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    credential_source().into_iter().collect()
}

/// `$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`.
fn credential_source() -> Option<CredentialSource> {
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|value| !value.is_empty()) {
        return crate::accounts::environment_directory(crate::providers::ProviderId::Claude)
            .map(|directory| CredentialSource::File(directory.join(".credentials.json")));
    }
    Some(CredentialSource::File(
        dirs::home_dir()?.join(".claude").join(".credentials.json"),
    ))
}

pub(super) fn native_credential_path() -> Option<PathBuf> {
    match credential_source()? {
        CredentialSource::File(path) if read_credentials_file(&path).is_some() => Some(path),
        _ => None,
    }
}

fn credential_watch_signature(source: &CredentialSource) -> Option<String> {
    let CredentialSource::File(path) = source;
    let key = format!("file:{}", path.display());
    Some(match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    })
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|expires_at| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        now >= expires_at
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_failures_keep_their_status_for_account_display() {
        for status in [401, 403, 429, 500, 503] {
            assert_eq!(
                usage_request_error(&ureq::Error::StatusCode(status)),
                PollError::HttpStatus(status)
            );
        }
    }

    #[test]
    fn explicit_missing_or_expired_export_never_uses_another_login() {
        let directory = std::env::temp_dir().join(format!(
            "claude-profile-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("export.json");
        assert_eq!(poll_account(&path), Err(PollError::NoCredentials));
        std::fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"fixture-token","expiresAt":0}}"#,
        )
        .unwrap();
        assert_eq!(poll_account(&path), Err(PollError::TokenExpired));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn malformed_usage_responses_are_distinct_from_network_and_auth_failures() {
        for json in [
            "not json",
            "{}",
            r#"{"five_hour":{"utilization":"wrong"}}"#,
            r#"{"limits":[{"kind":"weekly_scoped"}]}"#,
        ] {
            let mut body = ureq::Body::builder().data(json.as_bytes().to_vec());
            assert_eq!(
                parse_usage_body(&mut body),
                Err(PollError::UnexpectedResponse)
            );
        }
        assert!(PollError::UnexpectedResponse.is_transient());
        assert!(!PollError::UnexpectedResponse.is_auth());
    }

    #[test]
    fn array_only_usage_fills_standard_windows_without_using_scoped_caps() {
        let mut body = ureq::Body::builder().data(br#"{"limits":[
            {"kind":"session","percent":29},
            {"kind":"weekly_all","percent":26},
            {"kind":"weekly_scoped","percent":99,"is_active":true,"scope":{"model":{"display_name":"Fable"}}}
        ]}"#.to_vec());
        let data = parse_usage_body(&mut body).unwrap();
        assert_eq!(data.session.percentage, 29.0);
        assert_eq!(data.weekly.percentage, 26.0);
        assert_eq!(data.limits.len(), 3);
        assert!(data.session.available && data.weekly.available);
        let data = usage_from_json(
            r#"{"five_hour":{"utilization":10},"seven_day":{"utilization":20},"limits":[{"kind":"session","percent":90},{"kind":"weekly_all","percent":95}]}"#,
        );
        assert_eq!(data.session.percentage, 10.0);
        assert_eq!(data.weekly.percentage, 20.0);
    }

    fn usage_from_json(json: &str) -> UsageData {
        let response: UsageResponse =
            serde_json::from_str(json).expect("the fixture should deserialize");
        usage_from_response(response)
    }

    #[test]
    fn reported_windows_without_resets_remain_available_at_zero_usage() {
        for percentage in [0.0, 42.0] {
            let data = usage_from_json(&format!(
                r#"{{"five_hour":{{"utilization":{percentage},"resets_at":null}},"seven_day":null}}"#,
            ));
            assert!(data.session.available);
            assert_eq!(data.session.percentage, percentage);
            assert!(data.session.resets_at.is_none());
            assert!(!data.weekly.available);
        }
    }

    #[test]
    fn utilization_headers_report_windows_without_reset_headers() {
        for (header, session, weekly) in [
            ("anthropic-ratelimit-unified-5h-utilization", true, false),
            ("anthropic-ratelimit-unified-7d-utilization", false, true),
            ("anthropic-ratelimit-unified-status", false, false),
        ] {
            let response = ureq::http::Response::builder()
                .header(header, "0")
                .body(ureq::Body::builder().data(Vec::new()))
                .unwrap();
            let data = parse_rate_limit_headers(&response);
            assert_eq!(data.session.available, session);
            assert_eq!(data.weekly.available, weekly);
        }
    }

    #[test]
    fn shared_reset_headers_do_not_invent_a_session_window() {
        for status in ["allowed", "rejected"] {
            for (claim, session, weekly) in [
                ("five_hour", true, false),
                ("seven_day", false, true),
                ("unknown", false, false),
            ] {
                let response = ureq::http::Response::builder()
                    .header("anthropic-ratelimit-unified-status", status)
                    .header("anthropic-ratelimit-unified-reset", "1787198224")
                    .header("anthropic-ratelimit-unified-representative-claim", claim)
                    .body(ureq::Body::builder().data(Vec::new()))
                    .unwrap();
                let data = parse_rate_limit_headers(&response);
                assert_eq!(data.session.available, session);
                assert_eq!(data.weekly.available, weekly);
            }
        }
    }

    fn status_error(code: u16) -> ureq::Error {
        ureq::Error::StatusCode(code)
    }

    #[test]
    fn rate_limits_and_server_faults_do_not_trigger_the_messages_fallback() {
        // Spending quota on a Messages request is the wrong answer to being
        // rate limited, and it feeds the condition that caused it.
        assert_eq!(
            classify_usage_failure(&status_error(429)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(500)),
            UsageEndpointFailure::Transient
        );
        assert_eq!(
            classify_usage_failure(&status_error(503)),
            UsageEndpointFailure::Transient
        );
    }

    #[test]
    fn rejected_credentials_are_kept_separate_from_an_absent_endpoint() {
        assert_eq!(
            classify_usage_failure(&status_error(401)),
            UsageEndpointFailure::Auth
        );
        assert_eq!(
            classify_usage_failure(&status_error(403)),
            UsageEndpointFailure::Auth
        );
        // A 404 is the case the Messages API fallback exists to cover.
        assert_eq!(
            classify_usage_failure(&status_error(404)),
            UsageEndpointFailure::Unsupported
        );
    }

    #[test]
    fn spend_becomes_a_credit_gauge_against_the_plan_cap() {
        // Shape taken from a live /api/oauth/usage response.
        let data = usage_from_json(
            r#"{
                "seven_day": {"utilization": 100.0, "resets_at": null},
                "spend": {
                    "used": {"amount_minor": 1359, "currency": "USD", "exponent": 2},
                    "limit": {"amount_minor": 5000, "currency": "USD", "exponent": 2},
                    "percent": 27,
                    "enabled": true
                }
            }"#,
        );

        let credits = data.credits.expect("enabled spend should expose a gauge");
        assert!((credits.percentage - 27.18).abs() < 0.01, "{credits:?}");
        assert!((credits.remaining - 36.41).abs() < 0.001, "{credits:?}");
        assert_eq!(credits.total, 50.0);
    }

    #[test]
    fn disabled_or_uncapped_spend_gets_no_gauge() {
        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 0, "exponent": 2},
                          "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": false}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(
            r#"{"seven_day": {"utilization": 100.0},
                "spend": {"used": {"amount_minor": 10, "exponent": 2},
                          "limit": {"amount_minor": 0, "exponent": 2}, "enabled": true}}"#
        )
        .credits
        .is_none());

        assert!(usage_from_json(r#"{"seven_day": {"utilization": 1.0}}"#)
            .credits
            .is_none());
    }

    #[test]
    fn the_gauge_waits_for_a_spent_window_and_for_credits_to_be_in_play() {
        let spend = r#""spend": {"used": {"amount_minor": 1359, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2}, "enabled": true}"#;

        // Room left in both windows, so the bars stay on the ordinary limits.
        let json = format!(r#"{{"five_hour": {{"utilization": 40.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_none());

        // A spent five-hour window is enough; it need not be the weekly one.
        let json = format!(r#"{{"five_hour": {{"utilization": 100.0}}, {spend}}}"#);
        assert!(usage_from_json(&json).credits.is_some());

        // Spent window, but nothing charged to credits yet.
        let json = r#"{"five_hour": {"utilization": 100.0},
                       "spend": {"used": {"amount_minor": 0, "exponent": 2},
                                 "limit": {"amount_minor": 5000, "exponent": 2},
                                 "enabled": true}}"#;
        assert!(usage_from_json(json).credits.is_none());
    }
}
