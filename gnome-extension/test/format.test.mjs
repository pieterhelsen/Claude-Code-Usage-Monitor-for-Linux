// Run with: node --test gnome-extension/test/
import assert from 'node:assert/strict';
import {test} from 'node:test';

import {
    countdown, countdownLong, panelLabel, pickFigure, pickProvider, windowRow,
} from '../format.js';

const NOW = 1_000_000;

function snapshot(overrides = {}) {
    return {
        schema_version: 1,
        usage_countdown: false,
        providers: [
            {
                id: 'claude',
                enabled: true,
                status: 'ok',
                error: null,
                usage: {
                    stale: false,
                    headline: {used_percent: 42, remaining_percent: 58, source: 'five_hour', resets_at_unix: NOW + 3 * 3600 + 60},
                    windows: [
                        {kind: 'five_hour', label: '5h', used_percent: 42, remaining_percent: 58, resets_at_unix: NOW + 3 * 3600 + 60},
                        {kind: 'weekly', label: '7d', used_percent: 12, remaining_percent: 88, resets_at_unix: NOW + 4 * 86400},
                    ],
                    limits: [],
                },
                accounts: [],
            },
            {id: 'codex', enabled: false, status: 'disabled', error: null, usage: null, accounts: []},
            {
                id: 'cursor', enabled: true, status: 'error', usage: null, accounts: [],
                error: {code: 'no_credentials', message: 'No usable login found; sign in first'},
            },
        ],
        ...overrides,
    };
}

const PREFS = {provider: 'auto', window: 'headline', showPercent: true, showCountdown: true, warn: 70, critical: 90};

test('auto picks the first enabled provider with usage', () => {
    assert.equal(pickProvider(snapshot(), 'auto').id, 'claude');
    assert.equal(pickProvider(snapshot(), 'cursor').id, 'cursor');
    // A disabled preference falls back to auto.
    assert.equal(pickProvider(snapshot(), 'codex').id, 'claude');
    assert.equal(pickProvider({providers: []}, 'auto'), null);
});

test('a named window is used when reported, else the headline', () => {
    const claude = snapshot().providers[0];
    assert.equal(pickFigure(claude, 'weekly').used, 12);
    assert.equal(pickFigure(claude, 'monthly').used, 42);
    assert.equal(pickFigure(claude, 'headline').label, '5h');
});

test('panel label shows percent and countdown', () => {
    assert.deepEqual(panelLabel(snapshot(), PREFS, NOW), {text: '42% · 3h', state: 'ok', fraction: 0.42});
    assert.equal(panelLabel(snapshot(), {...PREFS, showCountdown: false}, NOW).text, '42%');
    assert.equal(panelLabel(snapshot({usage_countdown: true}), PREFS, NOW).text, '58% · 3h');
});

test('thresholds use the used figure even when counting down', () => {
    const hot = snapshot({usage_countdown: true});
    hot.providers[0].usage.headline.used_percent = 91;
    hot.providers[0].usage.headline.remaining_percent = 9;
    const label = panelLabel(hot, PREFS, NOW);
    assert.equal(label.state, 'critical');
    assert.equal(label.text, '9% · 3h');
    hot.providers[0].usage.headline.used_percent = 75;
    assert.equal(panelLabel(hot, PREFS, NOW).state, 'warn');
});

test('loading, error and stale states', () => {
    assert.equal(panelLabel(null, PREFS, NOW).state, 'loading');
    assert.deepEqual(panelLabel(snapshot(), {...PREFS, provider: 'cursor'}, NOW), {text: '!', state: 'error', fraction: null});
    const stale = snapshot();
    stale.providers[0].status = 'stale';
    assert.equal(panelLabel(stale, PREFS, NOW).state, 'stale');
});

test('menu rows describe usage and reset time', () => {
    const [fiveHour] = snapshot().providers[0].usage.windows;
    assert.deepEqual(windowRow(fiveHour, false, NOW, 70, 90), {
        label: '5h', figure: '42% used', detail: 'resets in 3h 1m', fraction: 0.42, state: 'ok',
    });
    const credits = {kind: 'credits', label: 'Credits', used_percent: 27, remaining_percent: 73, resets_at_unix: null, remaining_amount: 36.5, total_amount: 50};
    assert.equal(windowRow(credits, true, NOW, 70, 90).detail, '36.50 of 50.00 left');
    assert.equal(windowRow(credits, true, NOW, 70, 90).figure, '73% left');
});

test('countdowns match the daemon formatting', () => {
    assert.equal(countdown(30), '<1m');
    assert.equal(countdown(45 * 60), '45m');
    assert.equal(countdown(3 * 3600 + 600), '3h');
    assert.equal(countdown(2 * 86400 + 5), '2d');
    assert.equal(countdownLong(3 * 3600 + 12 * 60), '3h 12m');
    assert.equal(countdownLong(86400 + 5 * 3600), '1d 5h');
    assert.equal(countdownLong(20), '1m');
});
