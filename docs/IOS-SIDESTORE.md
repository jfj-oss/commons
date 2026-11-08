# Megatron on iPhone, through SideStore

This is the owner's private build: an **unsigned** `Megatron-ios.ipa` that SideStore signs with a free Apple ID and
re-signs every 7 days. There is no paid developer account, no App Store, and no push notifications.

The app is the same Megatron as on the computer: same account, same rooms, same end-to-end encryption. The relay is
the one in `app/src-tauri/default-relay.txt`; nothing needs to be entered for a new device or a recovery key.

## 1. What is needed

On the **Mac** (8 GB MacBook Air is fine):

- **Full Xcode** from the Mac App Store (free, about 10 GB). The Xcode *command line tools* are not enough. Open Xcode
  once, accept the licence, and let it install the iOS platform when it offers to
  (Settings → Platforms). Then make sure it is selected:

  ```sh
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  xcodebuild -version            # must print an Xcode version
  xcrun --sdk iphoneos --show-sdk-path   # must print a path
  ```

- Rust (https://rustup.rs). The build script adds the `aarch64-apple-ios` target itself.
- Either Node.js (`npm`, so the script can install the Tauri CLI into `tools/`) or the Tauri CLI already installed
  (`cargo install tauri-cli --version ^2`).

On the **Linux PC**: the usual SSH access to the Mac (`~/.ssh/macbook-air`, `user@100.114.187.94`). Nothing else is
installed on the Mac by hand.

On the **iPhone**: SideStore (step 3), and about 100 MB free. The first launch also needs the iPhone to reach the
relay (the Tailscale network, if that is how the relay is reached).

## 2. Build the .ipa

On the Linux PC, in the repository:

```sh
scripts/build-ios.sh
```

It checks that the Mac really has Xcode before copying anything, copies the source to `~/commons-build` on the Mac,
builds with code signing disabled (`CODE_SIGNING_ALLOWED=NO`, `tauri ios build --no-sign`), packs
`Payload/Megatron.app` into an `.ipa`, and copies it back to `dist/Megatron-ios.ipa`.

- The first build is slow: Rust compiles everything for the iPhone, and Xcode generates its project. Later builds
  reuse `app/src-tauri/gen/apple` and the cargo target directory.
- `scripts/build-ios.sh --fresh` deletes and regenerates the Xcode project (use it after changing `tauri.conf.json`,
  bundle icons, or after an Xcode update).
- `COMMONS_MAC_FULL=1` uses every core instead of two jobs at background priority. Leave it off on the 8 GB laptop.
- The build is **not signed**. That is deliberate. Do not add an Apple team or certificate to the project.

If the script stops at "full Xcode is not installed", nothing was copied or built; install Xcode as in step 1 and run
it again.

## 3. Install SideStore

SideStore is installed once, with a computer, and then refreshes itself and its apps on the phone. SideStore's
installation method changes with iOS releases, so follow the current guide at **https://sidestore.io** rather than an
old blog post. At the time of writing the shape of it is:

1. On the computer, generate a **pairing file** for the iPhone (the SideStore guide links the current tool; it is
   usually *jitterbugpair* for Windows/Linux or the SideStore-provided installer). Plug the iPhone in and trust the
   computer.
2. Install **SideStore** itself onto the iPhone using the current official method (AltServer, or the installer the
   SideStore guide provides). Sign in with the **free Apple ID** when asked — this is the account SideStore uses for
   all apps. The Apple ID password is only handled by SideStore on the phone, never by this build.
3. On the iPhone: Settings → General → VPN & Device Management → trust the developer certificate.
4. Install the **SideStore VPN profile** that the guide supplies (LocalDevVPN / StosVPN, depending on version) and
   import the pairing file into SideStore. The VPN must be **on** when SideStore refreshes apps. It is a local VPN —
   it does not send your traffic through a server.

Things to verify on the current guide, because they change: the pairing-file tool, whether iOS 17.4+ needs an extra
helper, whether SideStore wants AltServer or its own installer first, and the exact VPN app name. If the guide and
this document disagree, the guide wins.

## 4. Load Megatron

1. Get `dist/Megatron-ios.ipa` onto the iPhone in any way that puts a file in the Files app: AirDrop from the Mac,
   iCloud Drive, a USB transfer, or `scp` it to the Mac and AirDrop it.
2. Open **SideStore → My Apps** (or the **+** button), choose the `.ipa`, and tap **Install**. SideStore signs it with
   the free Apple ID.
3. The first launch may ask to trust the developer again (Settings → General → VPN & Device Management). Then open
   Megatron, unlock it with the passphrase (or pair the phone as a new device from an existing one — Devices →
   "Add a device", and approve the code on a device you already use), and it syncs the rooms it can decrypt.
4. The app keeps its data in its own sandbox. Re-signing or reinstalling the same bundle ID (`app.commons.chat`)
   keeps the account and history. **Deleting the app deletes the keys** — the recovery key is the only way back.

## 5. The 7-day refresh

Free Apple ID signatures last **7 days**. SideStore re-signs Megatron with the same Apple ID, so the app keeps working
and keeps its data.

- Open SideStore while the VPN is on, go to My Apps, and tap **Refresh All** before the 7 days run out. Do it every
  few days, or the day before it expires.
- SideStore can try to refresh in the background while the VPN is on, but iOS gives it little time, so a manual
  refresh is the reliable way.
- If the 7 days pass without a refresh, Megatron will not launch until SideStore refreshes it. Nothing is lost.
- SideStore itself also expires after 7 days. Its own guide explains how it refreshes itself (usually when it is
  opened with the VPN on); keep that working first, or nothing else can be refreshed.
- Free accounts allow **3 active sideloaded apps** and **10 App IDs per 7 days**. SideStore counts as one app. If there
  is an error about the app limit, remove another sideloaded app or wait out the 7 days.

## 6. What does not work on a free account (by design, not a bug)

- **No push notifications.** Megatron on the phone learns about messages by the same polling it uses on the desktop,
  and iOS suspends the app once it is in the background. Messages arrive when the app is open (or as soon as it is
  opened again). The unread count and sound only happen while the app is running.
- **No background calls.** There is no CallKit or VoIP push, so a call cannot ring or keep running with the app
  closed. A voice room works while Megatron is open in the foreground.
- **The app is suspended in the background.** Sync, sending, and the encrypted call stop when iOS suspends it. The
  core's outbox resumes when the app comes back.
- **No in-app updates.** The updater is turned off on iOS; SideStore updates the app by signing a newer `.ipa`. Build
  one with `scripts/build-ios.sh` and install it again in SideStore.
- **No game keys.** They read a computer's keyboard; the setting is hidden on the phone.
- Screen sharing depends on WKWebView support and is usually unavailable. Calls may refuse to start if the iOS web
  view cannot do encrypted insertable streams; Megatron never falls back to an unencrypted call.

## 7. Troubleshooting

- **"full Xcode is not installed"** — install Xcode and run `sudo xcode-select -s …` as in step 1. The script does not
  install it.
- **The build fails in `tauri ios init`** — run `scripts/build-ios.sh --fresh`. If it still fails, check
  `~/commons-build/ios-build.log` on the Mac.
- **SideStore says "Unable to install"** — usually the 3-app limit, an untrusted certificate, or the VPN/pairing file
  not set up. Re-read the current sidestore.io guide.
- **Megatron opens to the unlock screen every time** — expected: the passphrase is not stored. Pairing the phone as a
  device does not remove the passphrase.
- **Messages do not arrive while the app is closed** — expected on a free account (see section 6).
