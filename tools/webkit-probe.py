#!/usr/bin/env python3
# What does the Linux webview (WebKitGTK, the engine behind the Tauri desktop shell) offer for calls?
# Runs on GTK's Broadway backend (a virtual display served over HTTP): no window is ever shown on the desktop.
#   broadwayd :7 & GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 python3 tools/webkit-probe.py [default|enabled]
import sys, json, gi
gi.require_version('Gtk', '3.0'); gi.require_version('WebKit2', '4.1')
from gi.repository import Gtk, WebKit2, GLib
mode = sys.argv[1] if len(sys.argv) > 1 else 'default'
win = Gtk.OffscreenWindow() if False else Gtk.Window()
s = WebKit2.Settings()
view = None
props = [p.name for p in s.list_properties()]
if mode == 'enabled':
    for p in ('enable-media-stream', 'enable-mediasource', 'enable-webrtc', 'enable-mock-capture-devices', 'enable-webrtc-codecs'):
        if p in props and p != 'enable-mock-capture-devices': s.set_property(p, True)
view = WebKit2.WebView.new_with_settings(s)
print('webkit', WebKit2.get_major_version(), WebKit2.get_minor_version(), WebKit2.get_micro_version(), '| relevant settings:', {p: s.get_property(p) for p in props if any(k in p for k in ('media-stream', 'webrtc', 'mediasource', 'media-capabilities', 'mock'))})
PROBE = """(async () => { const r = {};
 r.secure = isSecureContext; r.RTCPeerConnection = typeof RTCPeerConnection; r.getUserMedia = !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia);
 r.getDisplayMedia = !!(navigator.mediaDevices && navigator.mediaDevices.getDisplayMedia);
 r.createEncodedStreams = typeof RTCRtpSender !== 'undefined' && typeof RTCRtpSender.prototype.createEncodedStreams;
 r.RTCRtpScriptTransform = typeof RTCRtpScriptTransform; r.Worker = typeof Worker; r.subtle = !!(crypto && crypto.subtle);
 r.setSinkId = 'setSinkId' in HTMLMediaElement.prototype; r.AudioContext = typeof AudioContext;
 try { const pc = new RTCPeerConnection(); r.canMakePC = true; pc.close(); } catch (e) { r.canMakePC = 'ERR ' + e.message; }
 try { r.devices = navigator.mediaDevices ? (await navigator.mediaDevices.enumerateDevices()).length : 'none'; } catch (e) { r.devices = 'ERR ' + e.message; }
 return JSON.stringify(r); })()"""
def done(webview, res):
    try:
        r = webview.run_javascript_finish(res)
        v = r.get_js_value().to_string()
    except Exception as e:
        v = 'ERR ' + str(e)
    print('probe', v); Gtk.main_quit()
def loaded(webview, ev):
    if ev == WebKit2.LoadEvent.FINISHED:
        # evaluate async function via a promise stored on window, then read it
        webview.run_javascript("window.__p = " + PROBE + ".then(x => { window.__r = x; })", None, lambda w, r: GLib.timeout_add(2500, read, w))
def read(w):
    w.run_javascript("window.__r || 'pending'", None, done); return False
view.connect('load-changed', loaded)
win.add(view); win.set_default_size(400, 300); win.show_all()
view.load_html("<html><body>probe</body></html>", "https://commons.test/")
GLib.timeout_add(20000, lambda: (print('timeout'), Gtk.main_quit()))
Gtk.main()
