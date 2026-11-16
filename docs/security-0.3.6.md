# Security fixes in 0.3.6

What the 2026-11-10 review (`review-2026-11-10.md`, read-only, no critical or high findings) found, what 0.3.5 already
fixed, what 0.3.6 changes, how old and new versions get on, and what is still accepted. Each fix has a test that fails on
0.3.5 and passes here; the test is named under the finding.

## Already fixed in 0.3.5 (checked against the code, not the commit messages)

* **M2, wall snapshots.** `wall.rs take_wall`: on a locked wall only the owner's snapshot counts, every author a snapshot
  names must be a member of the room (or the owner), and one sender may introduce only a bounded number of new authors
  (`intro` per sender). Test: `mega4.rs` (commit e934b21).
* **M3, assistant history and taint.** `bot.rs history_from(.., Some(owner), ..)`: the local model sees only the owner's
  and the bot's lines. `RoomRec.tainted` is saved with the room and cleared only by `/reset`, not by the next message or
  a restart. `open_target` with a web address always asks, tainted or not. Tests: `assistant.rs`
  `only_the_owner_and_the_bot_reach_the_model_from_the_history`, `after_reading_outside_text_even_a_safe_action_needs_approval_until_a_fresh_message`,
  `opening_a_web_address_always_needs_a_yes_even_on_a_clean_turn` (commit 9be16d6).
* **M6, bridge secret on the command line.** `megatron_launcher.rs` and `packaging/linux-browser/megatron` open the window
  with `#t=<one-time ticket>` and the page trades it for the secret at `/session`; the secret is never an argument.
  (The module comment at the top of `megatron_launcher.rs` still describes the old `#s=` form; the code does not.)
  Checked by `tools/launcher-test.sh` (commit 2daa6e6).
* **M7, part.** The deny list covers mega's own folders, the bridge secret, gcloud, Azure, fish, shell histories; Claude
  Code gets no `DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`, `DISPLAY`, `WAYLAND_DISPLAY` or `XAUTHORITY` unless the
  owner sets `MEGATRON_BOT_ASSIST_DESKTOP_ENV`; an unparsable user `settings.json`, hooks, plugins, `statusLine`,
  `apiKeyHelper` and the AWS/OTel helpers all stop Claude from starting; approval text is stripped of bidi, zero-width
  and control characters; secret-looking file names are refused everywhere. Tests: `assistant.rs`
  `mega_shell_history_and_cloud_places_are_protected_and_user_settings_that_run_code_are_refused`,
  `claude_gets_no_session_bus_or_display_unless_asked`, `approval_messages_hide_nothing` (commits 9be16d6, 5b43d2b).

## Fixed in 0.3.6

### M1. A member could roll back or erase the owner's signed style document

*Threat.* The community table is rewritten by commits. Only the owner can sign the style document (colours, looks, wall
locks, wall generations), but any member could commit a table that carries an older owner-signed document, or none.
Devices present kept the newer one (`adopt_style` only moves forward), but a newcomer or a re-added device read the
table afresh and got the old one: walls unlocked, an old wall generation, old colours. Only the owner's device repaired
it, and only while online.

*Why the commit is still accepted.* Refusing it (`ENFORCE_STYLE_FORWARD`) would make a 0.3.6 device reject a commit that
every earlier version merges. A device that rejects a commit stays on the old epoch and cannot read the group any more:
a hostile member could split every mixed-version community with one commit. That is worse than a stale colour, so the
rule stays off until no earlier versions remain.

*Fix.* Three parts, all compatible both ways:
1. Every device keeps the exact signed text it adopted (`Community.style_text`), not only the parsed document.
2. An admin whose copy is newer than what the table shows puts that very copy back (`ops.rs reconcile_group`, the
   non-owner branch), under the owner's existing backoff (at once, then 30 s doubling to 10 min while repairs keep being
   needed). The newcomer case is closed by construction: a newcomer is admitted by an admin's commit, and that commit
   now carries the current document. Carrying a newer owner-signed document forward is allowed by every version's rules.
3. A commit by anyone but the owner that regresses or removes the document raises a notice naming the sender
   (`sync.rs style_warning`), so the owner can see who did it and remove them.

*Test.* `audit_style.rs a_rolled_back_document_is_carried_forward_by_an_admin_while_the_owner_is_away`: the owner is
away, a member puts the old document back, the admin repairs with the identical signature, a newcomer the admin admits
reads the current document (unlocked wall, current colours), a second rollback is not repaired inside the gap, and the
owner finds nothing to do when back.

*Compatibility.* No wire change. Old admins do not carry; old members behave as before. Relay not involved.

