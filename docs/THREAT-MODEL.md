# Threat model

**This software has had no independent security audit.** It is a stage 1 build for a small group of friends. It uses a well-known MLS implementation (OpenMLS) and RustCrypto/dalek primitives and implements no cryptography itself, but the way the pieces are combined (device certificates, the relay protocol, authorization rules, recovery) is new code that nobody outside this project has reviewed. Treat it as "much better than nothing, not yet proven".

## What we protect, against whom

| Asset | Protected against | By |
| --- | --- | --- |
| Message text, edits, replies, reactions, attachment bytes, file names and types | The relay operator and anyone who breaks into the relay; network observers (add TLS on top, see DEPLOY.md) | MLS application messages (AEAD per sender key), attachments encrypted client-side before upload |
| Community names, channel names, roles, display names, invite settings | Same | They live only inside MLS group-context extensions and MLS messages |
| Who may speak or change membership | A malicious member, a malicious relay | Signed MLS commits; every client enforces the same group-local rules and skips commits that break them |
| Being put in a community without agreeing | Anyone who runs a community, however many devices you have | A device accepts one only if it asked, or the table holds that community's consent signed with your own account key (DESIGN.md 7.2). Test: `two_devices_are_not_a_way_to_pull_someone_into_a_community` |
| Later messages after someone is removed or a device is revoked | The removed member / lost device | Commit with a path: new epoch secrets they cannot derive. Tested by feeding the removed client the whole relay log |
| Identity of a friend | A relay or stranger impersonating them | Account key = account id; device certificates signed by it; safety numbers for out-of-band verification |
| Keys and history on a stolen powered-off laptop | A thief with the disk | Argon2id + XChaCha20-Poly1305 over the whole state; side files sealed with the same key |
| Message integrity and ordering within a group | A relay that reorders, duplicates, withholds | Strict `seq` processing, gap detection, MLS epoch and generation checks, exactly-once ids. A relay can still *delay or drop*, and the client says so |

## What the relay can see (exactly)

The relay is treated as hostile. This is everything it stores or observes. Verified by `relay_holds_no_plaintext` (greps all relay files for message text, file names/contents, channel/community/display names) and by the table list in that test.

**Stored (tables `devices`, `keypackages`, `group_members`, `group_auth`, `acl_demoted`, `entries`, `welcomes`, `rendezvous`, `blobs`, `usage`, plus a `blobs/` directory):**

* **Device registry**: for each device its public key, its account's public key, the certificate (including its creation time), and whether and when (signature only) it was revoked. So the relay knows which devices belong to one account.
* **Key packages**: opaque to it, but they contain the same certificate and public keys. It knows how many are left per device.
* **Group membership list (ACL)**: for each random group id, the set of *accounts* allowed to read and write it. So it learns the social graph at account level: which accounts share groups, group sizes, when members are added and (after the grace period) removed. It does not know names or purposes.
* **Group authority**: for each group its root (control group), and for each root its current owner account and how many times ownership changed. So the relay *knows* which channels belong to which community, and who owns it. It also sees an account's owner-signed admin grant whenever that account changes an ACL, and holds the accounts whose grant the owner withdrew. It does not hold a list of current admins, but it learns them as they act.
* **Usage**: bytes each account has posted, and the size and uploader of each blob, to enforce quotas.
* **Log entries**: `(group id, seq, client random id, tag byte, ciphertext)`. The ciphertext size is padded to a multiple of 256 bytes by MLS. The tag tells it: application message, commit (a membership/epoch change), rejoin request, join request.
* **Welcome inbox**: for each recipient device, MLS Welcome ciphertext, deleted when acknowledged.
* **Join requests** (the only plaintext JSON it holds): invite id (random), the joiner's certificate (account and device keys), a signature, and the account's consent to that community (a signature and a time). So it knows *which account asked to join which group id*, and when they pressed the button. The community invite's secret key is never sent.
* **Rejoin requests** (plaintext): the device's certificate, a nonce, a signature. It learns that a device lost state.
* **Blobs**: ciphertext, exact size, random id. It knows which device uploaded and which devices downloaded each blob, and when, but not which message or channel it belongs to.
* **Rendezvous** (device pairing): a sealed box it cannot open, for at most 15 minutes, under a random id it only learns from the request itself, with the account that wrote it.

