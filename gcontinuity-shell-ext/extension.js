// GContinuity GNOME Shell Extension
// Interface: com.gcontinuity.Transport  (Shell 45+, GJS ES modules)
//
// On enable():  subscribe to D-Bus signals, add panel indicator
// On disable(): remove indicator, disconnect all signal handlers

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as MessageTray from 'resource:///org/gnome/shell/ui/messageTray.js';

// ── D-Bus interface XML ───────────────────────────────────────────────────────

const TRANSPORT_IFACE = `
<node>
  <interface name="com.gcontinuity.Transport">
    <signal name="DeviceConnected">
      <arg type="s" name="device_id"/>
      <arg type="s" name="name"/>
      <arg type="s" name="addr"/>
    </signal>
    <signal name="DeviceDisconnected">
      <arg type="s" name="device_id"/>
    </signal>
    <signal name="PacketReceived">
      <arg type="s" name="device_id"/>
      <arg type="s" name="json"/>
    </signal>
    <method name="GetConnectedDevices">
      <arg type="a(ss)" direction="out"/>
    </method>
    <method name="SendPacket">
      <arg type="s" direction="in" name="device_id"/>
      <arg type="s" direction="in" name="json"/>
    </method>
  </interface>
</node>`;

const GContinuityProxy = Gio.DBusProxy.makeProxyWrapper(TRANSPORT_IFACE);

// ── Extension class ───────────────────────────────────────────────────────────

export default class GContinuityExtension extends Extension {
    /** @type {PanelMenu.Button|null} */
    _indicator = null;

    /** @type {St.Label|null} */
    _label = null;

    /** @type {GContinuityProxy|null} */
    _proxy = null;

    /** @type {number[]} Signal connection IDs for cleanup. */
    _signalIds = [];

    /** @type {string|null} Currently connected device_id (null if idle). */
    _connectedDeviceId = null;

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    enable() {
        // Build panel button with label.
        this._indicator = new PanelMenu.Button(0.0, 'GContinuity', false);

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        this._label = new St.Label({
            text: '🔴 GContinuity',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'system-status-icon',
        });
        box.add_child(this._label);
        this._indicator.add_child(box);

        // Popup menu with quick-disconnect option.
        const disconnectItem = new PopupMenu.PopupMenuItem('Disconnect');
        disconnectItem.connect('activate', () => this._disconnect());
        this._indicator.menu.addMenuItem(disconnectItem);

        Main.panel.addToStatusArea('gcontinuity', this._indicator);

        // Connect to D-Bus proxy (async, non-blocking).
        this._createProxy();
    }

    disable() {
        // Remove signal handlers before destroying the proxy.
        for (const id of this._signalIds) {
            if (this._proxy) this._proxy.disconnectSignal(id);
        }
        this._signalIds = [];
        this._proxy = null;

        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        this._label = null;
        this._connectedDeviceId = null;
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    _createProxy() {
        new GContinuityProxy(
            Gio.DBus.session,
            'com.gcontinuity.Daemon',
            '/com/gcontinuity/Transport',
            (proxy, error) => {
                if (error) {
                    console.log(`[GContinuity] D-Bus proxy error: ${error.message}`);
                    return;
                }
                this._proxy = proxy;
                this._connectSignals();
                this._queryInitialState();
            }
        );
    }

    _connectSignals() {
        if (!this._proxy) return;

        this._signalIds.push(
            this._proxy.connectSignal('DeviceConnected',
                (_proxy, _sender, [device_id, name, addr]) => {
                    this._onDeviceConnected(device_id, name, addr);
                }
            )
        );

        this._signalIds.push(
            this._proxy.connectSignal('DeviceDisconnected',
                (_proxy, _sender, [device_id]) => {
                    this._onDeviceDisconnected(device_id);
                }
            )
        );
    }

    _queryInitialState() {
        if (!this._proxy) return;
        try {
            // GetConnectedDevices is synchronous from the GJS proxy.
            const [devices] = this._proxy.GetConnectedDevicesSync();
            if (devices && devices.length > 0) {
                const [device_id, name] = devices[0];
                this._setConnected(device_id, name, '');
            }
        } catch (_e) {
            // Daemon not running — stay disconnected.
        }
    }

    _onDeviceConnected(device_id, name, addr) {
        console.log(`[GContinuity] Device connected: ${name} (${addr})`);
        this._setConnected(device_id, name, addr);
        this._notify('GContinuity', `${name} connected`);
    }

    _onDeviceDisconnected(device_id) {
        console.log(`[GContinuity] Device disconnected: ${device_id}`);
        if (this._connectedDeviceId === device_id) {
            this._connectedDeviceId = null;
            if (this._label) this._label.set_text('🔴 GContinuity');
        }
        this._notify('GContinuity', 'Device disconnected');
    }

    _setConnected(device_id, name, _addr) {
        this._connectedDeviceId = device_id;
        if (this._label) this._label.set_text(`🟢 ${name}`);
    }

    _disconnect() {
        if (!this._proxy || !this._connectedDeviceId) return;
        try {
            this._proxy.SendPacketSync(
                this._connectedDeviceId,
                JSON.stringify({type: 'disconnect'})
            );
        } catch (e) {
            console.log(`[GContinuity] Disconnect failed: ${e.message}`);
        }
    }

    /** Show a GNOME Shell notification toast. */
    _notify(title, body) {
        try {
            const source = MessageTray.getSystemSource();
            const notification = new MessageTray.Notification({source, title, body});
            source.addNotification(notification);
        } catch (_e) {
            Main.notify(title, body);
        }
    }
}
