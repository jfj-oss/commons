# Megatron: design

Private group chat for a handful of friends. Servers and channels like the apps you know, but the relay that carries the messages cannot read them. Reliability is ranked above features: a message that cannot be read must say why, and a client that falls behind must heal or tell you.

Status: stage 1 (desktop, text, files, multi-device, recovery). Not independently audited. Read `THREAT-MODEL.md` for what this does and does not protect.

## 1. Pieces

```
crates/proto   wire types, request signing, device certificates (Ed25519 + SHA-256 via dalek/sha2)
crates/relay   the relay: axum + SQLite. Append-only log per group, key package directory, welcome inbox, blob store
crates/core    the whole client without a UI: identity, OpenMLS, sync, local encrypted history, JSON `App::call` API
app/src-tauri  Tauri 2 shell (Rust core in-process; JS can only call one `rpc` command)
app/ui         plain HTML/CSS/JS, no build step
crates/core/tests   integration harness: real relay (127.0.0.1), several simulated clients, fault injection, chaos soak, fuzz
```

Why Tauri 2 + Rust core: OpenMLS is Rust, so the same crate runs on desktop and later behind a UniFFI binding on iPhone. The UI never sees key material.

Crypto is only ever invoked through reviewed libraries:

| Job | Library |
| --- | --- |
| MLS (RFC 9420), ciphersuite `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (0x0001, the RFC's mandatory suite) | `openmls` 0.9 + `openmls_rust_crypto` |
| Signatures (device/account keys, request auth, invites) | `ed25519-dalek` |
| Attachments (one fresh key and nonce per file) | `chacha20poly1305` |
| State at rest (passphrase to key, then AEAD) | `argon2` (Argon2id, 64 MiB, t=3) + XChaCha20-Poly1305 |
| Device pairing (sealed box to the new device's one-time key) | `crypto_box` |

Nothing in this repository implements a cipher, a hash, a KDF, a signature scheme or a key exchange.

## 2. Identity

* **Account** = an Ed25519 key pair (`account key`). The account id is its public key (hex). It is the stable "who is this person".
* **Device** = an Ed25519 key pair generated on that machine. The device key is also the MLS signature key.
* **Device certificate**: the account key signs `(account, device, created)`. The MLS `BasicCredential` identity of every leaf is this certificate. Every client checks the certificate (signature and that it names the leaf's key) for every key package, welcome, and every member of every group it joins. A relay cannot invent a device for someone.
* **Safety number**: SHA-256 chain over the two account ids (sorted), shown as 12 groups of 5 digits plus a QR of the same number. Comparing it out of band proves that both of you talk to the account keys you think. Because the account id *is* the key, there is nothing that can silently change underneath a verified friend; new *devices* appear in the member list's device count and as a notice.
* Keys are generated on the device, stored only inside the encrypted state file, and never leave it. The one exception is on purpose: pairing a second device and the recovery key both move the *account* key (see section 8).

## 3. Communities, channels, and the MLS mapping

An MLS group has one set of members and one key schedule. The simplest honest mapping that gives per-channel cryptography is:

* **channel = one MLS group** containing every community member's devices. Messages in a channel are MLS application messages in that group.
* **community = one extra "control" MLS group** with the same members. Its *group context extension* (authenticated by every commit, and handed to every joiner inside the Welcome) holds the community table: name, owner account, admin grants, channel list (id and name), public halves of invites, and the ban list. The roster of the community is exactly the membership of the control group.

Consequences worth knowing:

* Names of communities and channels, and roles, exist only inside MLS-protected state. The relay sees random group ids.
* Adding a person to a community costs one commit per channel plus one for the control group. That is cheap for the intended size (tens of people, a few dozen channels). If it ever hurts, the next step is to derive per-channel secrets from one group's exporter, at the price of weaker per-channel removal semantics. Not needed now.
* All channels contain all members in stage 1 (no private channels). A private channel is just a channel group with a subset; the machinery does not assume otherwise.

### Roles and authorization

Roles: **owner** (the founder at first; can be handed over), **admin**, **member**.

* The community id is `SHA-256("commons/community/v1" || founder || nonce)`, so the founder's key is pinned in the id and in every invite link.
* **Ownership transfer**: the table carries a chain of steps, each signed by the owner before (`Sign_prev(community, seq, from, to)`), starting at the founder; the last key is the owner today. The new owner carries the chain into each channel, as its own commit (a signed step speaks for itself, so this works in a channel whose table still names the old owner), and re-signs the admin grants. The previous owner stays an admin and can then leave.
* An admin is an *owner-signed grant* `Sign_owner(community, account)` stored in the control table and copied into each channel's own table. A grant counts if any owner in the chain signed it; a rogue group cannot make up admins. A grant leaves the table when its holder leaves, so someone who comes back comes back as a member.
* **Authorization is local to a group.** Every commit is checked by every member (including its author, before the author merges it) against the group's own authenticated table:
  * Add: sender is admin/owner, or adds a device of their own account.
  * Remove: owner can remove anyone but the owner; admin can remove members; anyone can remove their own account's devices.
  * Group-context change: only the owner changes admins or hands over the community; only admins change names, channels, invites, bans. The chain of owners can only grow.
  * Everything else (PSK, reinit, external commits, standalone proposals) is rejected.
  A commit that fails the check is skipped by all honest clients identically, so they do not fork. This rule came out of the chaos test: an earlier version consulted other groups' state (ban list, revocations) and two clients that interleaved logs differently disagreed about one commit. Authorization now depends on nothing but the group's own state; "soft" policy (banned accounts, revoked devices, join validation) is enforced by the admin that *creates* the commit.
* Message-level rules (edit only by author; delete by author or admin; reactions by anyone) are enforced at display time. They affect what is shown, not cryptographic state.

### Invite links

`commons://join/<base64url json>` = relay URL, community id, owner account, invite id, and an **invite secret key**. The control table holds only the invite's public key. A joiner signs a join request with the secret key and posts it (in the clear, it contains no secrets) into the control group's log. Any online admin validates it (invite exists, not expired, uses left, signature, not banned, device not revoked), then commits `Add + table update` (consuming one use) in one commit and sends the Welcome. The joiner verifies the Welcome: group id derives from the owner and nonce, owner matches the link, every admin grant verifies, the sender is the owner or a granted admin. A relay that fabricates a lookalike group cannot pass this.

