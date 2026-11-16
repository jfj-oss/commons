# Updates

The app updates itself from the relay the account already uses, with Tauri's official updater plugin and its signature
check. Nothing else is involved: no update service, no GitHub. The person is always asked first, and never during a call.

## Publishing a version (the host)

```sh
scripts/publish-update.sh                      # next patch version: 0.2.0 -> 0.2.1
scripts/publish-update.sh minor --notes "Faster sending, and pictures in replies."
scripts/publish-update.sh 0.3.0 --min-version 0.2.0   # also tell apps older than 0.2.0 that they must update
```

It bumps the version (tauri.conf.json and app/src-tauri/Cargo.toml), builds Linux (Ubuntu 22.04 in Docker), Windows
(cross-built in Docker; the NSIS installer) and macOS (on the Mac over ssh, two jobs at background priority, only when it
is plugged in or above 40%; `--skip-mac` goes on without it) — and each system's browser version alongside its desktop
app — signs the files here with the updater key, and puts them in `dist/`:

| In dist/ | Served at | For |
| --- | --- | --- |
| `Megatron-linux-x86_64.AppImage`, `Megatron-windows-x64-setup.exe`, `Megatron-macos-arm64.dmg` | `/download/…`, listed on `/join` | friends installing |
| `Megatron-linux-browser.tar.gz`, `Megatron-windows-browser.zip`, `Megatron-macos-browser.zip` | `/download/…`, listed on `/join` | friends who want the browser version |
| `update/latest.json` | `/update/latest.json` | the apps: version, notes, and per system a file address and its signature |
| `update/Commons_<version>_…` | `/update/…` | the files the apps install (this version and the one before) |

The relay reads `dist/` as it goes: publishing needs no restart. The previous downloads are kept in `dist/.previous/`
(not served). Commit the version bump afterwards; the script prints the command.

The manifest's file addresses are relative (`update/Commons_0.2.1_x86_64.AppImage`). Each app resolves them against the
relay address its account uses, so the same manifest works however friends reach the relay.

## What the app checks

1. **Where:** the manifest only from `<relay>/update/latest.json` of the account's own relay, over https (plain http only
   for a relay on this computer), redirects not followed. Every file address in it must lead into that same relay, or
   the whole manifest is refused before anything is downloaded (`crates/core/src/update.rs`).
2. **The file:** downloaded by the plugin (redirects off) and checked against the minisign public key built into the app
   (`plugins.updater.pubkey` in tauri.conf.json). A changed byte or another key: refused, nothing installed.
3. **The version:** the signature carries the version it was made for (`tauri signer sign --app-version`), and the app
   requires it to equal the version the manifest announces (`requireSignedVersion`). A genuine old file cannot be passed
   off as a new version. Only a version newer than the running one is offered: no downgrades.
4. **The person:** "Update available: version X. Install now?" with **Later**. Checked 15 seconds after unlocking and every
   4 hours. "Later" waits 4 hours (1 hour if the server says this version is too old).
5. **Calls:** while a call runs on this device (in the window, or handed off to the browser: the core sees its key polls
   and heartbeats, `App::call_active`), nothing is offered and nothing is installed. An install already downloaded waits,
   and once the call has ended the app asks again: "Restart now?"

`dangerousInsecureTransportProtocol` is on in tauri.conf.json only so that the plugin accepts a loopback relay for the
tests; the https rule above is enforced by our own code before the plugin is called.

How it installs: Linux replaces the AppImage file in place (keep it where it was first put, writable). Windows runs the
new installer (per user, no administrator rights) and the app restarts. macOS swaps the `Megatron.app` bundle (works when
it was dragged to Applications; not from inside the dmg). The app then restarts, and asks for the passphrase as after any start.

## The browser versions (Linux, Windows, macOS)

Each browser version is the core as a local bridge plus `app/ui`, opened in the person's own browser as an app window:
`Megatron-linux-browser.tar.gz` (Brave/Chrome/Chromium/Edge; `packaging/linux-browser`),
`Megatron-windows-browser.zip` (Edge/Brave/Chrome; `packaging/windows-browser`) and
`Megatron-macos-browser.zip` (`Megatron.app`; Brave/Chrome/Edge, or Safari as a normal tab;
`packaging/macos-browser`). All are published and signed by the same script, each in the same manifest as its own
platform:

