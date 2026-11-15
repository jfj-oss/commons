#!/usr/bin/env python3
"""Measure mega-gpui or the patched Tauri build inside the private headless KWin (see SPIKE.md).

  measure.py <gpui|tauri> <start|idle|room|long|scroll200|scroll> [runs]

  start   exec -> first frame -> sign-in screen painted (process cold, files warm in the page cache)
  idle    unlocked, no room open: memory and CPU of the whole process tree after it settles
  room    the long room as it opens by default (its newest 200 messages): memory, CPU while nothing happens
  long    the 5000-message room open with all 5000 rows loaded: memory, and CPU while nothing happens
  scroll  that room (scroll200: the default 200), real wheel events for 5 s: frame intervals as the app saw them, and CPU meanwhile

Everything it starts it stops, by PID. It needs run-gpui.sh / run-tauri.sh, the relay and the seeded people.
"""
import json, os, subprocess, sys, time
D = os.path.expanduser('~/data/agent-tmp/mega-gpui')
HZ = os.sysconf('SC_CLK_TCK')

def tree(pid):
    kids, out = {}, []
    for p in os.listdir('/proc'):
        if p.isdigit():
            try: kids.setdefault(int(open(f'/proc/{p}/stat').read().rsplit(')', 1)[1].split()[1]), []).append(int(p))
            except OSError: pass
    todo = [pid]
    while todo:
        p = todo.pop(); out.append(p); todo += kids.get(p, [])
    return out

def mem(pid):
    rss = pss = 0; names = []
    for p in tree(pid):
        try:
            for line in open(f'/proc/{p}/smaps_rollup'):
                if line.startswith('Rss:'): rss += int(line.split()[1])
                if line.startswith('Pss:'): pss += int(line.split()[1])
            names.append(open(f'/proc/{p}/comm').read().strip())
        except OSError: pass
    return {'rss_mb': round(rss / 1024, 1), 'pss_mb': round(pss / 1024, 1), 'processes': names}

def cpu(pid):
    t = 0
    for p in tree(pid):
        try: f = open(f'/proc/{p}/stat').read().rsplit(')', 1)[1].split(); t += int(f[11]) + int(f[12])
        except OSError: pass
    return t / HZ

def marks():
    try: return [json.loads(l) for l in open(f'{D}/run/marks.jsonl') if l.strip()]
    except OSError: return []
def wait_mark(name, secs=120):
    t = time.time()
    while time.time() - t < secs:
        for m in marks():
            if m['mark'] == name: return m
        time.sleep(0.02)
    raise SystemExit(f'no mark {name} after {secs}s; marks: {[m["mark"] for m in marks()]}')

def launch(app, room, env):
    t0 = time.time() * 1000
    subprocess.run([f'{D}/run-{app}.sh', room] + [f'{k}={v}' for k, v in env.items()], check=True)
    return t0, int(open(f'{D}/run/app.pid').read())

def stop(pid):
    pids = tree(pid)
    for p in pids:
        try: os.kill(p, 15)
        except OSError: pass
    time.sleep(1.5)
    for p in pids:
        try: os.kill(p, 9)
        except OSError: pass

def cpu_over(pid, secs):
    a = cpu(pid); time.sleep(secs); return round((cpu(pid) - a) / secs * 100, 1)   # percent of one core

def one(app, what):
    if what == 'start':
        t0, pid = launch(app, '', {'MEGA_GPUI_PASS': ''})
        try:
            s = wait_mark('signin_painted'); m = {x['mark']: x['t'] for x in marks()}
            return {'exec_to_main_ms': round(m['main'] - t0), 'exec_to_first_frame_ms': round(m['first_frame'] - t0), 'exec_to_signin_painted_ms': round(s['t'] - t0)}
        finally: stop(pid)
    if what == 'idle':
        t0, pid = launch(app, '', {})
        try:
            wait_mark('unlocked'); time.sleep(12)
            return {**mem(pid), 'cpu_pct_of_one_core': cpu_over(pid, 10)}
        finally: stop(pid)
    scroll = what.startswith('scroll')
    env = {'MEGA_GPUI_SETTLE_MS': 16000 if scroll else 1000}
    if what in ('long', 'scroll'): env['MEGA_GPUI_LIMIT'] = 5000   # room / scroll200: the 200 newest, as both apps open a room by default
    if scroll and app == 'tauri': env['MEGA_GPUI_FRAMES'] = 6
    # KWin's fake wheel events carry no axis source, and GPUI's Wayland client waits for one (a real wheel or touchpad sends it),
    # so the GPUI list is moved the same distance per frame from inside: 310 frames up at 41 px, which is what the wheel
    # moved the web UI in its 200-row run, then 310 back down.
    if scroll and app == 'gpui': env['MEGA_GPUI_SCROLL'] = 620
    t0, pid = launch(app, 'long', env)
    try:
        a = wait_mark('room_open'); b = wait_mark('room_painted', 180); loaded = wait_mark('room_loaded')
        out = {'rows': loaded.get('rows'), 'open_to_painted_ms': round(b['t'] - a['t'])}
        if not scroll:
            time.sleep(10)
            return {**out, **mem(pid), 'cpu_pct_of_one_core': cpu_over(pid, 10)}
        if app == 'gpui':
            wait_mark('scroll_start', 60); c0, w0 = cpu(pid), time.time()
            f = wait_mark('scroll_done', 60)
            out.update({'scrolled_px': round(f.get('scrolled_px') or 0), 'frames': f['summary'], 'cpu_pct_of_one_core_while_scrolling': round((cpu(pid) - c0) / (time.time() - w0) * 100, 1), **mem(pid)})
            return out
        hl = dict(os.environ, WAYLAND_DISPLAY='wayland-megatest'); hl.pop('DISPLAY', None)
        fk = f'{D}/target/release/examples/fakekeys'
        subprocess.run([fk, 'move:900,520'], env=hl, check=True)
        wait_mark('frames_start', 60)
        c0, w0 = cpu(pid), time.time()
        # a wheel notch every 16 ms for 5 s, upward into the history
        subprocess.run([fk, 'sleep:300', f'wheel:{os.environ.get("WHEEL", "-15")}x310@16'], env=hl, check=True)
        f = wait_mark('frames_done', 60)
        out.update({'scrolled_px': round(f.get('scrolled_px') or 0), 'frames': f['summary'], 'cpu_pct_of_one_core_while_scrolling': round((cpu(pid) - c0) / (time.time() - w0) * 100, 1), **mem(pid)})
        return out
    finally: stop(pid)

if __name__ == '__main__':
    app, what = sys.argv[1], sys.argv[2]
    runs = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    res = [one(app, what) for _ in range(runs)]
    print(json.dumps({'app': app, 'what': what, 'runs': res}, indent=1))
    with open(f'{D}/results/{app}-{what}.json', 'w') as f: json.dump(res, f, indent=1)
