use super::*;

fn usage_with_session_percent(percentage: f64) -> UsageData {
    UsageData {
        limits: Vec::new(),
        session: UsageSection {
            available: true,
            percentage,
            resets_at: None,
        },
        weekly: UsageSection::default(),
        weekly_label: None,
        monthly: None,
        credits: None,
        stale: false,
    }
}

#[test]
fn idle_window_presence_survives_cached_poll_failures() {
    let previous = AppUsageData::from_iter([(ProviderId::Claude, usage_with_session_percent(0.0))]);
    let cached: AppUsageData =
        serde_json::from_str(&serde_json::to_string(&previous).unwrap()).unwrap();
    let carried = carry_forward_failures(
        AppUsageData::default(),
        &cached,
        ProviderSet::from_enabled([ProviderId::Claude]),
    );
    let usage = carried.get(ProviderId::Claude).unwrap();
    assert!(usage.stale);
    assert!(usage.session.available);
    assert_eq!(usage.session.percentage, 0.0);
    assert!(usage.session.resets_at.is_none());
    assert!(!usage.weekly.available);
}

#[test]
fn configured_https_transport_does_not_panic() {
    crate::https_test::assert_tls_handshake(build_agent().expect("HTTP agent should build"));
}

#[test]
fn stale_usage_does_not_trigger_reset_polling() {
    let mut usage = usage_with_session_percent(42.0);
    usage.session.resets_at = Some(UNIX_EPOCH);
    usage.stale = true;

    assert!(!is_past_reset(&usage));

    let mut app_usage = AppUsageData::default();
    app_usage.insert(ProviderId::Claude, usage);
    assert!(!app_is_past_reset(&app_usage));
}

#[test]
fn iso8601_parser_applies_timezone_offsets() {
    assert_eq!(
        parse_iso8601(Some("1970-01-01T01:00:00+01:00")),
        Some(UNIX_EPOCH)
    );
    assert_eq!(
        parse_iso8601(Some("1970-01-01T00:00:00-01:00")),
        Some(UNIX_EPOCH + Duration::from_secs(3_600))
    );
    assert_eq!(
        parse_iso8601(Some("2026-03-05T08:00:00.321598Z")),
        parse_iso8601(Some("2026-03-05T08:00:00+00:00"))
    );
}

#[test]
fn iso8601_parser_validates_calendar_and_time_fields() {
    assert!(parse_iso8601(Some("2024-02-29T23:59:59Z")).is_some());
    for invalid in [
        "2023-02-29T00:00:00Z",
        "2026-00-01T00:00:00Z",
        "2026-14-01T00:00:00Z",
        "2026-01-00T00:00:00Z",
        "2026-01-01T24:00:00Z",
        "2026-01-01T00:60:00Z",
        "2026-01-01T00:00:60Z",
        "2026-01-01T00:00:00.Z",
        "2026-01-01T00:00:00+24:00",
        "1969-12-31T23:59:59Z",
    ] {
        assert_eq!(parse_iso8601(Some(invalid)), None, "accepted {invalid}");
    }
}

#[test]
fn every_registered_provider_has_a_poller() {
    for provider in ProviderId::ALL {
        assert!(
            provider_poller(provider).is_some(),
            "{} is missing a poller registration",
            provider.descriptor().key
        );
    }
}

#[test]
fn claude_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| match provider {
            ProviderId::Claude => Err(PollError::AuthRequired),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
        },
    )
    .expect("codex data should keep the poll successful");

    assert!(data.get(ProviderId::Claude).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn codex_failure_does_not_block_claude_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| match provider {
            ProviderId::Claude => Ok(usage_with_session_percent(64.0)),
            ProviderId::Codex => Err(PollError::RequestFailed),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
        },
    )
    .expect("claude data should keep the poll successful");

    assert_eq!(
        data.get(ProviderId::Claude).unwrap().session.percentage,
        64.0
    );
    assert!(data.get(ProviderId::Codex).is_none());
}

#[test]
fn returns_first_error_when_no_enabled_provider_succeeds() {
    let error = poll_with(
        ProviderSet::from_enabled(ProviderId::ALL),
        |provider| match provider {
            ProviderId::Claude => Err(PollError::AuthRequired),
            ProviderId::Codex => Err(PollError::RequestFailed),
            ProviderId::OpenCode => Err(PollError::NoCredentials),
            ProviderId::Cursor => Err(PollError::NoCredentials),
            ProviderId::Grok => Err(PollError::NoCredentials),
        },
    )
    .expect_err("all-provider failure should return an error");

    assert_eq!(
        error,
        PollFailure {
            provider: ProviderId::Claude,
            error: PollError::AuthRequired,
        }
    );
}

