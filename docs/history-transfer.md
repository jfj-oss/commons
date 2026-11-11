# History on a new device

A device added to an account (a phone, a laptop) joins every group at the current epoch. MLS gives it no past messages,
and until now the app said so plainly ("the new device does not get old history"). From 0.3.6 a device the account
already uses can hand its room history to the new device, end to end encrypted, after the person says yes on that old
device. The relay carries ciphertext it cannot open, the new device gets only what the old device chose to send, and
nothing changes for anyone else in the rooms.

## What today's pieces are

* **Devices.** Every device of an account holds the account key (Ed25519) and its own device key (Ed25519). A device is
  known by its account-signed `DeviceCert`; the relay lists the account's devices (`/v1/devices/{account}`) and knows
  which are revoked. Each device is its own MLS leaf in every group of the account.
* **Pairing.** The new device shows a code (`commons-pair:` with a rendezvous id and a one-time X25519 key); the old
  device seals the account key to that key and leaves it in the relay's rendezvous mailbox; the new device picks it up,
  registers, writes its device id into the mailbox's second slot, and the old device (any device of the account, in its
  next reconcile) adds the new leaf to every group. The old device never learns the new device's id from the mailbox;
  it sees it in the account's device list, with a "new device added" notice.
* **Epochs and keys.** Each group ratchets per commit; a new leaf can decrypt from its Welcome's epoch on. Past
  ciphertext in the relay's log is unreadable to it, by design, and the relay never holds plaintext or keys.
* **History.** Each device keeps every room's messages in its encrypted state (`State.chans[gid].msgs`, a `Msg` per
  message: id, sender, device, time, log position, body, reply, edits, deletions, reactions). A file message's body
  holds the blob id and the blob's key and nonce; the bytes sit on the relay as an encrypted blob (large files in
  chunks) and are fetched when opened.
* **The device inbox.** The relay keeps a per-device inbox (`POST /v1/welcomes/{device}`, read in `/v1/sync`, deleted
  on acknowledgement) for things a device must receive without a group: Welcomes and join replies. Any registered
  device may post to any unrevoked device; items up to 1 MiB, 500 items or 64 MiB per inbox.

## Threat model

* **A malicious relay.** It carries every byte of the transfer and may read, keep, drop, reorder, replay or inject.
  It must learn nothing about the messages (not their count per room, as far as padding allows, not who wrote them),
  must not be able to make the new device accept history that no device of the account sent, and must not be able to
  make either device believe the transfer completed when it did not.
* **A stolen new device.** Someone who paired a device into the account, or stole the new device after pairing. The
  history must reach a new device only after a person said yes on a device that already has the history; a request
  from a device the person did not approve must sit visibly until they do. Once a device is revoked, nothing more goes
  to it and what sits undelivered in its inbox cannot be fetched (the relay refuses a revoked device).
* **A member of the room.** Other members must learn nothing: not that a transfer happened, not its size. They have the
  messages themselves anyway; what they must not get is a copy of messages from rooms they are not in, or of this
  account's unread state.
* **The old device's own exposure.** A stolen *old* device that has not been revoked can pair new devices and approve
  anything, including this. That is the existing trust: whoever holds an unrevoked device holds the account. Nothing
  here widens it.

## The protocol

Three message kinds travel through the device inbox. None is a group message: no group, no epoch, no other member
involved. All are new in 0.3.6; see compatibility for what older apps do with them.

### 1. Request (new device to each other device of the account)

Right after the pairing completes (`pairing_apply`), and again if nothing has arrived after a restart, the new device
posts to every other unrevoked device of its account one small JSON item:

```
{"t":"hist-req","v":1,"community":<sha256 of the device, hex>,"reason":"hist-req","cert":<its DeviceCert>,"ts":<seconds>,"sig":<hex>}
sig = device key over "commons/history-req/v1" | account | device | ts
```
(`community` and `reason` only make a 0.3.5 app read it as a join reply it can drop in silence; see compatibility.)

The old device accepts a request only if: the cert verifies under the account key and is for this account; the device
is not revoked; the signature verifies under the cert's device key; the time is within ten minutes. It then shows the
request in the Devices screen ("a new device asks for this device's message history") until the person presses
**Send** or **Not now**. If the person ticked "also send my history" when approving a pairing in the last fifteen
minutes, and the request comes from a device whose cert was issued after that approval, it is sent without asking
again: that is the same yes, given a moment earlier.

### 2. Chunks (old device to the new device)

The old device builds one export of every room it is in (text rooms and voice rooms alike; only rooms it has left or
been removed from are skipped): for each room its `Msg` list in log order plus how many at the end were unread. The
export is cut into chunks of at most 512 KiB of plaintext; a room larger than that is cut into several slices, each a
contiguous run of messages. If the whole export is over 64 MiB the oldest messages are left out, room by room from the
oldest, and the count of what was left out travels with the chunks. Each chunk is:

```
"HIST1" || JSON {
  "tid":  <transfer id, 16 bytes hex>,
  "i": <chunk index>, "n": <chunk count>,
  "cert": <the sending device's DeviceCert>,
  "blob": <base64 of eph_pk(32) | nonce(12) | ChaCha20-Poly1305 ciphertext>,
  "sig":  <hex, sending device key over "commons/history-chunk/v1" | tid | i | n | sha256(blob)>
}
```

