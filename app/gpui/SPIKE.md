# mega on GPUI: the spike

Question: can mega's desktop app be a native GPUI app that links the core directly, and is it enough lighter and
faster than the Tauri app on Linux to be worth the port?

**Verdict: go.** The parts a chat app cannot do without all work (sign-in, rooms, a virtual message list with live
updates, a real multi-line composer with paste), the current look carries over, and it is far lighter where the
Tauri app hurts: about a third of the memory, a fraction of the idle CPU, a room that opens in tens of milliseconds
and scrolls without a missed frame. Startup time is the one place it is no better. What is not proven is listed
under "Not verified" and "What this does not show"; none of it is a known blocker, two items need a person at a real
keyboard for a minute each.

## GPUI version

- `gpui-kit = "=0.9.1"` from crates.io, which pins `gpui-pre = "=0.3.12"` (also `gpui-pre-platform`,
  `gpui-pre-linux`, `gpui-pre-wgpu`) and `gpui-component 0.9.1`. The workspace `Cargo.lock` holds every checksum.
- Why not `gpui` itself: the `gpui` crate on crates.io is 0.2.2 from October 2025 and has not followed Zed since.
  `gpui-pre` is a snapshot of Zed's GPUI (it says of zed@d2a4fb6) republished by Longbridge, the authors of
  gpui-component, so that their components build from crates.io. Pinning a Zed git revision directly would give
  the same GPUI without the middleman, but then the components would have to be pinned to a matching git revision
  too, and git dependencies make offline and reproducible builds worse.
- Why gpui-component at all: it supplies what the brief feared missing. Its `Textarea` is a real editor (IME through
  GPUI's input handler, clipboard, undo, selection, auto-grow, submit-on-Enter), and it has switches, buttons, menus,
  scrollbars, a colour picker, Markdown, resizable panels. Without it the composer alone would have been weeks.
- The cost of that choice: GPUI now reaches this project through one third party (crates owned by one person,
  `huacnlee`). If they stop publishing, the fallback is the Zed git revision named in the snapshot plus
  gpui-component from git; the app code would not change.
- `Cargo.lock`: adding the crate added about 300 packages and re-resolved three build-time ones the Tauri app's
  dependencies use (`proc-macro-crate` 2.0.2 -> 2.0.0, and with it `toml` 0.8.2 and `toml_datetime` 0.6.3 dropped for
  versions already in the file). No runtime crate of the released apps changed version, and the Tauri app builds and
  runs from this branch. If even that is unwelcome before a release, the alternative is to make `app/gpui` a
  workspace of its own with its own lock file.
- Rust: the existing toolchain (1.99.1) builds it. First build about 5 minutes with `-j 8`; an incremental build
  of the app about 35 s in release.
- Rendering on Linux is wgpu on Vulkan. There is no OpenGL fallback: a machine without a working Vulkan driver gets no
  window.

## What was built for the spike

- `app/gpui`, binary `mega-gpui`: in the workspace, not in `default-members`, in no release script.
- The core is linked directly. Every call is `commons_core::api::App::call(method, args)`, the same JSON API the
  Tauri shell's one `rpc` command and the browser bridge forward to. There are no events to subscribe to: the web UI
  polls `snapshot` and `messages` every 500 ms with the hash of what it holds, and the core answers "same" or a
  difference. This app does the same from one thread, and applies the differences to GPUI's virtual list.
- Sign-in: unlock an existing account, or create one. The account folder is `COMMONS_DATA` as in the Tauri app;
  without it the app opens the Tauri app's folder (`app.commons.chat`; the spike once used a folder of its own, changed
  when the native app became the real one). The core's lock keeps the two programs off one account at once. **Tests use
  throwaway `COMMONS_DATA` folders only.**
- Rooms, one room as a virtual list (GPUI `list`, bottom-aligned, only visible rows are laid out), live updates,
  typing, read marks, and the composer: multi-line, grows to 8 lines, Enter sends, Shift+Enter is a new line.
- Phase 1 (done after the go): the current look and the rest of the list in PORT-PLAN.md marked "done".

## How it was measured

- Machine: Ryzen 7 7800X3D, RTX 4090 (driver 620.70), CachyOS, kernel 7.3.4. This is a fast machine; see "What this
  does not show".
- Nothing was opened on the desktop. Both apps ran inside a private headless KWin (`kwin_wayland --virtual`, its own
  D-Bus session, config folder and socket, 1600x1000 at 60 Hz). Rendering was on the **real GPU** in both cases: KWin
  and `mega-gpui` appear in `nvidia-smi` on the 4090 (Vulkan), and WebKit's web process has the NVIDIA libraries
  mapped and is listed as a graphics client on it. Nothing was measured on llvmpipe or lavapipe.
- Data: a local relay (`commons-relay` on 127.0.0.1) with two throwaway accounts made by `examples/seed.rs`; a room
  with 5,000 messages of mixed length.