#[test]
fn ready_providers_are_published_before_slow_providers_finish() {
    let (release, wait) = std::sync::mpsc::channel();
    let wait = std::sync::Mutex::new(wait);
    let mut published = Vec::new();
    let data = poll_concurrently_with_progress(
        ProviderSet::from_enabled([ProviderId::Cursor, ProviderId::OpenCode]),
        |provider| {
            if provider == ProviderId::OpenCode {
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            Ok(usage_with_session_percent(42.0))
        },
        |update| {
            let provider = update.iter().next().unwrap().0;
            published.push(provider);
            if provider == ProviderId::Cursor {
                release.send(()).unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(published, [ProviderId::Cursor, ProviderId::OpenCode]);
    assert_eq!(data.iter().count(), 2);
}

#[test]
fn progress_keeps_pending_provider_readings_until_they_finish() {
    let previous = AppUsageData::from_iter([
        (ProviderId::Cursor, usage_with_session_percent(10.0)),
        (ProviderId::OpenCode, usage_with_session_percent(20.0)),
    ]);
    let update = AppUsageData::from_iter([(ProviderId::Cursor, usage_with_session_percent(15.0))]);
    let merged = merge_poll_progress(
        update,
        &previous,
        &crate::accounts::AccountSettings::default(),
    );
    assert_eq!(
        merged.get(ProviderId::Cursor).unwrap().session.percentage,
        15.0
    );
    assert_eq!(
        merged.get(ProviderId::OpenCode),
        previous.get(ProviderId::OpenCode)
    );
}

#[test]
fn concurrent_polling_is_bounded_and_preserves_results() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let data = poll_concurrently_with(ProviderSet::from_enabled(ProviderId::ALL), |provider| {
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        active.fetch_sub(1, Ordering::SeqCst);
        Ok(usage_with_session_percent(f64::from(provider as u8)))
    })
    .expect("all concurrent provider polls should succeed");

    assert_eq!(data.iter().count(), ProviderId::ALL.len());
    assert!(peak.load(Ordering::SeqCst) > 1);
    assert!(peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROVIDER_POLLS);
}

#[test]
fn concurrent_polling_reports_the_first_provider_error_deterministically() {
    let error = poll_concurrently_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| {
            if provider == ProviderId::Claude {
                std::thread::sleep(Duration::from_millis(20));
                Err(PollError::AuthRequired)
            } else {
                Err(PollError::RequestFailed)
            }
        },
    )
    .expect_err("both provider polls should fail");

    assert_eq!(
        error,
        PollFailure {
            provider: ProviderId::Claude,
            error: PollError::AuthRequired,
        }
    );
}

#[test]
fn opencode_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::OpenCode]),
        |provider| match provider {
            ProviderId::Claude => unreachable!("Claude Code is disabled"),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::OpenCode => Err(PollError::NoCredentials),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
        },
    )
    .expect("Codex data should keep the poll successful");

    assert!(data.get(ProviderId::OpenCode).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn cursor_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Cursor]),
        |provider| match provider {
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Cursor => Err(PollError::NoCredentials),
            _ => unreachable!("provider is disabled"),
        },
    )
    .expect("Codex data should keep the poll successful");

    assert!(data.get(ProviderId::Cursor).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn one_provider_failing_does_not_blank_its_row() {
    let previous: AppUsageData = [
        (ProviderId::Claude, usage_with_session_percent(21.0)),
        (ProviderId::Codex, usage_with_session_percent(3.0)),
    ]
    .into_iter()
    .collect();

    // Only Codex answered this cycle.
    let fresh: AppUsageData = [(ProviderId::Codex, usage_with_session_percent(9.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        fresh,
        &previous,
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
    );

    let claude = merged.get(ProviderId::Claude).expect("claude is kept");
    assert_eq!(claude.session.percentage, 21.0);
    assert!(claude.stale, "a carried reading must be marked stale");

    let codex = merged.get(ProviderId::Codex).expect("codex refreshed");
    assert_eq!(codex.session.percentage, 9.0);
    assert!(!codex.stale, "a fresh reading must not be marked stale");
}

#[test]
fn a_disabled_provider_is_not_resurrected() {
    let previous: AppUsageData = [(ProviderId::Claude, usage_with_session_percent(21.0))]
        .into_iter()
        .collect();
    let fresh: AppUsageData = [(ProviderId::Codex, usage_with_session_percent(9.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        fresh,
        &previous,
        ProviderSet::from_enabled([ProviderId::Codex]),
    );

    assert!(merged.get(ProviderId::Claude).is_none());
}

#[test]
fn all_failed_providers_can_carry_their_previous_readings() {
    let previous: AppUsageData = [(ProviderId::Claude, usage_with_session_percent(21.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        AppUsageData::default(),
        &previous,
        ProviderSet::from_enabled([ProviderId::Claude]),
    );

    let claude = merged.get(ProviderId::Claude).expect("claude is kept");
    assert_eq!(claude.session.percentage, 21.0);
    assert!(claude.stale, "the carried reading must be marked stale");
}
