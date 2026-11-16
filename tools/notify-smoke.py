#!/usr/bin/env python3
"""A stub org.freedesktop.Notifications server for tools/notify-smoke.sh: logs every Notify call as a line of JSON and, a
moment later, answers it with a click (ActionInvoked "default"). Only ever run inside `dbus-run-session`."""
import json, sys
import dbus, dbus.service, dbus.mainloop.glib
from gi.repository import GLib

log = open(sys.argv[1], 'a', buffering=1)
dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName('org.freedesktop.Notifications', bus)

class Server(dbus.service.Object):
    n = 0
    @dbus.service.method('org.freedesktop.Notifications', in_signature='', out_signature='as')
    def GetCapabilities(self): return ['actions', 'body', 'body-markup']
    @dbus.service.method('org.freedesktop.Notifications', in_signature='susssasa{sv}i', out_signature='u')
    def Notify(self, app, rid, icon, summary, body, actions, hints, timeout):
        Server.n += 1; nid = Server.n
        log.write(json.dumps({'app': str(app), 'summary': str(summary), 'body': str(body), 'actions': [str(a) for a in actions], 'timeout': int(timeout), 'category': str(hints.get('category', ''))}) + '\n')
        GLib.timeout_add(700, lambda: (self.ActionInvoked(nid, 'default'), False)[1])
        return nid
    @dbus.service.method('org.freedesktop.Notifications', in_signature='u', out_signature='')
    def CloseNotification(self, nid): pass
    @dbus.service.method('org.freedesktop.Notifications', in_signature='', out_signature='ssss')
    def GetServerInformation(self): return ('stub', 'mega', '1', '1.2')
    @dbus.service.signal('org.freedesktop.Notifications', signature='us')
    def ActionInvoked(self, nid, key): pass

Server(bus, '/org/freedesktop/Notifications')
print('ready', flush=True)
GLib.MainLoop().run()