- The Tauri side is the 0.3.11 app built in release from this branch with `bench/tauri-bench.patch` applied in a
  separate worktree (two extra commands and a script that unlock, open the room and write time marks). The patch is
  not applied to this branch. Building it also showed the Tauri app still builds with the changed `Cargo.lock`.
- Input: real key, pointer and wheel events through KWin's fake-input protocol (`examples/fakekeys.rs`), which reach
  an app the way a keyboard and mouse do.
- Memory is the whole process tree from `/proc/*/smaps_rollup` (Tauri: the app, WebKitWebProcess,
  WebKitNetworkProcess). RSS counts shared pages once per process; PSS shares them out and is the fairer number for
  three processes. CPU is the tree's user+system time over 10 s, as a percentage of one core.
- Scripts: `bench/measure.py`, `bench/hl-start.sh`, `bench/run-*.sh`. Raw results were kept outside the repository.

## Numbers

Tauri 0.3.11 (WebKitGTK) against `mega-gpui` at the phase 1 commit, which draws the animated cards and room backdrop
too, so both sides are doing the same kind of work. `mega-gpui`: two runs per row. Tauri: two runs for idle and the
long room, one for the 200-row room and for each scroll row. Ranges are both runs.

| | Tauri 0.3.11 | mega-gpui | |
| --- | --- | --- | --- |
| Start: exec to the sign-in screen painted (5 runs, median, range) | 400 ms (390-419) | 399 ms (376-440) | the same |
| Idle, unlocked, home cards showing: RSS / PSS | 731-768 MB / 522-553 MB | 234-236 MB / 178-180 MB | about a third |
| the same: CPU | 16.4-16.8 % | 2.3-2.4 % | 7x less |
| A room open (newest 200 messages): RSS / PSS | 811 MB / 596 MB | 243 MB / 187 MB | about a third |
| the same: CPU, nothing happening | 3.8 % | 2.5 % | |
| the same: click to painted | 317 ms | 44-47 ms | 7x faster |
| Long room, all 5,000 rows loaded: RSS / PSS | 2,026-2,099 MB / 1,811-1,889 MB | 320-321 MB / 263-265 MB | a sixth |
| the same: CPU, nothing happening | 12.9-13.2 % | 6.7 % | |
| the same: open to painted | 6.7-6.8 s | 63-73 ms | about 100x |
| Scrolling 200 rows, 12,400-12,700 px in 5 s: frame interval p50 / p95 / p99 / max | 16 / 34 / 37 / 81 ms | 16.7 / 16.7 / 16.7 / 18.1 ms | |
| the same: frames longer than 20 ms | 37 of 330 (17 longer than 33 ms) | 0 of 620 | |
| the same: CPU while scrolling | 37 % | 8.3-8.4 % | |
| Scrolling with 5,000 rows loaded: frame interval mean / p95 / max | 136 / 426 / 505 ms (44 frames in 6 s) | 16.7 / 16.7 / 18.1 ms | |
| the same: CPU while scrolling | 150 % | 12.7-12.8 % | |

Before the look was added (phase 0: plain list, no animation) `mega-gpui` idled at 230 MB RSS and 0.7 % CPU. So the
animated ASCII fields cost about 1.6 % of a core on the home screen and almost no memory.

