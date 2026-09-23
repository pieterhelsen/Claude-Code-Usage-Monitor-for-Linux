// D-Bus binding for the claude-code-usage-monitor daemon (see src/dbus.rs).
// Shared by the extension and its preferences window.

import Gio from 'gi://Gio';

export const BUS_NAME = 'io.github.pieterhelsen.ClaudeCodeUsageMonitor';
export const OBJECT_PATH = '/io/github/pieterhelsen/ClaudeCodeUsageMonitor';
export const SCHEMA_VERSION = 1;

const INTERFACE_XML = `
<node>
  <interface name="io.github.pieterhelsen.ClaudeCodeUsageMonitor1">
    <method name="GetUsage">
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="Refresh"/>
    <method name="GetSettings">
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="SetSettings">
      <arg type="s" direction="in" name="json"/>
      <arg type="s" direction="out" name="normalized"/>
    </method>
    <method name="Quit"/>
    <property name="Version" type="s" access="read"/>
    <signal name="UsageChanged">
      <arg type="s" name="json"/>
    </signal>
  </interface>
</node>`;

/**
 * Proxy constructor: `new MonitorProxy(Gio.DBus.session, BUS_NAME,
 * OBJECT_PATH, callback, cancellable)`. Without DO_NOT_AUTO_START, the first
 * call D-Bus-activates the daemon when its service file is installed.
 */
export const MonitorProxy = Gio.DBusProxy.makeProxyWrapper(INTERFACE_XML);

/** Settings keys the daemon uses to enable each provider. */
export const PROVIDER_SETTING_KEYS = {
    claude: 'show_claude_code',
    codex: 'show_codex',
    opencode: 'show_opencode',
    cursor: 'show_cursor',
    grok: 'show_grok',
};