Because an invite needs *an admin online at some point*, joining is asynchronous. The UI says so.

### Join, leave, kick

* **Leave**: the leaver posts `Leave` in the control group; an admin removes all of that account's devices from all groups (an MLS member cannot remove itself). The owner has to hand the community over first. Coming back takes a new invite, and the new Welcome replaces the old group state.
* **Refused**: a joiner is in no group, so an admin answers through the joiner's inbox with a signed `JoinReply` (a reason code, the admin's certificate and grant; the joiner checks it against the founder in the link). A link admits as many people as it has uses left, counted per batch; the others are told it is used up. Without an answer the request is sent again every hour, and the person can stop waiting.
* **Kick** (owner removes anyone, admin removes members): one commit per group removes every device of the account, which rotates the group secrets. The same commit that removes the account from the control group adds it to the ban list, so the old invite no longer lets them back. A removed member keeps what they already received (nothing can change that) and cannot derive any later epoch. `tests/reliability.rs::removed_member_cannot_read_later_messages` feeds the removed client every ciphertext the relay holds and asserts that none of the post-removal ones opens.
* The relay's read access for the removed account is cut after a grace period (default 120 s) so that the removed device can fetch and understand its own removal commit rather than just seeing "denied".

## 4. The relay protocol and ordering

The relay is a dumb, hostile-assumed append-only log:

* `POST /v1/groups/{gid}/messages {id, data}` appends an opaque entry and returns a gap-free `seq` (1, 2, 3 ...). `id` is client-chosen (random 128-bit); a repeated `id` returns the original `seq`, which makes retries after a lost response exactly-once.
* `POST /v1/sync {cursors, ack_welcomes, wait_ms, max_bytes}` returns entries after each cursor (and the device's welcome inbox), long-polling optionally. A response stays under about 4 MB but always holds at least one entry, no group gets more than a share of it, and `more` lists the groups that have further entries: a client is not caught up on those, however few entries arrived. An entry is at most 256 KB. A client that cannot read an answer asks for less, then for one group at a time, so one bad group pauses alone. Offline queue = the log itself.
* `POST /v1/groups/{gid}/acl` changes who may read a group. The relay records each group's root (its control group) and the root's owner. A change needs the owner, or an account showing its owner-signed grant for the root; the owner cannot be removed; the owner can withdraw a grant at the relay (`demote`), since a signature cannot be recalled. A signed ownership step moves the record to the new owner. Admins compare the relay's list with the group about once a minute and put back anyone who was cut off. A group the relay does not know stays denied: re-creating it would restart its log below everyone's cursor.
* Entries start with a tag byte: `1` MLS application message, `4` commit, `2` rejoin request, `3` join request. The relay does not parse further.
* There is **no epoch enforcement** on the relay. The log order *is* the order. Clients process each group's log strictly in `seq` order, and the first valid commit for an epoch wins; later commits for the same epoch are ignored by everyone alike (the loser's client notices, drops its pending change, re-evaluates and retries).