*Accepted.* A plain member's newly added device (added by the member's own old or new device, not by an admin) can read a
rolled-back table until an admin or the owner commits, which is at most the backoff (10 min) while an attacker keeps
rolling back. Each rollback still costs the attacker a commit and shows their name to everyone. The two `#[ignore]`
tests for the refusal rule stay until the rule can be turned on.

### M4. The password strength estimator over-credited ordinary words

*Threat.* The relay stores the password-derived id and the sealed invite (which holds the invite key and the relay's
sign-up secret). The operator, or anyone with a copy of the database, tests guesses offline at one Argon2id (64 MiB,
t=3) each. Words outside the 1009-word list were scored as random letters, so "sunshine-princess" passed as 37 bits
with under 30 bits of real entropy, and the three-word suggestion sat 2 bits above the floor.

*Fix.* `pwinvite.rs strength`: every run of letters counts as a dictionary word (10 bits for a short one, up to 16 for a
long one, never more than the same letters drawn at random), digits as a short number or a year, a token with four or
more runs as random characters, known passphrases ("correct horse battery staple" and the like) as nothing. `MIN_BITS`
35 to 50, `GOOD_BITS` 50 to 60, the suggestion from three words and two digits (37 bits) to five words and two digits
(56 bits). The invite screen's refusal text now says why.

*Tests.* `pwinvite.rs ordinary_words_count_as_words_not_random_letters` (the old suggestion, two-word and city-list
passwords fail; random strings and five-word phrases pass), `suggestions_pass_and_short_or_known_bad_ones_do_not`.

*Compatibility.* Client-side only. An invite a 0.3.5 app made with a 35-bit password still opens on every version.
Relay not involved.

*Accepted.* The operator can still test guesses offline; at 50 bits and a well-equipped guesser (a few thousand
Argon2id runs a second) that is thousands of years per invite, and the copy is deleted when the invite expires (30 days
at most), is used up or revoked. Making the relay unable to test guesses at all (an OPRF, or a relay-held pepper) is not
done; it would need a new relay route and a new link format.

### M5. Whole-relay lockout by anyone, and a guess oracle on the PUT route

*Threat.* Lookups are anonymous (the friend has no account yet). Forty misses in ten minutes from anyone made the relay
refuse every lookup, so one client could keep every friend from joining by password. `PUT /v1/pwinvite/{lid}` answered
409 when the id existed, so an account holder could test password guesses without touching the miss budget, and each
probe stored a row toward the 2000 cap.

*Fix.* `relay/src/lib.rs`:
* The relay knows who is calling (`client_key`): the peer address, or, when the peer is loopback or a private address
  (Caddy, tailscale serve), the last `X-Forwarded-For` entry. The header is never believed from a public peer.
* A caller's own miss budget (`pwinvite_misses_per_client`, 12 per window) refuses that caller alone, hits included.
  The whole-relay budget stays as the backstop against many addresses and is 400 (was 40): one caller cannot spend it.
* A PUT of an id held by another account answers exactly like success and stores nothing; the same account is still
  told (409) because it knows its own copies. Filing counts as a miss for the caller.
* When the table is full, the account holding the most copies (not the requester, with more than two) gives up its
  soonest-expiring copy instead of every inviter being refused.

*Tests.* `tests/pwinvite.rs one_caller_cannot_lock_everyone_out_and_filing_a_copy_is_not_an_oracle`,
`a_full_table_makes_the_heaviest_account_give_up_a_copy_rather_than_refusing_everyone`.

*Compatibility.* **Relay upgrade needed for this one.** Old clients: unchanged routes and codes; a 0.3.5 client's own
duplicate password still gets the 409 it handles. A new client against an old relay sees the old budgets.

*Accepted.* Behind a proxy that does not set `X-Forwarded-For`, every caller is the proxy and the per-caller budget
is the global one (12), which is stricter than before, not weaker; the deploy files (Caddy, tailscale) set it. Rows a
prober files are only removed by expiry; the per-account cap (20) bounds them.

### M7. Reads under `~/projects` without asking

*Threat.* The read dirs are passed as `--add-dir`, so Claude Code read anything there without the owner seeing it; the
name-based deny patterns miss a token in `.git/config` or a `config.toml`.

*Fix.* `deny.rs claude_settings` puts `Read(<read dir>/**)` and `Grep` in the `ask` list. The approver (`claude.rs
read_scope`, `handle_approval`) answers a read in Claude's own workspace at once, asks once per project folder (the read
dir's direct child that holds the path; a file right in the read dir grants the read dir's own files only) with the
folder named in the message, and a yes grants that folder for the rest of the task. Grep is asked the same way (it
shows contents); Glob is not (names only).