Binary: 50 MB unstripped (Tauri app 34 MB plus the system's WebKitGTK).

### Reading the numbers honestly

- **Startup is not a win** on this machine with files in the page cache. Both take 0.4 s. GPUI spends its time
  opening the Vulkan device and loading fonts; WebKit spends its own starting two helper processes. A real cold start
  (after a reboot) was not measured: dropping the page cache needs root. WebKitGTK is a far larger thing to read
  from disk, so a true cold start probably favours GPUI, but that is an expectation, not a measurement.
- **The scroll runs were not driven the same way**, and could not be. The web UI was scrolled with real wheel events.
  GPUI's Wayland client ignores wheel events that carry no axis source, which is what KWin's fake input sends (a real
  wheel or touchpad always sends one), so the GPUI list was moved from inside the app by 41 px per frame, the speed
  the wheel moved the web UI. The drawing work is the same; the input path was not exercised. Real wheel scrolling
  in GPUI is the path Zed uses every day, but **it was not seen working here**.
- Both frame clocks are capped by the 60 Hz virtual output, so "16.7 ms" means "never missed a frame", not "takes
  16.7 ms". The CPU column is the better measure of cost: 8 % of one core at 60 frames a second is about 1.4 ms of
  CPU per frame for the whole process, core included. The web UI's clock is `requestAnimationFrame` with 1 ms
  resolution.
- **5,000 rows is a stress case, not how the web UI opens a room.** It loads 200 and adds 200 on demand; the fair
  everyday comparison is the 200-row rows. The 5,000-row rows show what a virtual list buys: a room's cost stops
  depending on its length.
- `mega-gpui` does less than the Tauri app (no pictures, no wall, no call code, fewer controls). Memory will grow as
  features land: decoded images and video frames above all. The gap at 200 rows (596 MB against 187 MB PSS) is
  mostly the web engine itself and will mostly stay.
- Small samples: two runs per row, one machine.

### Found on the way

- With 5,000 rows loaded the first build used **745 MB**, not 320. The half-second poll ran on GPUI's thread pool,
  and glibc keeps a separate heap per thread: each thread that ever built the big JSON answer kept its high-water
  mark. Running the poll on one thread fixed it. The Tauri shell runs every `rpc` through `spawn_blocking`, the same
  pattern; its own process may be carrying the same waste.
- The core rebuilds and hashes a room's whole window on every poll. With 5,000 rows that is the 6.7 % idle CPU in
  the table (and part of Tauri's 13 %). A change counter in the core would make it free for both UIs. Not done here:
  the brief is not to change the core's behaviour.
- `ListState::scroll_by` in this GPUI starts from the wrong place when a bottom-aligned list rests at its end (it
  takes the end as the top). The wheel handler does not have the problem. Only the measurement used `scroll_by`; it
  works around it.

## The composer: what was checked

The go/no-go hinged on text input, so this was checked with real key events through the compositor, not by calling
the widget:

| Check | Result |
| --- | --- |
| Typing, including Shift for capitals | works |
| Shift+Enter inserts a new line; the box grows | works |
| Enter sends and clears the box | works |
| Ctrl+A, Ctrl+C, Ctrl+V through the real Wayland clipboard | works |
| Non-ASCII text in messages (CJK, emoji) displays | works |
| A message arriving while the list is at the bottom keeps it at the bottom | works |
| The typing line appears and goes | works |

## Not verified

- **IME composition** (fcitx5 / ibus, dead keys, the compose key). The input implements GPUI's input handler, which is
  how Zed takes IME input, and gpui-component ships composition tests, but no input method was running in the
  headless compositor, so no composed character was ever typed here. **A person needs to type a few characters with
  their input method before this is called done.** This is the one go/no-go item resting on trust.
- **Real wheel and touchpad scrolling** (see above), and scrolling feel in general: nobody has had a hand on it.
- macOS: not built.
- X11: GPUI supports it; not run.
- Fractional scaling and HiDPI: the virtual output was 1x.
- Screen readers.

## What this does not show

- That it is better on a slow or integrated-GPU laptop. The reason for the port is that WebKitGTK is slow and heavy
  on Linux; this machine hides slowness on both sides. The memory numbers will carry over; the frame numbers should
  be re-run on the weakest machine that matters.
- How it behaves on a machine with no or a broken Vulkan driver (it will not start).
- Anything about calls. See PORT-PLAN.md section 9.

## Why go, in short

1. Nothing required turned out to be missing: the text input exists and works; the virtual list works with the core's
   differences as they are; fonts, colour emoji and CJK render.
2. The core needed no change. The same `App::call` API serves both UIs, so iOS and the browser edition are untouched.
3. Memory: a third at rest, a sixth with a long room. Idle CPU: a seventh on the animated home screen.
4. The look carries over more cheaply than feared: the ASCII fields are glyph sprites from GPUI's atlas, about 1.6 % of
   a core at 20 frames a second.
5. It took one session to reach a usable chat window, which says the framework is workable for this project's size.

Reasons for caution, none of which change the verdict: the IME check above; one third party between this project
and GPUI; and the size of what is left (PORT-PLAN.md), calls most of all.

## Run it

Build (heavy; the first build compiles several hundred crates, about 5 minutes at `-j 8`):

    cargo build --release -p mega-gpui

Try it on throwaway data, without touching a real account:

    target/debug/commons-relay --bind 127.0.0.1:18941 --data /some/scratch/relay &
    cargo run --release -p mega-gpui --example seed -- setup http://127.0.0.1:18941 /some/scratch/people 500
    COMMONS_DATA=/some/scratch/people/alice COMMONS_RELAY=http://127.0.0.1:18941 target/release/mega-gpui
    # passphrase: throwaway-pass-1

Environment:

| Variable | Meaning |
| --- | --- |
| `COMMONS_DATA`, `COMMONS_RELAY` | as in the Tauri app |
| `COMMONS_DATA=<dir>` | use this account folder instead of the Tauri app's `app.commons.chat` (tests, a second profile) |
| `MEGA_GPUI_NOTIFY_FILE=<file>` | write notifications to a file instead of showing them |
| `MEGA_GPUI_PASS`, `MEGA_GPUI_OPEN`, `MEGA_GPUI_LIMIT`, `MEGA_GPUI_SCROLL`, `MEGA_GPUI_FRAMES`, `MEGA_BENCH_OUT` | measurement only (bench.rs). `MEGA_GPUI_PASS` puts a passphrase in the environment: throwaway accounts only |

Headless, as the measurements were made: `bench/hl-start.sh` starts the private KWin, `bench/run-gpui.sh <room>` starts
the app inside it, `bench/hl-shot.py out.png` takes a picture, `examples/fakekeys.rs` sends input,
`bench/measure.py gpui|tauri start|idle|room|long|scroll200|scroll` measures. The scripts assume the scratch layout
under `~/data/agent-tmp/mega-gpui`; they are kept for the record, not as a product.