Commit procedure (the committer):

1. sync until caught up; build the commit; persist the staged commit and advanced key schedule to disk **before any network I/O**;
2. post it; *do not merge yet*;
3. when the log delivers the entry, check it against the group rules; if it is still the first commit at this epoch, merge, then send Welcomes to the added devices and schedule the relay-ACL changes. If another commit won, `clear_pending_commit` and recompute.

Merge-after-acceptance is what makes concurrent commits converge instead of fork. Welcomes are only released after the commit is accepted.

The background loop runs in rounds: plan (reconcile, encrypt, pick what to send; local work and disk), talk to the relay with the client unlocked, apply the results (local work and disk). Device lists and key packages are fetched by one round and used by the next. Actions that need an answer on the spot (creating a room or an invite, changing a role) still hold the client while they wait; the UI's read calls then get the last answer instead of blocking.

The reconcile loop (admins, and each user for their own devices) derives needed commits from state: *roster (control group) x device directory x current leaves*. Adds, removals, new channels' members, revoked devices, leave/kick requests all fall out of the same diff, so a crash halfway through only means the next tick finishes the job.

## 5. Attachments

The sender encrypts the file with a fresh ChaCha20-Poly1305 key, uploads the ciphertext as a blob under a random id, and sends `{blob id, key, nonce, name, mime, size}` inside an MLS message. Receivers download, authenticate, decrypt and cache locally (re-encrypted with the vault key). The relay sees the blob's exact size and who fetched it, not what it is. Limit 25 MB.

## 6. Local storage

One state file: magic, Argon2id parameters, salt, nonce, XChaCha20-Poly1305 ciphertext. It holds identity secrets, the whole OpenMLS store, every community's state, and the message history, so a stolen laptop yields nothing without the passphrase. Writes are atomic (temp file, fsync, rename) and the previous generation is kept as `state.bak`. A failed authentication tag means a wrong passphrase or a damaged file, so the backup is tried before saying which: if it opens, the damaged file is set aside as `state.damaged`, the previous generation is used, and the user is told. An exclusive lock on the data folder means only one process can have it open, because two would hand out the same key material. Search is a local scan of that history. Attachment cache files are sealed with the same key and named after the blob id, key and nonce together.

Typing indicators are off by default. When on, they are ordinary padded application messages with no special tag, which leaks only that "a message-sized packet was sent now", something the relay sees about every message anyway.

## 7. The hard parts (what hurt in Matrix) and what is done about them

### 7.1 Never a silent undecryptable message

Every failure turns into (a) a line in the channel timeline, in words, and (b) an entry in the Activity log. There is no code path that drops a log entry without one of these. The taxonomy:

| What happened | Detection | What the client does |
| --- | --- | --- |
| Relay withholds or loses entries | `seq` is not `cursor+1` | Does **not** skip. Raises a `gap` notice and refetches. The cursor only advances over contiguous entries, so nothing is "missed" quietly. Dependent groups wait for the control group. |
| Duplicates, reordering by the transport | sorted and de-duplicated by `seq` | Exactly-once processing. |
| Same message posted twice by the sender after a lost response | relay de-dupes on `id` | Nothing to do. |
| Garbage, malformed MLS, unknown tag | parse failure | Timeline line: "a malformed message was in the log"; group keeps working. |
| Message from an older epoch than we keep (we keep 8) | MLS error | Timeline line explaining that its key was dropped. |
| Replay / key already used | MLS error | Timeline line. |
| Message for a future epoch | MLS error | If we are in the rolled-back state (below): resync. Otherwise a line explaining a commit in between was rejected or is missing. |
| Commit not allowed by the group's rules | authorization check | Skipped identically by everybody; notice names who and why. |
| **Own state is older than what we already sent** (restore from backup, cloned disk) | own entry in the log that we did not make | See 7.3. |
| Relay says "denied" for a group | sync response | Marked, notice: probably removed while offline (a real removal is learned from the log itself during the grace period). |

### 7.2 Multiple devices per person

