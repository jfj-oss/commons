#!/usr/bin/env python3
"""A mock org.freedesktop.Notifications service for tests (never the user's own). Run it only inside its own private bus:
    notifmock.py <log file> [caps]       caps: "reply" (default: with inline-reply, as KDE) or "plain" (no inline reply)
Every Notify call is one line of JSON in the log. Commands arrive on stdin, one per line:
    reply <id> <text...>     the person typed a quick reply into the note       (signal NotificationReplied)
    action <id> <key>        a click on the note or one of its buttons          (signal ActionInvoked)
It prints "ready" once it owns the name."""
import json, sys, os
import dbus, dbus.service, dbus.mainloop.glib
from gi.repository import GLib

log = open(sys.argv[1], 'a', buffering=1)
caps = ['actions', 'body', 'body-markup'] + (['inline-reply'] if (sys.argv[2] if len(sys.argv) > 2 else 'reply') == 'reply' else [])
dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName('org.freedesktop.Notifications', bus)

class Server(dbus.service.Object):
    n = 100
    @dbus.service.method('org.freedesktop.Notifications', in_signature='', out_signature='as')
    def GetCapabilities(self): return caps
    @dbus.service.method('org.freedesktop.Notifications', in_signature='susssasa{sv}i', out_signature='u')
    def Notify(self, app, rid, icon, summary, body, actions, hints, timeout):
        if int(rid) == 0:
            Server.n += 1; nid = Server.n
        else:
            nid = int(rid)
        log.write(json.dumps({'id': nid, 'replaces': int(rid), 'app': str(app), 'summary': str(summary), 'body': str(body), 'actions': [str(a) for a in actions],
                              'category': str(hints.get('category', '')), 'hints': sorted(str(k) for k in hints.keys()), 'timeout': int(timeout)}) + '\n')
        return dbus.UInt32(nid)
    @dbus.service.method('org.freedesktop.Notifications', in_signature='u', out_signature='')
    def CloseNotification(self, nid): pass
    @dbus.service.method('org.freedesktop.Notifications', in_signature='', out_signature='ssss')
    def GetServerInformation(self): return ('mock', 'mega', '1', '1.2')
    @dbus.service.signal('org.freedesktop.Notifications', signature='us')
    def ActionInvoked(self, nid, key): pass
    @dbus.service.signal('org.freedesktop.Notifications', signature='us')
    def NotificationReplied(self, nid, text): pass

srv = Server(bus, '/org/freedesktop/Notifications')

def on_line(fd, cond):
    line = os.read(fd, 65536).decode().strip() if cond & GLib.IO_IN else ''
    if not line and cond & (GLib.IO_HUP | GLib.IO_ERR): loop.quit(); return False
    for l in line.splitlines():
        parts = l.split(' ', 2)
        if parts[0] == 'reply' and len(parts) == 3: srv.NotificationReplied(dbus.UInt32(int(parts[1])), parts[2])
        elif parts[0] == 'action' and len(parts) >= 2: srv.ActionInvoked(dbus.UInt32(int(parts[1])), parts[2] if len(parts) > 2 else 'default')
    return True

loop = GLib.MainLoop()
GLib.io_add_watch(0, GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR, on_line)
print('ready', flush=True)
loop.run()