The ciphertext is the chunk's JSON (`{"v":1,"tid","i","n","omitted","slices":[{gid, community, unread, msgs:[Msg...]}]}`),
sealed to the new device's **device** key: an ephemeral X25519 key agreed against the new device's Ed25519 device key
used as X25519 (the way libsodium's `crypto_sign_ed25519_pk_to_curve25519` does; the same construction 0.3.6 uses for
direct-call offers, `seal.rs`), key = SHA-256("commons/seal/v1", eph_pk, recipient_x, shared), AAD = "commons/history/v1"
| tid | i | n | recipient device. The account key alone does not open it: only the one device named does, so a copy of
the account key from a backup, or another device of the account, learns nothing from the relay's copy. Each chunk is
padded to a multiple of 4 KiB before sealing, so the relay sees sizes in 4 KiB steps.

The sender queues the chunks in its saved state (`State.history_out`) and feeds them to the inbox a few at a time
(at most four in flight), through the same retried queue Welcomes use, so a restart on either side resumes. An
"inbox full" answer pauses the feed for a while. A 404 for the device (revoked) drops everything left for it.

### 3. What the new device does with a chunk

An inbox item that starts with `HIST1` is a chunk (MLS messages start with a version number, join replies and requests
with `{`). The new device checks: the cert verifies and is for its own account and is not revoked; the signature
verifies under that device key over the exact bytes; the chunk opens with its device key (the AAD binds the transfer
id, index, count and this device). It then merges each slice into its room log: messages it already has (by id) are
left alone, the others are inserted and the log keeps relay order (`seq`); the unread count becomes the larger of what
it had and what the old device had. A room's log can exist before the room's Welcome arrives, so order of arrival
does not matter. Progress (`got` of `n`) is kept in the state and shown; when every chunk is in, a notice says how many
messages arrived and how many, if any, the old device left out. Chunks are acknowledged after they are stored, so the
relay deletes them; a chunk that fails any check is acknowledged too (dropped) and noted once.

Files: nothing is transferred. A file message carries its blob's id, key and nonce as before; the new device fetches the
blob from the relay when the file is opened, exactly as any member does. Blobs stay on the relay until the account
deletes them; large files expire after seven days on the relay, and a file whose bytes are gone shows as unavailable,
as it would on the old device.

### Resumability

Every chunk is independent: it names the transfer, its index and the count, and merging is idempotent. The sender's
queue and the receiver's progress are both in saved state. The relay keeps unacknowledged items; the receiver
acknowledges only after saving. A sender restart continues feeding; a receiver restart re-reads what it did not
acknowledge. A new request from the same device while a transfer is in flight is answered with the same transfer (the
request is ignored while `history_out` has chunks for that device). A request for a second time after a completed
transfer (the new device lost its state and paired again) is a new request and is shown again.

### What the relay learns

That device A posted k items of these sizes to device B's inbox over some minutes, shortly after B registered. It
cannot tell them from Welcomes except by size. It learns nothing of rooms, counts per room or senders.

## UX

**Old device, approving.** The Devices screen (the devices button at the bottom left; the same screen where a pairing
code is pasted and approved) gets a tick box under the code: "Also send this device's message history to it", on by
default. Pressing **Approve new device** with it on is the yes; a request arriving in the next fifteen minutes from the
device that pairing produced is answered at once. A request that arrives any other way (the box was off, or the new
device paired through another of your devices) appears at the top of the same screen: "Your new device 3f9a… asks for
this device's message history (N messages in M rooms). **Send** / **Not now**", and the activity dot lights. Sending
shows "Sending history to 3f9a…: 4 of 12 parts" there until done. No new menu, no new screen.

**New device, filling.** The room list shows one note at the top, in the place join notes already use: "Your other
device is sending this device's message history: 4 of 12 parts." Rooms open and work meanwhile; messages appear in
place as chunks land. When complete: "History from your other device arrived: N messages." with Dismiss; if the old
device left messages out: "... the oldest K messages stayed on the other device." If no other device answers within a
few minutes, the note says "No other device has answered yet. Open mega on a device you already use and press Send
under Devices." and stays until dismissed.

## Compatibility with older clients

* **Old device on 0.3.5, new device on 0.3.6.** The request lands in the old app's inbox as a JSON item, which such an
  app reads as a reply to a join request. It is shaped as a well-formed one (a `community` it is not joining, a
  `reason`, the cert and a 64-byte signature), so the old app drops it in silence: no notice, no problem indicator.
  Nothing else happens; the new device shows the "no other device has answered yet" note. Updating the old device and pressing Send under Devices completes it later (the new device asks again on each
  start while it has nothing).
* **Old device on 0.3.6, new device on 0.3.5.** The old device never sends chunks unprompted, and a 0.3.5 device never
  asks, so nothing is sent and nothing is shown. (Had a chunk reached a 0.3.5 inbox it would be refused as a malformed
  Welcome and acknowledged, one notice.)
* **Relay.** No relay change: the inbox route, its limits and its delivery in `/v1/sync` are those of 0.3.4 and later.
  The sender respects the inbox's 1 MiB item limit and backs off on "inbox full".
* **State.** `State.history_out`, `history_in`, `history_requests` and `history_preapproved` are new optional fields; an
  older app reading the state ignores them.

## Accepted

* The old device must be online and unlocked for the transfer; the person has to press Send there (or have ticked the
  box when approving). There is no server-side copy to pull from, on purpose.
* Only one device's history travels: the one where the person said yes. Rooms that device is not in are not covered.
* Read state travels as a count of unread messages per room, nothing finer.
* A transfer over 64 MiB is cut at the oldest messages.
* Reactions and edits that arrive on the new device for a message it has not yet received from the chunks are kept as
  orphans (the existing mechanism) and attached when the chunk lands.