*Test.* `assistant.rs a_read_in_a_project_folder_is_asked_once_per_task_and_the_workspace_never`.

*Accepted.* Reads inside a granted folder are then unlogged in the room (the task log still has every call). A read dir
that is itself one project (set with `MEGATRON_BOT_ASSIST_READ_DIRS`) is granted whole on the first yes.

### M8. Direct-call authorisation rested on the founder's admin-side check; the invite secret travelled in a readable room

*Threat.* The call offer (`Envelope::Direct`) is posted in a room the two share and carried the invite's secret key. A
third member of that room could sign a join; only the founder's up-to-date admin devices refused them (`admits`), and a
device of the founder's account on an older app would have let them in. Also, a stranger sharing any room could fill
the receiver's 16 call slots with offers, and each refused join made a notice.

*Fix.* `direct.rs seal_for / open_sealed`: the invite key is sealed to the peer's account key (an ephemeral X25519 key
agreed against the peer's Ed25519 account key used as X25519, as libsodium does; ChaCha20-Poly1305 with the peer's
account in the AAD). The link in the room carries a throwaway key. A join signed with what the room shows fails the
signature check (`join_signature_ok`) on every admin device, old or new: the check is cryptographic, not policy.
`admits` stays as the second layer. A receiver holds at most two direct communities founded by one account
(`DIRECT_MAX_PER_FOUNDER`). Refused-join notices were already deduplicated by `notice()` when identical in a row.

*Tests.* `direct.rs the_invite_secret_opens_only_for_the_named_account`, `tests/direct.rs
a_third_member_who_reads_the_offer_cannot_use_its_invite` (also checks the old-peer fallback),
`one_founder_cannot_fill_the_call_slots`.

*Compatibility.* `Profile` gains `direct_sealed: true` (0.3.5 apps ignore unknown profile fields; they send none). A
caller seals only for a peer whose profile says `direct_sealed`; for a 0.3.5 peer it sends the offer as before (the key
in the link), so calls between versions keep working and the fix applies as soon as both sides are 0.3.6. A 0.3.5
receiver of a sealed offer (which cannot happen unless its profile lied) sees an invite it cannot sign; nothing happens.
Relay not involved; the envelope rides inside the usual encrypted message.

*Accepted.* A call to a 0.3.5 peer has the 0.3.5 exposure (the founder's admin devices refuse by policy). The call
community's existence (a two-person community with one voice room) is as visible to the relay as before.

### L1. Live replay table cleared by flooding

`live.rs Seen::first`: at `SEEN_MAX` (16 384) the entries older than the window go, then the oldest quarter; the table is
never emptied. Test: `live.rs a_full_replay_table_forgets_the_oldest_not_the_newest`.

### L3. Style version parked at 2^53

`state.rs adopt_style`: a device that holds a document ignores one more than `MAX_STYLE_STEP` (1 000 000) ahead; a device
with none takes any valid document, so an honest owner is never stuck. `publish_style` fails with a clear message at
the ceiling instead of signing an invalid document. Test: `state.rs a_version_that_leaps_to_the_ceiling_is_not_adopted_over_one_in_force`.

### L5. `peer_is_same_user` failed open

`bridge_auth.rs`: a connection with no `/proc/net/tcp` row is looked up four times over 20 ms and then refused; a row
whose uid does not parse is refused. An unreadable table is still let through to the secret check, which is the layer
that holds there and on macOS and Windows. Test: `bridge_auth.rs a_peer_without_a_row_is_refused`.

### L7. `open_target` extension blocklist

`tools.rs OPEN_OK`: an allowlist of document, picture, music and video extensions replaces the blocklist; `.html`, `.svg`,
`.deb`, `.ps1` and everything unlisted is refused. Test: `assistant.rs safe_actions_run_at_once_from_the_allowlist_only`
(adds `page.html` and `tool.deb`).

## Not changed

* **L2** (what the relay and members learn from presence): informational, as the review says.
* **L4** (bridge header limits, thread per connection): not done in this round; local-only exposure behind the uid check
  and the secret.
* **L6** (Mac launcher self-swap, codesign): on the `mac-launcher` branch, not on this one. Its merge note stands: the
  dotfile rule in `ui_file` must land with the carrier.
* **L8** (password shown in plain text to the inviter): intended.

## Relay and client matrix

| | old relay (0.3.5) | new relay (0.3.6) |
|---|---|---|
| old client (0.3.5) | as before | as before, with fairer lookup budgets |
| new client (0.3.6) | everything works; lookup budgets are the old global ones | everything |

Only M5 is relay-side. Nothing here changes a stored format: `Community.style_text` and `Profile.direct_sealed` are new
optional fields in the encrypted state, read as empty by an older app.
