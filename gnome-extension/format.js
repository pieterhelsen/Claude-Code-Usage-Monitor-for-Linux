// Pure presentation logic for the usage snapshot (see src/snapshot.rs).
//
// No GNOME imports here, so the same module runs under GNOME Shell, the
// preferences process and `node --test` (gnome-extension/test/).

export const WINDOW_LABELS = {
    headline: 'Closest to its limit',
    five_hour: '5-hour window',
    weekly: 'Weekly / long window',
    monthly: 'Monthly window',
    credits: 'Credits',
};

/**
 * The provider the panel should show.
 *
 * @param {object} snapshot usage snapshot from the daemon
 * @param {string} preferred provider id, or 'auto'
 * @returns {object|null}
 */
export function pickProvider(snapshot, preferred) {
    const providers = snapshot?.providers ?? [];
    const enabled = providers.filter(p => p.enabled);
    if (preferred && preferred !== 'auto') {
        const chosen = enabled.find(p => p.id === preferred);
        if (chosen)
            return chosen;
    }
    return enabled.find(p => p.usage) ?? enabled[0] ?? null;
}

/**
 * The single figure for a provider: the headline or one named window.
 * Falls back to the headline when the chosen window is not reported.
 *
 * @param {object} provider provider entry of the snapshot
 * @param {string} windowKind 'headline' or a window kind
 * @returns {{used: number, remaining: number, resetsAt: number|null, label: string}|null}
 */
export function pickFigure(provider, windowKind) {
    const usage = provider?.usage;
    if (!usage)
        return null;
    if (windowKind && windowKind !== 'headline') {
        const window = usage.windows.find(w => w.kind === windowKind);
        if (window) {
            return {
                used: window.used_percent,
                remaining: window.remaining_percent,
                resetsAt: window.resets_at_unix ?? null,
                label: window.label,
            };
        }
    }
    const headline = usage.headline;
    if (!headline)
        return null;
    const window = usage.windows.find(w => w.kind === headline.source);
    return {
        used: headline.used_percent,
        remaining: headline.remaining_percent,
        resetsAt: headline.resets_at_unix ?? null,
        label: window?.label ?? '',
    };
}

/** Compact countdown for the panel: `<1m`, `45m`, `3h`, `2d`. */
export function countdown(seconds) {
    if (seconds < 60)
        return '<1m';
    if (seconds < 3600)
        return `${Math.floor(seconds / 60)}m`;
    if (seconds < 86400)
        return `${Math.floor(seconds / 3600)}h`;
    return `${Math.floor(seconds / 86400)}d`;
}

/** Longer countdown for the menu: `12m`, `3h 12m`, `2d 5h`. */
export function countdownLong(seconds) {
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    if (days > 0)
        return `${days}d ${hours}h`;
    if (hours > 0)
        return `${hours}h ${minutes}m`;
    return `${Math.max(minutes, seconds > 0 ? 1 : 0)}m`;
}

/** "just now", "5 min ago", "2 h ago". */
export function ago(seconds) {
    if (seconds < 60)
        return 'just now';
    if (seconds < 3600)
        return `${Math.floor(seconds / 60)} min ago`;
    if (seconds < 86400)
        return `${Math.floor(seconds / 3600)} h ago`;
    return `${Math.floor(seconds / 86400)} d ago`;
}

/**
 * Severity of a usage figure. Thresholds apply to what has been *used*, even
 * when the label shows what is left.
 */
export function level(used, warn, critical) {
    if (used >= critical)
        return 'critical';
    if (used >= warn)
        return 'warn';
    return 'ok';
}

/**
 * Everything the top-bar label needs.
 *
 * @param {object|null} snapshot usage snapshot, or null before the first reply
 * @param {object} prefs {provider, window, showPercent, showCountdown, warn, critical}
 * @param {number} now unix seconds
 * @returns {{text: string, state: string, fraction: number|null}}
 *   state is one of loading, ok, warn, critical, stale, error
 */
export function panelLabel(snapshot, prefs, now) {
    if (!snapshot)
        return {text: '…', state: 'loading', fraction: null};
    const provider = pickProvider(snapshot, prefs.provider);
    if (!provider)
        return {text: '—', state: 'error', fraction: null};
    const figure = pickFigure(provider, prefs.window);
    if (!figure) {
        if (provider.error)
            return {text: '!', state: 'error', fraction: null};
        return {text: '…', state: 'loading', fraction: null};
    }

    const countDown = Boolean(snapshot.usage_countdown);
    const shown = countDown ? figure.remaining : figure.used;
    const parts = [];
    if (prefs.showPercent)
        parts.push(`${Math.round(shown)}%`);
    if (prefs.showCountdown && figure.resetsAt && figure.resetsAt > now)
        parts.push(countdown(figure.resetsAt - now));

    let state = level(figure.used, prefs.warn, prefs.critical);
    if (state === 'ok' && provider.status === 'stale')
        state = 'stale';
    return {text: parts.join(' · '), state, fraction: shown / 100};
}

/**
 * One menu row for a usage window.
 *
 * @returns {{label: string, figure: string, detail: string, fraction: number, state: string}}
 */
export function windowRow(window, countDown, now, warn, critical) {
    const shown = countDown ? window.remaining_percent : window.used_percent;
    const details = [];
    if (window.resets_at_unix && window.resets_at_unix > now)
        details.push(`resets in ${countdownLong(window.resets_at_unix - now)}`);
    if (window.remaining_amount !== undefined && window.total_amount !== undefined)
        details.push(`${window.remaining_amount.toFixed(2)} of ${window.total_amount.toFixed(2)} left`);
    return {
        label: window.label,
        figure: `${Math.round(shown)}% ${countDown ? 'left' : 'used'}`,
        detail: details.join(' · '),
        fraction: shown / 100,
        state: level(window.used_percent, warn, critical),
    };
}

/** Short human text for a provider without usage. */
export function providerProblem(provider) {
    if (provider.error)
        return provider.error.message;
    if (provider.status === 'no_data')
        return 'Waiting for the first poll';
    return null;
}