Account = one account key, any number of device keys, one MLS leaf per device. A device is added to every group by an existing device of the same account (members may add their own account's devices) or by an admin, driven by the relay's device directory (which is self-verifying: certs are signed by the account key).

Adding a second device **by QR / code**:

1. The new device generates a one-time X25519 key and a random rendezvous id, and shows `commons-pair:<base64 json {relay, rid, pk}>` as text and QR.
2. An existing device scans or pastes it and posts a sealed box (to that one-time key) containing the account secret key and the display name to `rendezvous/rid/a`. The relay holds an opaque sealed blob for at most 15 minutes and cannot open it: it never saw the code.
3. The new device opens it, makes its own device key and certificate, registers, publishes key packages, and answers on `rendezvous/rid/b`.
4. Its account's existing devices (and admins) see the new device in the directory and add it to the control group and every channel. It then receives Welcomes and the community table.

The code travels out-of-band (screen or camera), which is the authentication. A four-digit check number derived from the one-time key is shown on the new device and on the approving device before anything is sent, so you can tell you are approving the screen in front of you. Writing to the rendezvous needs a registered device and the first writer keeps the slot; anything that does not open with the one-time key is ignored and the wait goes on. A pairing interrupted by a restart shows its code again, and can be given up from that screen.

What the new device does **not** get: history from before it joined. MLS gives a device the keys of its epoch onward, by design, and the old plaintext lives only on other devices. A device-to-device encrypted history export over the same pairing channel is the obvious next feature (see section 9) and the seam exists (`rendezvous` can carry it, history is one JSON structure). The UI says this plainly.

**Who may put you in a community.** A community Welcome is accepted in exactly three cases: this device asked (an invite link of ours names that community and founder), the community is already live here (a re-add after a lost state), or the Welcome's table holds a **consent**: `Consent { account, ts, sig }`, signed with this account's key over the community id, the account and the time. The account key exists only on the person's own devices, so nobody else can make one, and it names one community, so it cannot be reused for another.

It works like this. The consent goes out with the join request; the admin who adds the joiner writes it into the control table in the same commit. The table is part of the group context, so it reaches every device inside the Welcome, authenticated by MLS. A second device, or one made from the recovery key, has no history of its own to go on, and the table is what it reads: pairing and recovery work with no other device of the account online, whoever adds the device first. Being in a group is no evidence: whoever runs a community can add every device of an account in one commit, so each device sees the other there (this was the old rule, and the hole). Consents in tables from before this rule heal by themselves: a member whose table entry is missing offers one, as a `Consent` envelope to the control group (or directly if they are an admin), and any admin merges it. The table holds one per account, the newest, at most 300; honest admins drop it when the member goes.

A consent can be taken back. Leaving, being removed, cancelling a join or being refused puts its signature on the device's `withdrawn` list, and a Welcome that rests on a withdrawn consent is refused however genuine the signature: a hostile owner can keep an old table entry. Joining again signs a new one. Pairing passes the list on. Limit: a device made from the recovery key has no list, so it can be pulled back into a community this account once joined and left, if the owner kept the old entry and adds it. It cannot be pulled into one the account never joined. A device whose Welcome was refused stays as a dead leaf in the sender's group; that harms nobody but the sender.

A channel Welcome needs a community we are in and a sender who is its owner, an admin, or our own account; for a community we do not know yet it is set aside (the key store is restored, so it can be opened again) and retried when the community arrives.

### 7.3 Restored from an old state, cloned disks, and key reuse

The dangerous case is a client that comes back from a backup: it believes it is at epoch N with unused sender keys, but the group has moved on and *it* used some of those keys. Reusing an AEAD key and nonce on different plaintexts is catastrophic. The defences:

1. A client encrypts only after it has caught up with the relay in this process (`synced`), so a restored client sees the evidence before it can send.
2. The state is saved, including the advanced key schedule and the ciphertext, **before** any network I/O when sending; the ciphertext is stored in the outbox and re-posted by the same id, never re-encrypted.
3. Evidence = an entry in the log from our own leaf that our state did not create (we remember our own entry ids; they are part of the backup too).
   * If it is an application message: the group is `tainted`; before sending anything the client commits a self-update, moving to a fresh epoch with fresh sender keys. The content of the lost message is not recoverable on this device; the timeline says so.
   * If it is a **commit** (tag `4`): our private leaf keys from that commit are gone, so we can no longer follow the group at all. The client marks the group `broken` and posts a signed *rejoin request*. An admin (or one of our own other devices) replaces our leaf (one commit: remove + add) and we continue from a Welcome. Messages in between cannot be read on this device (shown as one explained notice); other devices still have them.

### 7.4 Lost devices and the recovery key

* **Lost or stolen device**: from any device (or a fresh one made from the recovery key) run `Revoke`. The account key signs a revocation; the relay stops serving that device (403, which the device shows as "revoked") and deletes its key packages; the revocation is also broadcast into every community's control group; admins and the account's own devices remove its leaves, which rotates keys. Until revoked, a lost device is still a member. At rest it is protected by its passphrase only.
* **Recovery key**: the account secret key and a checksum as exactly 56 base32 characters (typo-checked; a character more or less is an error). It restores *identity*: a new device can be created for the same account, and any online admin or other device adds it to the communities. It restores **no messages**: messages exist only as per-device local history and MLS epochs the new device was never part of. This is stated in the UI where the key is shown and where it is entered. If every device and the relay's key packages are gone and no old leaf of yours remains in a group, you need a fresh invite; the Welcome acceptance rule above only trusts welcomes that show another device of yours.
* **Not restored: the withdrawn-consents list.** The recovery key is only the account secret plus a checksum (56 characters a person types), so it cannot carry the list of consents this account took back by leaving, being removed or cancelling a join (section 6, "A consent can be taken back"). A device made from the recovery key therefore starts with an empty list, and a community that once had this account can pull it back in if its owner kept the old table entry and adds the device. It cannot be pulled into a community the account never joined. This is a known limit, left as is on purpose: putting the list in the key would make the key longer than a person can copy, and putting it on the relay would let the relay see which communities an account left. Pairing a second device does carry the list (the newest 40). If you left a community and later restore from the recovery key, leave it again if it shows up.
* An optional encrypted backup of local history (key from the recovery key) is a stage 2 candidate.

## 8. Trade-offs we took on purpose

* The **account key is on every device** (needed to issue certs for new devices and to sign revocations). Stealing one unlocked device's state means being able to enroll more devices, **even after that device is revoked**: revocation stops the device, not the key it holds, and the same key can revoke your good devices, so "approval by a still-trusted device" cannot be built without the split below. Mitigations now: at-rest encryption, a notice on your own devices whenever one appears, device-added notices for members, fast revocation. If a stolen device keeps enrolling new ones, the way out is a new account. Planned: a cross-signing split (account key only on devices explicitly marked "trusted", others cannot enroll).
* One MLS group per channel is simple and robust but multiplies commits. Fine for tens of people.
* The relay can see which device posted each entry and when (it authenticates requests). Hiding that needs a different transport (sealed sender style); not in stage 1.
* Past epoch secrets are kept for 8 epochs so that messages that cross a commit still decrypt. This widens the forward-secrecy window slightly.
* No OS keychain in stage 1: the passphrase is the only at-rest protection and is asked at start. A keychain-wrapped key is an easy later addition.

## 9. Seams for later stages (not built)

* **Voice and screen share (LiveKit with E2EE)**: built in stage 2, see section 11.
* **iPhone app**: `commons-core` has no UI or OS dependency except the filesystem path; put a UniFFI layer over `App::call` (already a JSON in/out API) and write a SwiftUI shell. Push: a wake-up-only APNs ping from the relay that carries no content.
* **History transfer between devices** over the pairing rendezvous; **encrypted history backup**; **private channels**; **ack/read markers** if wanted; **sealed sender** transport.

## 10. Test strategy

The point of the harness is to try to break the properties above with a real relay on loopback and several in-process clients:

* `basic`: three people, messages/edits/deletes/replies/reactions/files/search/unread, roles, restart, plaintext absent from the state file.
* `reliability`: kick (and the removed client fed the whole log), relay storage scanned for every plaintext marker (messages, file contents, file names, channel and community names, display names), offline periods, reversed and duplicated batches, withheld entries, lost responses, relay restart, garbage entries, concurrent commits and two admins racing for one invite, a hostile member's unauthorized removal, a client restored from an old disk including a lost commit.
* `devices`: pairing by code, revocation, recovery key, leave, rename.
* `regress` and `adv`: one test per bug from the adversarial review, including the attacks themselves (`adv` needs the `test-hooks` feature, which test builds turn on).
* `simulation`: seeded chaos (joins, kicks, messages, offline, reorder, duplicates, stale views, relay restarts, app restarts) followed by a convergence check: same epoch secrets on every member for every group, kicked users cannot open any final message, every member can send and read.
* `fuzz_parse`: mutation fuzzing of every parser that sees network bytes.


## 11. Voice rooms and screen share (stage 2)

Any room can host a call ("Call" button in the room header). There is no new group type: the room's own MLS group supplies the keys, so the people who can join are exactly the people who can read the room.

```
 client (browser or webview)                       relay                       LiveKit (SFU)
  call.js + livekit-client                    POST /v1/groups/{gid}/call-token     |
  rpc call_state ----> core: MLS exporter      (signed by device; checks ACL) -->  | room = H(gid), identity = device id
  rpc call_token ----> core ---------------->  JWT HS256, 15 min, no names          |
  frames encrypted in a Web Worker (AES-GCM) ---------------- ciphertext ---------> forwards ciphertext
```

* **Keys.** For every device `D` in the group at the current epoch, `key(D) = MLS-Exporter("commons-call-e2ee-v1", "<gid>|<D>", 32)` (RFC 9420 section 8.5, OpenMLS `export_secret`; `crates/core/src/call.rs`). Each sender therefore has its own key (no key/nonce sharing between senders), every member derives all of them, and nobody outside the group at that epoch can. The 32 bytes are handed to the page, which gives them to the LiveKit client's key provider; LiveKit runs its own HKDF over them and encrypts every audio/video frame with AES-GCM in a Web Worker (insertable streams). We wrote no cipher or KDF. The media key is the one deliberate exception to "the UI never sees key material": it has to be in the web view to encrypt frames. It is derived per epoch, only for the media, and is not the text keys.
* **Rotation.** Every commit changes the epoch (join, leave, kick, device change, self-update), and the page polls the core (1.5 s) and installs the new keys at key slot `epoch mod 16`. The previous epoch's key stays for 3 s so frames in flight still play, then its slot is overwritten with random bytes (and the current keys are set again, because setting a slot also selects it for sending: this was a real bug found by the tests). Result: a removed member's old key stops decoding within seconds and the new key is never available to them.
* **Tokens.** `POST /v1/groups/{gid}/call-token`, signed like every relay request, answered only if the account is in the relay's ACL for that group *and* has not been cut off. The client that applies a removal sends the cut-off immediately (`call_remove` in the ACL request) carrying its owner-signed admin grant, and the relay treats it like any other change to who is in the group: only the owner or a granted, not-demoted admin may make it (a plain member gets 403, tested); the read ACL keeps its 120 s grace period. Token requests are rate-limited per device and the call server can be absent (404 with a plain message). The token is an HS256 JWT (standard `jsonwebtoken` crate) with `sub` = device id, `video.room` = `SHA-256("commons/livekit-room/v1" || gid)` in hex, 15 min lifetime, no display name. The relay holds the LiveKit API secret (can mint tokens, sees no media, has no keys).
* **Refusing to run unencrypted.** The page checks, in this order: browser can do frame encryption (insertable streams or `RTCRtpScriptTransform`) else the call does not start and says why; keys are installed before connecting; after connecting the worker must confirm encryption is on or the connection is closed before anything is published; every local track must come back marked encrypted or it is stopped; every remote track is subscribed only if its sender is a current member of the MLS group and the track is marked encrypted, otherwise it is blocked and shown as such. No code path publishes without encryption.
* **Control surface.** Join, leave, mute, deafen (also mutes), per-person volume (remembered per account), push to talk (a key, default backtick, only while the window has focus; a hold-to-talk button for mouse use), microphone / speaker / camera pick, browser noise suppression and echo cancellation, speaking indicator, automatic reconnect (the SDK's resume first; if the connection is lost for good the page fetches new keys and a new token and rejoins with back-off for up to 10 minutes). Screen share: the browser's own picker (screen, window or tab), audio where the browser and OS give it, two sender profiles ("sharp text" 1080p/15 with 360p and 720p layers; "smooth" 720p/30), viewer quality Auto/High/Medium/Low (simulcast layers), several sharers at once, fullscreen per tile. Camera is the same machinery with a camera track.
* **UI integration.** Three script tags in `index.html` (`vendor/livekit-client.umd.js`, `call.js`, `call-ui.js`) and `call.css`; nothing else of the front end is changed. `call-ui.js` adds the Call button to the room header and one floating panel, using the app's `S`, `rpc`, `esc` and the clubhouse design tokens.
* **Where it runs.** Needs a WebView with WebRTC, insertable streams and `getUserMedia`: Chromium-based browsers and WebView2 (Windows) and recent Safari/WKWebView qualify on paper; the Linux Tauri shell (WebKitGTK) is a separate question, answered in `docs/CALLS-PLATFORMS.md`.