| System | Manifest platform key | Update file |
| --- | --- | --- |
| Linux x86_64 | `linux-browser-x86_64` | `Megatron-linux-browser.tar.gz` |
| Windows x86_64 | `windows-browser-x86_64` | `Megatron-windows-browser.tar.gz` |
| macOS Apple silicon | `darwin-browser-aarch64` | `Megatron-macos-browser.tar.gz` |

No Tauri updater target has any of those names (desktop apps look for `linux-x86_64`, `windows-x86_64`,
`darwin-aarch64`), so the desktop apps ignore them, and each browser bridge looks only for its own.

The launcher runs `bridge --update <relay> <install-dir>` before it starts the page, with a five-second cap: offline, or
a relay that does not answer, only delays the window a moment. On Linux this is the shell script
`packaging/linux-browser/megatron`; on Windows and macOS it is `megatron_launcher`, a small Rust program (no console
window on Windows, and the `Megatron.app` executable on macOS). The bridge:

1. fetches `<relay>/update/latest.json` and localizes it exactly as the desktop app does (`crates/core/src/update.rs`:
   the account's relay only, https or loopback http, no redirects);
2. takes this system's entry, downloads it without following redirects, with timeouts and a size cap;
3. verifies the tarball with the minisign public key embedded in the bridge at build time — the same key as the desktop
   app — and requires the signature's trusted comment to name exactly the manifest's version; an unsigned, wrongly
   signed, or legacy (non-pre-hashed) file is refused;
4. installs only a strictly newer version (a downgrade is refused);
5. unpacks into a temporary folder next to the install (plain files and directories only, no symlinks or `..`), then
   renames `ui/` and the bridge (and the launcher, where the package carries one) into place. The previous copy stays in
   `.update-previous` until the new bridge has bound its port; if the new bridge does not start, the launcher puts the
   previous bridge and `ui/` back.

Windows cannot overwrite a running `.exe`, so the update runs as a child process of the running `bridge.exe`: the tarball's
`bridge.exe` replaces the file by renaming the running image aside (Windows permits renaming a running executable) and
moving the new one into place. The launcher does the same for itself. The portable Windows download is a zip that
`install.cmd` copies to `%LOCALAPPDATA%\Megatron-browser` and points a Start-menu shortcut at; the macOS download is a
`Megatron.app` signed ad-hoc (right-click, Open the first time). Account data lives in
`%LOCALAPPDATA%\Megatron-browser\data` and `~/Library/Application Support/Megatron-browser`, apart from the desktop app.

The page shows a small note from that check: "Updated to x.y.z" after an install, or "Update available" and the reason
if a newer version exists but could not be installed. The bridge can run for days, so the page also asks it (15 s after
unlocking, then every 4 hours, or from Settings, "Check for updates") and shows an **Update now** button. That runs the
same checks with no time limit, refuses during a call, then the bridge replaces itself and the page reloads; only an
installed copy with an embedded key can do this. `crates/core/src/browser_update.rs` has the code and its tests; `tools/browser-update-test.sh` exercises the
Linux launcher end to end with a throwaway key and a loopback relay.

## The per-start key, and updating across it

Since the bridge gained a per-start key (docs/THREAT-MODEL.md, "The browser edition"), an update has to cross three kinds of install
without locking anyone out. What each gets:

| Install | After the update | Protection |
| --- | --- | --- |
| Linux, standard launcher script in the install folder | The new bridge replaces an older `megatron` script (kept as `megatron.old`); the next start with it asks for a key, uses a browser profile of its own, and replaces the open-mode bridge | Full, from the second start after the update. The first start after the update still runs the old launcher: open mode, with a plain notice in the page |
| Windows | The update swaps `launcher.exe` in the same transaction as the bridge; the next start restarts an open-mode bridge with a key | Full, from the next start. The window of the start that installed the update is open mode, with the notice |
| macOS | The update tarball carries the new launcher inside `ui/` as `.launcher-macos` (an older bridge, which installs the update, has no place for a top-level `launcher` in `Contents/Resources` and would refuse the update). The new bridge, at its next start and before it binds the port, writes it next to `Contents/MacOS/launcher`, renames it over (the old one is kept as `launcher.previous` in the data folder), moves the carrier out of the sealed `Resources`, signs the app again ad hoc and checks it with `codesign --verify`. If signing fails the old launcher and carrier are put back and that launcher is not tried again. Everything is logged in the data folder's `launcher.log`. The next start restarts an open-mode bridge with a key | Full, from the next start. The start that installed the update is open mode, with the notice "close mega and open it again" |
| A launcher the update cannot replace (your own script, a systemd unit that starts `bridge 7420 ui`) | Keeps working unchanged: a bridge started without `MEGATRON_SECRET` is open to Host/Origin checks (and, on Linux, the same-user check) only. The page shows a notice. To get the full protection, start the bridge with `MEGATRON_SECRET` set to a random 32+ character string and open the window at `/#s=<that string>` | Host/Origin plus same-user on Linux; nothing against another program of the same user |
| A new launcher with a bridge that ignores the key (the previous bridge restored after a failed update) | Opens at the plain address, warns once | As the line above |

A launcher that finds something it cannot identify on its port refuses to open it (it never kills a program that is not this install's
own bridge). The browser profile of the new launcher is separate from the everyday one: nothing of the account is lost (the account
lives in the data folder), but the window's own preferences (theme, call device choices, Jellyfin address) start fresh once and the
microphone permission is asked again. The old profile is left untouched. `tools/browser-update-test.sh` and `tools/launcher-test.sh`
cover these cases.

## Window or tab

By default the browser versions open mega as an app window with a browser profile of its own. In Settings (browser versions only,
installed copies) **Open mega as** can be changed to **A tab in my browser** for this device. The choice is the file `open-as`
(`tab` or `window`) in the data folder, written by the bridge (`POST /open-as?v=tab`, protected by the secret like `/rpc`); the
environment variable `MEGATRON_OPEN=tab|window` overrides it, and all three launchers read it. It applies the next time mega is opened
from its icon. A tab starts the browser with only the address (no `--app`, no profile folder), so it lands in the everyday profile: on
macOS `open -a "Brave Browser"` when Brave is installed, else the default browser; on Windows and Linux the system's default
browser (whatever the person chose, Firefox included), and only if that cannot be started the first of Brave, Chrome, Chromium,
Edge found (an already-running browser opens a new tab).

A tab's address is not the same as a window's. A normal browser keeps every address it visits, so the launcher does not put the secret
in it: it writes a one-time ticket file (`.open-ticket-<port>`, mode 600, next to the secret file) and opens `/#t=<ticket>`. The page
removes the ticket from the address bar at once and posts it to `/session`; the bridge checks it, deletes the file (so it works once,
and for two minutes at most) and answers with the secret, which the page keeps in `sessionStorage` for that tab. Whatever the history
keeps is a spent ticket. A tab you close needs mega to be opened from its icon again.

## The signing key

`~/.config/commons/updater.key` (the private key, encrypted with its password), `~/.config/commons/updater.key.password`
(both mode 600), `~/.config/commons/updater.key.pub` (the public key, also in tauri.conf.json). The private key and its
password are never committed, printed, or copied to the Mac or into a build container: only finished files are signed,
on this PC.

**Back it up now, and after nothing else.** Every app out there trusts this one key. If it is lost, no installed app
will ever accept an update again: every friend would have to download and install by hand, once, an app with a new key.
If it leaks, whoever has it (and can get a manifest onto your relay) can push an app to your friends.

To back it up, copy all three files to two places that are not this PC, for example an encrypted USB stick and your
password manager:

```sh
mkdir -p /run/media/$USER/STICK/commons-updater-key
cp -p ~/.config/commons/updater.key ~/.config/commons/updater.key.password ~/.config/commons/updater.key.pub /run/media/$USER/STICK/commons-updater-key/
sha256sum ~/.config/commons/updater.key* /run/media/$USER/STICK/commons-updater-key/*    # the pairs must match
```

In Vaultwarden: a secure note "Megatron updater key" with the contents of `updater.key` and `updater.key.password` as two
hidden fields (they are short text). To restore: put the files back in `~/.config/commons/`, `chmod 600` the key and the
password file. `publish-update.sh` refuses to sign with a key whose id is not the one in tauri.conf.json, so a mix-up
shows at once.

If the key is ever lost: make a new pair (`tools/node_modules/.bin/tauri signer generate -w ~/.config/commons/updater.key`),
put the new `.pub` into tauri.conf.json, publish, and ask friends to download the app from `/join` once more.

## Compatibility: an update must never split a community

Friends update at different times, so every version has to work with older apps and with an older relay. What carries
this, and the rules for changes:

* **Relay API** (`/v1/...`, crates/proto): unchanged by this release. A new app uses nothing an older relay lacks except
  `/update/`, which an older relay answers with 404, read as "no updates published". Rule: never change or remove a `/v1`
  route or field; add new ones as optional, and keep old ones working. A breaking change gets `/v2` beside `/v1`.
* **Log entries** (one tag byte, crates/proto): an unknown tag is skipped by every version so far. New kinds of entry
  take a new tag.
* **Messages inside MLS** (`Envelope`, crates/core/src/envelope.rs): JSON with a kind `t`. Unknown fields are ignored by
  every version. From 0.2.0 an unknown kind is skipped with a note in the activity log ("someone has a newer
  version"); 0.1.0 shows it as a message that could not be read, but carries on. Rule: new fields are optional
  (`#[serde(default)]`); a new kind of message must be something older apps can do without.
* **The community table** (`GroupExt`, MLS group context): every member's app checks every change to it against the
  roles. An older admin who rewrites it would drop fields it does not know. Rule: a new field in the table needs
  `--min-version` set to the version that understands it, and should be optional until everyone has it.
* **MLS** (OpenMLS 0.9, one ciphersuite) and the request signatures: unchanged. Changing either is a breaking change
  and needs a new version of the protocol run beside the old one.
* **Minimum version:** `commons-relay --min-app-version 0.2.0` (or `COMMONS_MIN_APP_VERSION`), or
  `publish-update.sh --min-version`, puts `minimum_version` in the manifest (the relay's setting wins). Apps below it
  show a bar, "This server asks everyone to use Megatron X or newer", and the update offer comes back after an hour
  instead of four. Nothing is refused: it is a request, so a stale setting cannot lock anyone out. Apps from before 0.2.0
  have no updater at all, and cannot be told: they need one manual install from `/join`.

`tools/compat-e2e.mjs` runs an old and a new client against each other (and an old relay) and checks that they still
chat in both directions.

## Tests

```sh
cargo test -p commons-core --lib update        # manifest rules: same server only, no redirects, versions
cargo test -p commons-relay site_tests         # /update/ serving, the minimum version, nothing extra on the download page
systemd-run --user --scope -q -p MemoryMax=6G node tools/update-e2e.mjs ~/data/agent-tmp/commons-update/e2e
```

update-e2e needs, in its work folder, two throwaway key pairs and version N built with the first one:

```sh
W=~/data/agent-tmp/commons-update/e2e; mkdir -p $W/keys
for k in test wrong; do tools/node_modules/.bin/tauri signer generate --ci -w $W/keys/$k.key >/dev/null; : > $W/keys/$k.key.password; chmod 600 $W/keys/$k.key*; done
COMMONS_OUT=$W/n COMMONS_TAURI_CONFIG="$(jq -cn --arg k "$(cat $W/keys/test.key.pub)" '{version:"0.1.901",plugins:{updater:{pubkey:$k}}}')" scripts/build-appimage-portable.sh
```

## The native app (GPUI)

The native desktop app (`app/gpui`, `mega-gpui`) updates itself from the same feed, with the same key and the same rules,
but through its own entry and its own code (`app/gpui/src/update.rs`, the window's side in `updateui.rs`). The other apps
never see it.

| System | Feed platform key | File (in `dist/update/`) |
| --- | --- | --- |
| Linux x86_64 | `linux-gpui-x86_64` | `Megatron_<version>_gpui-x86_64`, the `mega-gpui` executable |
| macOS Apple silicon | `darwin-gpui-aarch64` | `Megatron_<version>_gpui-aarch64.app.tar.gz`, one `Megatron.app` |

(Linux aarch64 and macOS Intel have keys too, `linux-gpui-aarch64` and `darwin-gpui-x86_64`, for when those are built.
Windows has no native build yet and does not check.)

**Checks.** The feed is read 15 seconds after unlocking and then every 4 hours, from the account's own relay by the same
code as the Tauri app (`commons_core::update::fetch_manifest`: https or loopback http, no redirects, every address inside
that relay). Nothing is read, offered or restarted during a call (`CallUi::in_call`, and the core's `call_active` for a
call handed to the browser). The version the app compares is the one it was built as (`MEGA_GPUI_VERSION` at build time).

**Signature.** The download is checked against the minisign public key of the Tauri updater: `update.rs` reads
`plugins.updater.pubkey` from `app/src-tauri/tauri.conf.json` when it is built, so there is no second key to keep. The
signature must also have been made for exactly the version the feed announces (the `version:` in its trusted comment), so
a genuine old file cannot be passed off as a new one, and a file that fails any of it is dropped before a byte of it is
written next to the app. Nothing unverified is ever run. (`MEGA_GPUI_UPDATE_PUBKEY` at build time swaps the key, for the
tests' throwaway key; it is not read at run time.)

**Installing.**

- Linux: the new executable is written next to the running one (`.mega-gpui.update-new`, synced, mode 755), the running
  one is kept as `mega-gpui.previous` (a hard link, so nothing moves), and one `rename` puts the new file in place. The
  running process keeps its old file open and goes on unharmed. The folder has to be writable.
- macOS: the `.app.tar.gz` is unpacked next to `Megatron.app` (files, folders and links that stay inside the app; any
  other entry refuses the whole update), sealed again ad hoc (`codesign --force --deep --sign -`, then `--verify`; the
  build is not Developer ID signed, so it is the same right-click, Open the first time as the other macOS downloads), and
  swapped for the old bundle with two renames. The old one is kept as `Megatron.previous.app`. Not from inside the dmg.

The person is then told the version is installed and offered **Restart now**. "Restart" waits for the end of a call and
is never done by itself; otherwise the new version simply runs the next time mega is opened.

**Rollback.** An installed version is counted at each start until it has stayed up 20 seconds. Three starts in a row that
never did: the previous one is put back (one rename of the `.previous` copy), started in its place, and Home
says once that the new version did not start well. The state is `gpui-update.json` beside `gpui-prefs.json`.

**What's new.** After an install, the feed's `notes` (the text `publish-update.sh --notes` put there) are kept in that file;
the first launch of the new version shows them as a card on Home, with Dismiss. They are shown only if the running
version is the one the notes are for, and only once.

**Publishing the native build.**

```sh
scripts/publish-update.sh minor --notes "..."      # the Tauri apps first: this makes the version and the notes
MEGA_GPUI_VERSION=0.4.0 cargo build --release -p mega-gpui     # on the oldest system you support
scripts/publish-gpui.sh --version 0.4.0 --binary target/release/mega-gpui --dry-run   # look first
scripts/publish-gpui.sh --version 0.4.0 --binary target/release/mega-gpui              # then for real
# on the Mac: scripts/publish-gpui.sh --version 0.4.0 --app path/to/Megatron.app
```

`publish-gpui.sh` refuses unless `latest.json` already announces exactly that version (a feed holding only a native
entry would take the other apps' updates away), signs with the same updater key as `publish-update.sh`, checks the
signature carries the key the apps trust, copies the file, and adds only its own platform entry, last. `--dry-run` signs
and copies nothing and does not read the key.

**Tests.** `cargo test -p mega-gpui --bin mega-gpui update::` (version order, platform keys, entry choice, signature
accept and reject with a throwaway key, atomic replace and rollback of a binary and of a bundle, hostile archives, state
and start counting). `tools/gpui-update-test.sh` adds the end-to-end run against a private relay (`COMMONS_TEST_PORT_OFFSET`
1400+): `publish-gpui.sh` with a throwaway key into a scratch `dist/`, a local `commons-relay` serving it, and the check,
download, verification and install against it.
