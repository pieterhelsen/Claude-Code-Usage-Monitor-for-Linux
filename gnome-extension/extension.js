// Claude Code Usage Monitor: top-bar indicator for GNOME Shell 46+.
//
// All data comes from the claude-code-usage-monitor daemon over D-Bus; this
// file only renders it. Presentation rules live in format.js.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {BUS_NAME, MonitorProxy, OBJECT_PATH, SCHEMA_VERSION} from './dbus.js';
import * as Format from './format.js';

const TICK_SECONDS = 30;
const PANEL_BAR_WIDTH = 32;
const MENU_BAR_WIDTH = 72;
const STATE_CLASSES = ['loading', 'ok', 'warn', 'critical', 'stale', 'error']
    .map(state => `ccum-${state}`);

function nowUnix() {
    return Math.floor(Date.now() / 1000);
}

/** A small horizontal gauge: a track with a fill whose width follows `fraction`. */
const Gauge = GObject.registerClass(
class Gauge extends St.BoxLayout {
    _init(width, styleClass) {
        // A horizontal box packs the fill from the left edge of the track.
        // (A BinLayout would centre it.)
        super._init({
            style_class: `ccum-gauge ${styleClass}`,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._width = width;
        this.set_style(`width: ${width}px;`);
        this._fill = new St.Widget({
            style_class: 'ccum-gauge-fill',
            x_expand: false,
            y_expand: true,
            y_align: Clutter.ActorAlign.FILL,
        });
        this.add_child(this._fill);
    }

    update(fraction, state) {
        const clamped = Math.max(0, Math.min(1, fraction ?? 0));
        this._fill.set_style(`width: ${Math.round(clamped * this._width)}px;`);
        for (const cls of STATE_CLASSES)
            this._fill.remove_style_class_name(cls);
        this._fill.add_style_class_name(`ccum-${state}`);
    }
});

/** A non-interactive menu row: label, gauge, figure and detail. */
const WindowItem = GObject.registerClass(
class WindowItem extends PopupMenu.PopupBaseMenuItem {
    _init(row) {
        super._init({reactive: false, can_focus: false, style_class: 'ccum-window-item'});
        this.add_child(new St.Label({
            text: row.label,
            style_class: 'ccum-window-label',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        const gauge = new Gauge(MENU_BAR_WIDTH, 'ccum-menu-gauge');
        gauge.update(row.fraction, row.state);
        this.add_child(gauge);
        this.add_child(new St.Label({
            text: row.figure,
            style_class: `ccum-window-figure ccum-${row.state}`,
            y_align: Clutter.ActorAlign.CENTER,
        }));
        this.add_child(new St.Label({
            text: row.detail,
            style_class: 'ccum-window-detail',
            x_expand: true,
            x_align: Clutter.ActorAlign.END,
            y_align: Clutter.ActorAlign.CENTER,
        }));
    }
});

const UsageIndicator = GObject.registerClass(
class UsageIndicator extends PanelMenu.Button {
    _init(extension) {
        super._init(0.5, extension.metadata.name, false);
        this._extension = extension;

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box ccum-panel-box'});
        this._icon = new St.Icon({
            gicon: Gio.icon_new_for_string(
                `${extension.path}/icons/claude-code-usage-monitor-symbolic.svg`),
            style_class: 'system-status-icon',
        });
        this._label = new St.Label({
            text: '…',
            style_class: 'ccum-panel-label',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._gauge = new Gauge(PANEL_BAR_WIDTH, 'ccum-panel-gauge');
        box.add_child(this._icon);
        box.add_child(this._label);
        box.add_child(this._gauge);
        this.add_child(box);

        this.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this._extension.render();
        });
    }

    /**
     * @param {object|null} snapshot latest usage snapshot
     * @param {string|null} daemonError why the daemon cannot be reached
     * @param {object} prefs panel preferences from GSettings
     */
    update(snapshot, daemonError, prefs) {
        const now = nowUnix();
        const label = Format.panelLabel(snapshot, prefs, now, daemonError);

        this._icon.visible = prefs.showIcon || (!prefs.showPercent && !prefs.showCountdown);
        this._label.text = label.text;
        this._label.visible = label.text !== '';
        for (const cls of STATE_CLASSES)
            this._label.remove_style_class_name(cls);
        this._label.add_style_class_name(`ccum-${label.state}`);
        this._gauge.visible = prefs.showBar && label.fraction !== null;
        this._gauge.update(label.fraction, label.state);

        if (this.menu.isOpen || !this._menuBuilt) {
            this._buildMenu(snapshot, daemonError, prefs, now);
            this._menuBuilt = true;
        }
    }

    _buildMenu(snapshot, daemonError, prefs, now) {
        this.menu.removeAll();

        if (daemonError) {
            this._addText('Usage daemon unavailable', 'ccum-error ccum-menu-heading');
            this._addText(daemonError, 'ccum-menu-detail');
        }

        const countDown = Boolean(snapshot?.usage_countdown);
        const enabled = (snapshot?.providers ?? []).filter(p => p.enabled);
        for (const provider of enabled) {
            let title = provider.display_name;
            const selected = provider.accounts.find(a => a.selected);
            if (selected && provider.accounts.length > 1)
                title += ` — ${selected.name}`;
            if (provider.status === 'stale')
                title += ' (stale)';
            this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem(title));

            for (const window of provider.usage?.windows ?? []) {
                this.menu.addMenuItem(new WindowItem(
                    Format.windowRow(window, countDown, now, prefs.warn, prefs.critical)));
            }
            const problem = Format.providerProblem(provider);
            if (problem)
                this._addText(problem, provider.error ? 'ccum-error' : 'ccum-menu-detail');

            const others = provider.accounts.filter(a => !a.selected);
            if (others.length > 0) {
                const submenu = new PopupMenu.PopupSubMenuMenuItem('Other accounts');
                for (const account of others) {
                    const figure = account.usage?.headline;
                    const shown = figure
                        ? `${Math.round(countDown ? figure.remaining_percent : figure.used_percent)}%`
                        : account.error?.message ?? 'No data';
                    const item = new PopupMenu.PopupMenuItem(`${account.name}   ${shown}`, {reactive: false});
                    submenu.menu.addMenuItem(item);
                }
                this.menu.addMenuItem(submenu);
            }
        }

        if (snapshot && enabled.length === 0)
            this._addText('No providers enabled. Open Settings to choose some.', 'ccum-menu-detail');

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        if (snapshot) {
            const status = snapshot.polling
                ? 'Refreshing…'
                : snapshot.fetched_at > 0
                    ? `Updated ${Format.ago(Math.max(0, now - snapshot.fetched_at))}`
                    : 'Not refreshed yet';
            this._addText(status, 'ccum-menu-detail');
        }
        this.menu.addAction(daemonError ? 'Retry' : 'Refresh now',
            () => this._extension.refresh());
        this.menu.addAction('Settings', () => this._extension.openPreferences());
    }

    _addText(text, styleClass) {
        const item = new PopupMenu.PopupMenuItem(text, {reactive: false, can_focus: false});
        item.label.add_style_class_name(styleClass);
        item.label.clutter_text.line_wrap = true;
        this.menu.addMenuItem(item);
    }
});

export default class UsageMonitorExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._snapshot = null;
        this._daemonError = null;
        this._addIndicator();
        this._settingsChangedId = this._settings.connect('changed', (_settings, key) => {
            if (key === 'panel-position' || key === 'panel-index') {
                this._indicator?.destroy();
                this._addIndicator();
            }
            this.render();
        });
        this._tickId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, TICK_SECONDS, () => {
            this.render();
            return GLib.SOURCE_CONTINUE;
        });
        this._connectDaemon();
        this.render();
    }

    disable() {
        if (this._tickId) {
            GLib.Source.remove(this._tickId);
            this._tickId = null;
        }
        this._disconnectDaemon();
        if (this._settingsChangedId) {
            this._settings.disconnect(this._settingsChangedId);
            this._settingsChangedId = null;
        }
        this._indicator?.destroy();
        this._indicator = null;
        this._settings = null;
        this._snapshot = null;
        this._daemonError = null;
    }

    render() {
        this._indicator?.update(this._snapshot, this._daemonError, this._prefs());
    }

    refresh() {
        if (!this._proxy) {
            this._disconnectDaemon();
            this._connectDaemon();
            return;
        }
        this._proxy.RefreshAsync()
            .then(() => this._fetch())
            .catch(error => this._setDaemonError(error));
    }

    _prefs() {
        const s = this._settings;
        return {
            provider: s.get_string('panel-provider'),
            window: s.get_string('panel-window'),
            showIcon: s.get_boolean('show-icon'),
            showPercent: s.get_boolean('show-percent'),
            showCountdown: s.get_boolean('show-countdown'),
            showBar: s.get_boolean('show-bar'),
            warn: s.get_int('warn-threshold'),
            critical: s.get_int('critical-threshold'),
        };
    }

    _addIndicator() {
        this._indicator = new UsageIndicator(this);
        Main.panel.addToStatusArea(
            this.uuid,
            this._indicator,
            this._settings.get_int('panel-index'),
            this._settings.get_string('panel-position'));
    }

    _connectDaemon() {
        const cancellable = new Gio.Cancellable();
        this._cancellable = cancellable;
        // eslint-disable-next-line no-new
        new MonitorProxy(Gio.DBus.session, BUS_NAME, OBJECT_PATH, (proxy, error) => {
            if (cancellable.is_cancelled())
                return;
            if (error) {
                this._setDaemonError(error);
                return;
            }
            this._proxy = proxy;
            this._signalId = proxy.connectSignal('UsageChanged',
                (_proxy, _sender, [json]) => this._apply(json));
            this._ownerId = proxy.connect('notify::g-name-owner', () => {
                if (proxy.g_name_owner)
                    this._fetch();
                else
                    this._setDaemonError(new Error('The usage daemon stopped. Choose Retry to start it again.'));
            });
            this._fetch();
        }, cancellable);
    }

    _disconnectDaemon() {
        this._cancellable?.cancel();
        this._cancellable = null;
        if (this._proxy) {
            if (this._signalId)
                this._proxy.disconnectSignal(this._signalId);
            if (this._ownerId)
                this._proxy.disconnect(this._ownerId);
        }
        this._signalId = null;
        this._ownerId = null;
        this._proxy = null;
    }

    _fetch() {
        const proxy = this._proxy;
        proxy?.GetUsageAsync()
            .then(([json]) => {
                if (proxy === this._proxy)
                    this._apply(json);
            })
            .catch(error => {
                if (proxy === this._proxy)
                    this._setDaemonError(error);
            });
    }

    _apply(json) {
        let snapshot;
        try {
            snapshot = JSON.parse(json);
        } catch (error) {
            this._setDaemonError(error);
            return;
        }
        if (snapshot.schema_version !== SCHEMA_VERSION) {
            this._setDaemonError(new Error(
                `Daemon speaks schema ${snapshot.schema_version}; this extension expects ${SCHEMA_VERSION}. Update both.`));
            return;
        }
        this._snapshot = snapshot;
        this._daemonError = null;
        this.render();
    }

    _setDaemonError(error) {
        if (!this._settings)
            return;
        let message = String(error?.message ?? error);
        if (error instanceof GLib.Error &&
            error.matches(Gio.DBusError, Gio.DBusError.SERVICE_UNKNOWN))
            message = 'The claude-code-usage-monitor daemon is not installed. Run the install script.';
        else if (error instanceof GLib.Error && Gio.DBusError.is_remote_error(error))
            message = message.replace(/^GDBus\.Error:[^:]+:\s*/, '');
        this._daemonError = message;
        this.render();
    }
}