**Observed, not stored:** IP addresses and timing of every request (inherent to any server; use a VPN/Tor if that matters), and **which device posted each entry** because requests are signed by the device key. Message *timing and size* is visible; who is talking to whom inside a group is *not* visible beyond "this device posted this entry" (MLS hides the sender inside the ciphertext, but the HTTP request does not).

**Not visible to it:** message text; attachment contents, names, types; community, channel and display names; invite limits; who reacted or edited; which entries are replies; typing indicators (on by default for new accounts, off for an account that turned them off; they are ordinary padded messages); room and community colours (carried inside the encrypted community table, signed by the owner); live events (sealed with a key only the room's members derive, held by the relay in memory for seconds and never stored; see "The live channel" below); read state (there are no read receipts); search queries (local).

**What a hostile relay can do (and what we do about it):**

| Attack | Outcome |
| --- | --- |
| Read | Only the metadata above. |
| Drop or delay entries / deny service | Possible. Detected as gaps (`seq` is gap-free) and shown. Cannot be prevented by design: this is a centralized relay. |
| Reorder or duplicate | Neutralized: processed strictly by `seq`, de-duplicated. |
| Show different logs to different members (fork) | Possible in principle. MLS makes the forks cryptographically distinct, clients that cannot follow report it (future-epoch / gap messages). Not fully detectable without a gossip channel between members. |
| Replay old requests | Within a 10-minute window; mostly idempotent (ids). |
| Forge a message or a member | Not possible without a device key; certificates are verified against the account key. |
| Serve a wrong key package / invent a device | Rejected: credential must be a valid certificate for the expected account and device. |
| Hide a device revocation | Possible (relay is the directory). Mitigated by also broadcasting the revocation inside every community. A device that never sees either could still be added by a stale admin. |
| Block a person from joining or being added | Possible (omit their key packages or requests). Visible as "waiting". |
| Lift a call cut-off, or cut someone off from calls | The relay keeps a `call_denied` list (accounts an admin cut off from a group's calls, kept apart from membership). The relay can add or drop entries, so it can deny a call to a member or let a cut-off member back into the room's token service; it still cannot decrypt the call, and members' clients stop trusting a removed device's media keys at the next epoch. |
| Make a client believe it was removed | Possible ("denied"); the client labels it as unconfirmed and keeps its local state. |
| Lie in `group_members` ACL | Gives it nothing to gain beyond what it already has; it can only cause denial of service. ACL rules protect against members, not against the relay itself. |
| Tell a client it has caught up when it has not (omit `more`) | The same as withholding entries. |

## Voice rooms and screen share (stage 2): what each server can see

Two servers are involved. Assume both are run by the same person and both are hostile.

**The relay (token service) additionally learns**, when someone calls `call-token`: the random group id (it knew it already), the calling device and account, and the time. So it learns *who joined which room's call and when*, not that a call has audio, video or a screen share, not names. It holds the LiveKit API secret, so it can mint tokens for anyone: that lets the operator join a room as a silent participant (see below), never decode it.

**The LiveKit server sees, exactly:**

* IP addresses of every participant (and, with TURN, of nothing more than the TURN allocation); join and leave times; the room name, which is a one-way hash of the group id (it cannot be linked to a room without knowing the group id, which the relay operator does, and it is the same person in the default setup); participant identities, which are device ids (stable across calls: it can tell that the same device returns, and, together with the relay, which account).
* Whether each participant is publishing audio, camera or a screen share (track type), publish and mute state, the video resolution and frame rate it negotiates, and simulcast layer choices.
* **Who is speaking and when.** The audio level travels in an RTP header extension the server needs for active-speaker detection, and silence suppression makes silent packets tiny, so talking time is visible even though content is not. Packet sizes and timing of voice leak speech rhythm (the well-known variable-bit-rate effect, with the first byte of each Opus frame and the packet size in clear); a determined server operator may learn something about *what* is said from sizes alone, which is why this is called "cannot hear", not "learns nothing".
* Sizes, timing and volume of everything, hence who is sharing a screen and for how long, and the screen's rough activity (static document vs. moving video) from the bitrate.
* LiveKit's own client info: SDK version, browser and OS strings.

**It cannot see:** audio, video or screen content (frames are AES-GCM encrypted in the sender's browser with keys the server never gets), display names, group, room, community names, who is in the community but not in the call.

**What a hostile media server or operator can do.** Hear nothing. Drop, delay or replay packets or end the call (denial of service; detected as silence or the "reconnecting" banner). Join the room with a minted token and record ciphertext: tested (the operator tab sees the forwarded Opus payloads; a plaintext control stream decodes to the tone, ours decodes to nothing). Add a fake participant: clients block any sender whose device id is not in the room's MLS group and any track not marked encrypted. Mark a track "unencrypted" to make clients play it as is: clients refuse to play tracks not marked encrypted, so this ends in silence, not in plaintext playback. Replay old ciphertext: harmless audio replay, same as a recording of encrypted data; frames are not individually sequence-protected beyond AES-GCM's IV (this is a limit of LiveKit's frame scheme).

**Members.** A current member can derive every sender's key (keys are per sender for separation, not for authenticity), so a malicious member can inject audio that appears to come from another device's identity if they also control the server's routing. Group-level authentication, not sender-level: same trust level as the text channel's members. A removed member keeps the media they already received and, if still connected with a modified client, keeps receiving ciphertext they can no longer decode after the next epoch; clients also stop trusting their identity at once. For up to the 3 s grace period after a rotation, frames under the previous epoch's key are still accepted, so a removed member could inject audio in that window only if the server still routes them (their token is not renewed; the SFU does not itself evict them: an operator who wants that calls LiveKit's `RemoveParticipant`).

**The call page holds the media keys.** A malicious script in the web view could read them; the same page can already read the decrypted messages shown in it, so this adds no new class of exposure, but the web view and its content security policy (`script-src 'self'`) are part of the trust base.

**Platform limits.** Frame encryption needs a browser or web view that provides insertable streams. Where it does not exist the app says so and does not start the call. See `docs/CALLS-PLATFORMS.md`.

## The live channel (unstored live events)

Camera-tile drags and holds, wall strokes and wall snapshots are *live events*: useful for seconds, worthless afterwards. They used to be ordinary MLS application messages. That made each one a permanent entry in the relay's append-only log, charged to a lifetime quota, and a state rewrite for the sender. It would also have been wrong to simply stop logging them: every MLS application message advances the sender's ratchet, and the groups tolerate only 2000 skipped steps (`SenderRatchetConfiguration::new(64, 2000)`), so a member offline while unlogged events piled up could never again read that sender's next real message. So live events do **not** go through the MLS application ratchet at all.

**How an event is protected** (`crates/core/src/live.rs`):
* *Key.* `MLS-Exporter("commons-live-v1", group id, 32)` of the group's current epoch: the same derivation as the call keys with a distinct label. Every current member has it; nobody else does; a commit starts a new one. Deriving it reads the group and changes nothing, so sending a live event advances no MLS state and saves nothing.
* *Seal.* ChaCha20-Poly1305, a fresh random 96-bit nonce per event, plaintext padded to a multiple of 128 bytes. The additional data binds the group id, the epoch and the sending device. The event's kind is inside the ciphertext and under the signature (so it is bound, and the relay does not learn it).
* *Sender.* The sender's device key signs, inside the ciphertext, the group, epoch, device, nonce, kind, time and data. A receiver verifies against the device the relay names, which must be in the group's current roster. So one member cannot speak as another (the group key alone would allow it), and a removed member is not heard even for an epoch they still hold a key for. The nonce is under the signature, so an event cannot be re-sealed with a new nonce to evade the replay filter.
* *Order with the log.* A wall lock or clear lives in the signed community table, which travels through the log; a stroke that follows it travels by the live channel, which has no order with the log. So each event also carries, under the signature, how far its sender had read the community's control log (one more while a change of its own is on its way in). A receiver that has not read that far holds the event (up to 10 seconds, at most 16 per sender and 64 in all) and delivers it once it has; otherwise a stroke drawn on a fresh wall could arrive before the clear and be thrown away, or a stroke by a non-owner after a lock could be taken by a device that has not yet heard of the lock. A hostile sender can only make its own events wait or lapse by claiming too far a position.
* *Epochs and replay.* Events under the current or the immediately previous epoch are accepted; earlier ones are dropped; one from an epoch not yet reached is held up to 10 seconds. Each nonce is taken once; events older than 30 seconds (by the signed time) or more than 60 seconds ahead are dropped. Receivers keep the old per-sender rate limit (20 a second from this channel) and buffer caps, and nothing is stored in history.

**What the relay does** (`crates/relay/src/live.rs`): `POST /v1/groups/{gid}/live` takes sealed blobs from a signed-in device whose account is a current member of the group, and `POST /v1/live/poll` long-polls (up to 25 s) for news in the rooms the caller belongs to. The buffer is in memory only, never written to the database or disk, per room: events are kept 30 seconds at most, at most 1500 events and 3 MiB, at most 450 events and 1 MiB from one device, 30 posts a second per device per room, 64 MiB for the whole relay; a poll answer carries at most 200 events and 512 KiB per room. A restart empties it.

**What the relay learns, exactly.** For each event: that device D, of account A, posted a blob of N bytes (padded, so N is a bucket, 128-byte steps) to group G at time T; and which devices poll which rooms, and when. It sees no plaintext, kind, key or signature, and it cannot forge an event (it lacks the room key and any device key). It could always see roughly this for log entries; the difference is that it now also sees *polling* (a device asks about all its rooms every few seconds; each answer says which of those rooms had events), and that live events leave no trace on disk. A hostile relay can drop, delay, reorder, or replay (once only: nonces are remembered) live events: a denial of service for a feature that is optional, never a break of content. It cannot deliver events to a non-member (membership is checked on every post and poll), but one removed from the relay's list only after a delay (`acl_delay_secs`) can still *fetch* sealed blobs in that window; they cannot open them once the room has moved to a new epoch.

**What a member can do.** Any current member holds the room key, so a member can read all live events of the room (as for any message). Because every event is signed by its sender, a member cannot impersonate another member. A malicious member can flood within the rate limits (30 posts a second per device per room at the relay; receivers take 20 a second per sender), which costs receivers CPU and the relay's capped memory, not durable storage.

**Limits we accept.** (1) The previous epoch's key is held only in memory: a client that restarts at an epoch boundary loses at most the events of that moment. (2) A removed member who is still connected can post sealed events under an old epoch until the relay's list drops them; receivers reject them because the sender is not in the roster. (3) Versions before this channel (0.3.0 and earlier) only read the log: on a relay that has the channel, they no longer see live events from newer clients (tile drags, wall strokes). Chat, calls, colours and the wall's saved state through the usual paths are unaffected, and they get no errors. They still send live events through the log, which new clients still read.

## Hosting through Tailscale Funnel (deploy/go-online.sh)

What it adds to the above, when the relay runs on your PC and friends reach it on one public port:

* **Tailscale** runs the public entry point. It sees friends' IP addresses, the host name (TLS SNI), timing and volume. TLS ends on your PC, with a certificate only your PC holds, so it does not see the requests or the media, and those are end-to-end encrypted inside anyway.
* **The relay sees every connection as coming from 127.0.0.1**, so it logs no friend's IP address, and it cannot rate-limit per address (its limits are per account and whole-relay).
* **The front** (`--front`) routes by first bytes and parses nothing else. Only LiveKit's `/rtc` is reachable through it, never its admin API. Tested: `/twirp/...` gets the relay's 404.
* **The invite secret is required**: go-online.sh makes one and `--front` refuses to start without it. Anyone holding a link that carries it can make an account. They still cannot read a community they were not let into. It gets a stranger as far as the quotas, so keep the link private, and change it (delete `~/.local/share/commons-host/invite-secret`, go offline and online) if it leaks. Community invite links carry it too, for the same friend-sized trust.
* **What is public**: `/health`, `/v1/info` (says that a secret is needed), `/join`, `/download/`, `/update/`. The first install comes from your PC unsigned for Windows and macOS: whoever controls that PC controls what your friends first install. Updates after that need the updater key as well (below).
* **LiveKit's TURN** relays media for call members. It has to be allowed to reach its own SFU on loopback (`allow_restricted_peer_cidrs: 127.0.0.1/32`), so a call member holding a token can also send UDP to other loopback services on the PC through it. Its TCP port ignores `bind_addresses` and also listens on your LAN; it does nothing without a token's credentials. Friends only: this is another reason to keep invites private.
* **No third-party STUN**: LiveKit hands clients Google and Twilio STUN servers when TURN over UDP is off; the client strips them before any connection exists (test H17).

## Updates (docs/UPDATES.md)

* **Who can push an update:** whoever holds the updater's private key (`~/.config/commons/updater.key` and its password, on the host's PC only). The apps install a file only if its minisign signature checks against the public key built into them, and only if the signature was made for the version announced. The relay, Tailscale, or anyone on the path can therefore not slip in a changed app; with the key, they could.
* **What the relay (or whoever controls it) can still do:** withhold updates, or keep serving an old manifest. It cannot make an app go back to an older version (only newer versions are offered, and a signature names its version), and it cannot point an app at another server (the manifest and the files must come from the account's own relay; redirects are not followed).
* **What the update check tells the relay:** that an app asked, when, and which file it downloaded (so which system it runs). The request carries no account and no signature.
* **Nothing installs by itself:** the person is asked every time, and nothing happens during a call.
* **Not code-signed for the systems:** Windows SmartScreen and macOS Gatekeeper still see an unknown publisher on the first install. The update signature is Megatron' own check, on top.

## Game keys (docs/GAME-KEYS.md)

Settings live in the encrypted state of one device; which games you play never reaches the relay or another device. The helper reads only the keys you set, only during a call, and sees programs by name only. Through the call bridge the tab learns one thing: a game key is holding the mic, with that program's name (a browser extension with access to the tab could read that). On Linux, joining the `input` group, if you choose to, lets every program of yours read all keys; Megatron does not ask for it and keeps only the keys you set.

## Other adversaries

* **A malicious or compromised member**: can read everything in the groups they are in, share it, and can spam, up to the relay's per-account quota. Cannot change membership or roles without the right role (all honest clients skip the commit), and cannot change a group's relay ACL at all. An admin can remove and add people, also at the relay, including other admins there (the relay cannot tell admins apart; the owner's client puts them back); the owner is irremovable. Two members can show the same name with a modified client; everyone then sees a short account tag next to it. A removed member keeps what they already received.
* **Someone who steals an unlocked, running device or its decrypted memory**: wins everything that device could read, and also holds the **account key**, which is present on every device. They could enroll more devices for that account and revoke yours. **Revoking the stolen device does not stop this**: the key it holds is the account key, and nothing tells its new devices apart from yours. You and the members get a "new device" notice each time, and you can revoke each one, but there is a window every time. If it keeps happening, move to a new account. A trusted-device split (account key only on devices you mark) is planned and is what would close this.
* **Someone who steals a powered-off device**: needs the passphrase. Argon2id (64 MiB, t=3) slows guessing; a weak passphrase is still weak. There is no OS keychain integration or hardware key in stage 1.
* **Malware, keyloggers, screen capture, clipboard snoopers, a compromised OS**: out of scope; end-to-end encryption does not help.
* **Recovered device and communities you left**: a device made from the recovery key does not know which consents you took back, so a community owner who kept your old entry can add it again. Only communities you once joined; leave again if it happens. See DESIGN.md section 7.4.
* **Lost recovery key and all devices**: the account is gone. The recovery key is as powerful as the account.
* **Targeted attackers who obtain the invite link**: whoever has a link can *ask* to join; an admin's click is not required (links are meant to be sent privately), so use one-use links and short lifetimes (the UI defaults to one use). A one-use link admits one person even when several ask at once. Kicked users are banned by account.
* **Strangers who can reach the relay**: with an invite secret set they cannot register at all. Without one they can register, up to a rate and account limit, and use their own quota. Join requests from non-members are rate limited per device and per community; a real one can be delayed by a flood from many devices, and is sent again.

## Cryptographic and design limits to know about

* Forward secrecy and post-compromise security are those of MLS: they come from commits. A group that never changes membership keeps one epoch for a long time unless clients commit. Stage 1 does **not** schedule periodic self-update commits (only after restore, and on membership changes). Past epoch secrets are kept for 8 epochs so late messages still open.
* Local history is stored for good, encrypted at rest with the passphrase key. Deleting a message deletes it for everyone's *display*; it cannot erase copies someone made.
* "Delete" and "edit" are advisory messages: a modified client can ignore them. Authorization of who may delete is enforced when displaying, not by cryptography.
* Key package and leaf lifetimes are set to 5 years so a quiet group does not break; this trades some hygiene for reliability.
* The device key is used both to sign MLS messages and to authenticate to the relay (different, domain-separated message formats).
* Group and channel membership is a single MLS group per channel: the relay sees the same account set for each channel of a community.
* Timing and size side channels exist (padding is coarse, 256 bytes; attachment sizes are exact).
* Typing indicators are on by default for new accounts (an existing account keeps its choice; the options room turns them off); when on, they tell the relay "a packet was sent now".
* **Owner colours** reveal nothing new to the relay: they travel in the community's encrypted table (the group-context extension) like the community name, as an owner-signed document. The relay sees the same thing it sees for any table change: a commit was posted, its size, and when. Members can read the colours; only the owner's signature makes them count, so an admin or member cannot change them (they can only drop them, which the owner's device puts back). A member on an older version ignores the field.
* **Live events** (a best-effort channel for small short-lived data, carrying the camera-stack order and the shared wall) no longer travel as MLS application messages, and no longer enter the relay's log. See "The live channel" below. A relay from before the channel, or a client that finds none, falls back to the old way (ordinary padded MLS messages through the log, four a second per room); the guarantees of that path are unchanged.
* **The camera stack order and the shared wall** ride on live events, so the relay learns nothing beyond "a packet of this size was posted now by this device". Everything inside (which tile a person holds, the order, the drawing, the questions and answers) is end-to-end encrypted like a message. What members learn: who is dragging which tile (visible to people in the call), and who drew which cells (a cell carries its author's account, which is how ties are broken). On the live channel an event is not in the relay's log, so the apps may send about ten a second per room (coalesced per kind), nothing while idle, and nothing into a room where nobody is around to see it. Through the log fallback each event is a permanent log entry, so the old limits (four a second per room) apply there. The wall is kept on each device inside the encrypted state (a capped field, written on a 20 second debounce). **Lock and clear** are fields of the owner-signed style document in the community table (a set of locked rooms and a per-room wall generation), so they are durable, reach late joiners and cannot be forged: a lock makes receivers drop strokes from anyone but the owner, a clear raises the generation so strokes and snapshots of the old generation are ignored by everyone, including a device that was offline. One limit: a snapshot (a member's copy of the wall, sent to someone who just opened the room) carries per-cell authorship that only the sender vouches for, since cells are not individually signed; while a wall is locked a snapshot can therefore only fill cells that are still empty, and a hostile member could still place marks there. Older apps note "newer version" for these events and ignore them; they also read a style document that has the new fields (unknown fields are skipped, and a document without them is byte-for-byte what it was).
* No sealed-sender style anonymity; no protection of IP addresses.
* The dev bridge (`dev_bridge`) exposes the same RPC over loopback HTTP **without authentication** for tests and screenshots; it checks Host and Origin, so a web page cannot reach it, but any local process can. It is a separate, feature-gated binary and must never be exposed.
* The Tauri page keeps `withGlobalTauri` on: turning it off would change nothing, because `window.__TAURI_INTERNALS__` is there either way. The defence against script in the page is that page input is validated and escaped, and the CSP.
* Timestamps are the sender's claim, capped at five minutes ahead of the reader's clock. Order comes from the log.
* After the relay loses its data, clients say so and re-register, but groups have to be re-created by new invites: their logs are gone.

## Out of scope in stage 1

Push notifications, iOS, federation, bots, large communities (hundreds of members), moderation tooling beyond kick/ban, history transfer between devices.
