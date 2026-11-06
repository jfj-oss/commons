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

Voice, video, screen share, push notifications, iOS, federation, bots, large communities (hundreds of members), moderation tooling beyond kick/ban, ownership transfer, history transfer between devices.
