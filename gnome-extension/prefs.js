// Preferences window: panel appearance (GSettings) and daemon settings (D-Bus).

import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {BUS_NAME, MonitorProxy, OBJECT_PATH, PROVIDER_SETTING_KEYS} from './dbus.js';
import {WINDOW_LABELS} from './format.js';

const PROVIDER_CHOICES = [
    ['auto', 'Automatic'],
    ['claude', 'Claude Code'],
    ['codex', 'Codex'],
    ['opencode', 'OpenCode'],
    ['cursor', 'Cursor'],
    ['grok', 'Grok'],
];
const POSITION_CHOICES = [['left', 'Left'], ['center', 'Center'], ['right', 'Right']];
const INTERVAL_MINUTES = [1, 5, 15, 30, 60];

/** An Adw.ComboRow that stores one of `choices` ([value, label]) in a string key. */
function stringComboRow(settings, key, title, choices, subtitle = '') {
    const row = new Adw.ComboRow({
        title,
        subtitle,
        model: Gtk.StringList.new(choices.map(([, label]) => label)),
    });
    const sync = () => {
        const index = choices.findIndex(([value]) => value === settings.get_string(key));
        row.selected = Math.max(0, index);
    };
    sync();
    row.connect('notify::selected', () => {
        const value = choices[row.selected]?.[0];
        if (value && value !== settings.get_string(key))
            settings.set_string(key, value);
    });
    return row;
}

function switchRow(settings, key, title, subtitle = '') {
    const row = new Adw.SwitchRow({title, subtitle});
    settings.bind(key, row, 'active', Gio.SettingsBindFlags.DEFAULT);
    return row;
}

function spinRow(settings, key, title, subtitle = '') {
    const row = Adw.SpinRow.new_with_range(1, 100, 1);
    row.title = title;
    row.subtitle = subtitle;
    settings.bind(key, row, 'value', Gio.SettingsBindFlags.DEFAULT);
    return row;
}

