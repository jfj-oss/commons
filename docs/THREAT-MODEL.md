# Threat model

**This software has had no independent security audit.** It is a stage 1 build for a small group of friends. It uses a well-known MLS implementation (OpenMLS) and RustCrypto/dalek primitives and implements no cryptography itself, but the way the pieces are combined (device certificates, the relay protocol, authorization rules, recovery) is new code that nobody outside this project has reviewed. Treat it as "much better than nothing, not yet proven".

## What we protect, against whom

| Asset | Protected against | By |
| --- | --- | --- |
| Message text, edits, replies, reactions, attachment bytes, file names and types | The relay operator and anyone who breaks into the relay; network observers (add TLS on top, see DEPLOY.md) | MLS application messages (AEAD per sender key), attachments encrypted client-side before upload |
| Community names, channel names, roles, display names, invite settings | Same | They live only inside MLS group-context extensions and MLS messages |
| Who may speak or change membership | A malicious member, a malicious relay | Signed MLS commits; every client enforces the same group-local rules and skips commits that break them |
| Later messages after someone is removed or a device is revoked | The removed member / lost device | Commit with a path: new epoch secrets they cannot derive. Tested by feeding the removed client the whole relay log |
| Identity of a friend | A relay or stranger impersonating them | Account key = account id; device certificates signed by it; safety numbers for out-of-band verification |
| Keys and history on a stolen powered-off laptop | A thief with the disk | Argon2id + XChaCha20-Poly1305 over the whole state; side files sealed with the same key |
| Message integrity and ordering within a group | A relay that reorders, duplicates, withholds | Strict `seq` processing, gap detection, MLS epoch and generation checks, exactly-once ids. A relay can still *delay or drop*, and the client says so |

## What the relay can see (exactly)

The relay is treated as hostile. This is everything it stores or observes. Verified by `relay_holds_no_plaintext` (greps all relay files for message text, file names/contents, channel/community/display names) and by the table list in that test.

**Stored (tables `devices`, `keypackages`, `group_members`, `entries`, `welcomes`, `rendezvous`, plus a `blobs/` directory):**

* **Device registry**: for each device its public key, its account's public key, the certificate (including its creation time), and whether and when (signature only) it was revoked. So the relay knows which devices belong to one account.
* **Key packages**: opaque to it, but they contain the same certificate and public keys. It knows how many are left per device.
* **Group membership list (ACL)**: for each random group id, the set of *accounts* allowed to read and write it. So it learns the social graph at account level: which accounts share groups, group sizes, when members are added and (after the grace period) removed. It does not know names or purposes. It can usually cluster groups into "communities" because the same set of accounts appears in several groups.
* **Log entries**: `(group id, seq, client random id, tag byte, ciphertext)`. The ciphertext size is padded to a multiple of 256 bytes by MLS. The tag tells it: application message, commit (a membership/epoch change), rejoin request, join request.
* **Welcome inbox**: for each recipient device, MLS Welcome ciphertext, deleted when acknowledged.
* **Join requests** (the only plaintext JSON it holds): invite id (random), the joiner's certificate (account and device keys), and a signature. So it knows *which account asked to join which group id*. The invite secret is never sent.
* **Rejoin requests** (plaintext): the device's certificate, a nonce, a signature. It learns that a device lost state.
* **Blobs**: ciphertext, exact size, random id. It knows which device uploaded and which devices downloaded each blob, and when, but not which message or channel it belongs to.
* **Rendezvous** (device pairing): a sealed box it cannot open, for at most 15 minutes, under a random id it only learns from the request itself.

**Observed, not stored:** IP addresses and timing of every request (inherent to any server; use a VPN/Tor if that matters), and **which device posted each entry** because requests are signed by the device key. Message *timing and size* is visible; who is talking to whom inside a group is *not* visible beyond "this device posted this entry" (MLS hides the sender inside the ciphertext, but the HTTP request does not).

**Not visible to it:** message text; attachment contents, names, types; community, channel and display names; roles and admin lists; invite limits; who reacted or edited; which entries are replies; typing indicators (when enabled they are ordinary padded messages); read state (there are no read receipts); search queries (local).

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
| Make a client believe it was removed | Possible ("denied"); the client labels it as unconfirmed and keeps its local state. |
| Lie in `group_members` ACL | Gives it nothing to gain beyond what it already has; it can only cause denial of service. |

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

## Other adversaries

* **A malicious or compromised member**: can read everything in the groups they are in, share it, and can spam. Cannot change membership or roles without the right role (all honest clients skip the commit). An admin can remove and add people; the owner is irremovable. A removed member keeps what they already received.
* **Someone who steals an unlocked, running device or its decrypted memory**: wins everything that device could read, and also holds the **account key**, which is present on every device. They could enroll more devices for that account and revoke yours. Members see a "new device" notice, and you can revoke from another device, but there is a window. A trusted-device split (account key only on devices you mark) is planned.
* **Someone who steals a powered-off device**: needs the passphrase. Argon2id (64 MiB, t=3) slows guessing; a weak passphrase is still weak. There is no OS keychain integration or hardware key in stage 1.
* **Malware, keyloggers, screen capture, clipboard snoopers, a compromised OS**: out of scope; end-to-end encryption does not help.
* **Lost recovery key and all devices**: the account is gone. The recovery key is as powerful as the account.
* **Targeted attackers who obtain the invite link**: whoever has a link can *ask* to join; an admin's click is not required (links are meant to be sent privately), so use one-use links and short lifetimes (the UI defaults to one use). Kicked users are banned by account.

## Cryptographic and design limits to know about

* Forward secrecy and post-compromise security are those of MLS: they come from commits. A group that never changes membership keeps one epoch for a long time unless clients commit. Stage 1 does **not** schedule periodic self-update commits (only after restore, and on membership changes). Past epoch secrets are kept for 8 epochs so late messages still open.
* Local history is stored for good, encrypted at rest with the passphrase key. Deleting a message deletes it for everyone's *display*; it cannot erase copies someone made.
* "Delete" and "edit" are advisory messages: a modified client can ignore them. Authorization of who may delete is enforced when displaying, not by cryptography.
* Key package and leaf lifetimes are set to 5 years so a quiet group does not break; this trades some hygiene for reliability.
* The device key is used both to sign MLS messages and to authenticate to the relay (different, domain-separated message formats).
* Group and channel membership is a single MLS group per channel: the relay sees the same account set for each channel of a community.
* Timing and size side channels exist (padding is coarse, 256 bytes; attachment sizes are exact).
* Typing indicators are off by default; when on, they tell the relay "a packet was sent now".
* No sealed-sender style anonymity; no protection of IP addresses.
* The dev bridge (`dev_bridge`) exposes the same RPC over loopback HTTP **without authentication** for tests and screenshots; it is a separate, feature-gated binary and must never be exposed.

## Out of scope in stage 1

Push notifications, iOS, federation, bots, large communities (hundreds of members), moderation tooling beyond kick/ban, ownership transfer, history transfer between devices.