export default class UsageMonitorPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();
        window._settings = settings;
        window.set_default_size(640, 720);
        window.add(this._panelPage(settings));
        window.add(this._providersPage(window));
    }

    _panelPage(settings) {
        const page = new Adw.PreferencesPage({
            title: 'Panel',
            icon_name: 'preferences-desktop-display-symbolic',
        });

        const shown = new Adw.PreferencesGroup({title: 'What the top bar shows'});
        shown.add(stringComboRow(settings, 'panel-provider', 'Provider', PROVIDER_CHOICES,
            'Automatic shows the first enabled provider with data'));
        shown.add(stringComboRow(settings, 'panel-window', 'Usage window',
            Object.entries(WINDOW_LABELS)));
        shown.add(switchRow(settings, 'show-icon', 'Icon'));
        shown.add(switchRow(settings, 'show-percent', 'Percentage'));
        shown.add(switchRow(settings, 'show-countdown', 'Time until reset'));
        shown.add(switchRow(settings, 'show-bar', 'Usage bar'));
        page.add(shown);

        const colours = new Adw.PreferencesGroup({
            title: 'Warning colours',
            description: 'Based on usage spent, even when showing what is left',
        });
        colours.add(spinRow(settings, 'warn-threshold', 'Amber from', 'Percent used'));
        colours.add(spinRow(settings, 'critical-threshold', 'Red from', 'Percent used'));
        page.add(colours);

        const placement = new Adw.PreferencesGroup({title: 'Placement'});
        placement.add(stringComboRow(settings, 'panel-position', 'Top bar area', POSITION_CHOICES));
        const index = Adw.SpinRow.new_with_range(0, 20, 1);
        index.title = 'Position in area';
        index.subtitle = '0 is the leftmost slot';
        settings.bind('panel-index', index, 'value', Gio.SettingsBindFlags.DEFAULT);
        placement.add(index);
        page.add(placement);
        return page;
    }

    _providersPage(window) {
        const page = new Adw.PreferencesPage({
            title: 'Providers',
            icon_name: 'network-server-symbolic',
        });
        const status = new Adw.PreferencesGroup();
        const statusRow = new Adw.ActionRow({title: 'Connecting to the usage daemon…'});
        status.add(statusRow);
        page.add(status);

        const providers = new Adw.PreferencesGroup({
            title: 'Providers',
            description: 'Each provider reads the login of its own CLI or app on this computer',
        });
        const polling = new Adw.PreferencesGroup({title: 'Refreshing'});
        page.add(providers);
        page.add(polling);

        const about = new Adw.PreferencesGroup({
            title: 'Accounts',
            description: 'Several Claude Code or Codex logins can be listed under "accounts" in ~/.config/claude-code-usage-monitor/settings.json.',
        });
        page.add(about);

        const toast = message => window.add_toast(new Adw.Toast({title: message, timeout: 4}));
        let proxy = null;
        let daemonSettings = null;

        const save = () => {
            proxy.SetSettingsAsync(JSON.stringify(daemonSettings))
                .then(([normalized]) => {
                    daemonSettings = JSON.parse(normalized);
                })
                .catch(error => toast(`Could not save: ${error.message}`));
        };

        const populate = snapshot => {
            for (const provider of snapshot.providers) {
                const key = PROVIDER_SETTING_KEYS[provider.id];
                if (!key)
                    continue;
                const row = new Adw.SwitchRow({
                    title: provider.display_name,
                    subtitle: provider.error?.message ?? provider.description,
                    active: Boolean(daemonSettings[key] ?? provider.enabled),
                });
                row.connect('notify::active', () => {
                    daemonSettings[key] = row.active;
                    save();
                });
                providers.add(row);
            }

            const minutes = Math.round(daemonSettings.poll_interval_ms / 60000);
            const choices = INTERVAL_MINUTES.includes(minutes)
                ? INTERVAL_MINUTES
                : [...INTERVAL_MINUTES, minutes].sort((a, b) => a - b);
            const interval = new Adw.ComboRow({
                title: 'Refresh every',
                model: Gtk.StringList.new(choices.map(m => (m === 60 ? '1 hour' : `${m} min`))),
                selected: choices.indexOf(minutes),
            });
            interval.connect('notify::selected', () => {
                daemonSettings.poll_interval_ms = choices[interval.selected] * 60000;
                save();
            });
            polling.add(interval);

            const countdown = new Adw.SwitchRow({
                title: 'Show what is left',
                subtitle: 'Count down from 100% instead of up from 0%',
                active: Boolean(daemonSettings.usage_countdown),
            });
            countdown.connect('notify::active', () => {
                daemonSettings.usage_countdown = countdown.active;
                save();
            });
            polling.add(countdown);

            const refresh = new Gtk.Button({label: 'Refresh now', valign: Gtk.Align.CENTER});
            refresh.connect('clicked', () => {
                proxy.RefreshAsync()
                    .then(() => toast('Refreshing usage'))
                    .catch(error => toast(error.message));
            });
            const refreshRow = new Adw.ActionRow({title: 'Poll every enabled provider now'});
            refreshRow.add_suffix(refresh);
            polling.add(refreshRow);
        };

        const fail = error => {
            statusRow.title = 'Usage daemon unavailable';
            statusRow.subtitle = `${error.message}\nInstall or start claude-code-usage-monitor, then reopen this window.`;
        };

        // eslint-disable-next-line no-new
        new MonitorProxy(Gio.DBus.session, BUS_NAME, OBJECT_PATH, (created, error) => {
            if (error) {
                fail(error);
                return;
            }
            proxy = created;
            Promise.all([proxy.GetSettingsAsync(), proxy.GetUsageAsync()])
                .then(([[settingsJson], [usageJson]]) => {
                    daemonSettings = JSON.parse(settingsJson);
                    const snapshot = JSON.parse(usageJson);
                    statusRow.title = `Usage daemon ${snapshot.daemon_version}`;
                    statusRow.subtitle = 'Connected';
                    populate(snapshot);
                })
                .catch(fail);
        });
        return page;
    }
}
