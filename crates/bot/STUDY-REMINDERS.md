# Study reminders through Oracle (design; the bot side is a stub)

The owner's study planner (`~/projects/homebase/study`, port 4575) keeps a schedule of due reminders: for every piece of
graded work with a date, a week before, two days before and the morning it is due (the days and the hour are settings in the
planner, and a course can be muted there). The dates and weights come from the syllabus tables; no model is involved on either side.

## How a reminder gets into mega

```
planner (4575)                          megatron-bot (Oracle)                      room
  GET /api/reminders/due?since=T0  <--- every 60 s, X-Homebase-Token
  {now: T1, reminders: [...]}      --->
                                         one message ("Reminder: ..." or a list) -->  posted as Oracle
  POST /api/reminders/sent {ids}   <--- only after the room took it
                                         keep T1 as the next since
```

* The planner remembers what was sent (`reminders_sent` table), so a bot restart never repeats a reminder, and the bot keeps no
  list of its own beyond the last `since`. A reminder older than 36 hours is dropped by the planner itself, so a bot that was
  down for a day posts nothing stale.
* The bot only reads the planner and records "sent". It never changes a date, a weight or a mute.
* One message per poll: several reminders due at the same minute become one list.
* Off by default. It runs only when `MEGATRON_BOT_STUDY_ROOM=<gid>` names a room; `MEGATRON_BOT_STUDY_URL` (default
  `http://127.0.0.1:4575`) and `MEGATRON_BOT_STUDY_TOKEN_FILE` (default `~/.config/homebase/token`) complete it. The token is
  read once and never logged, like the Studio token.

## Why this shape

* **Poll, not push.** The planner runs in a strict systemd sandbox (`ProtectHome=read-only`, its own data folder only) and
  has no mega keys; the bot already keeps a client open to the relay. One loopback GET a minute is the smallest change.
* **Not a file.** The bot's data folder is private to it and the planner's sandbox cannot write there; a shared file would
  need a third place and two writers.
* **The planner owns the schedule and the "sent" list.** Mutes, days and the hour live where the student sees them (the
  planner's Settings, "Reminders in mega"), and the planner's `now` (which a check can fix with `--today`) is the clock.

## What is stubbed here

* `crates/bot/src/study.rs`: the planner's answer parsed and checked, the one-message rule, the `sent` body, the settings
  from the environment, an `HttpStudy` client for exactly the two routes, and `poll_once` (ask, post, record), with unit tests
  on fakes. Not added to `lib.rs` and not called from `main.rs`, so the running bot is unchanged.

## To wire it in (not done)

1. `pub mod study;` in `lib.rs`.
2. In `main.rs::serve`, after the GPU thread: if `study::settings(&home(), env_string)` is `Some`, spawn a thread that every
   `study::POLL_EVERY` calls `study::poll_once(&api, &mut |text| SharedClient(&shared).send(&room, text, None).map(|_| ()), &since)`
   and keeps the returned `since`; log a failure once per change, like the GPU watcher does.
3. Respect the room's `oracle_quiet` look: when the owner has switched Oracle off in that room, skip the post and do not record it as sent.
4. A `study_reminders_tests` integration test with a fake planner (an `httptest`-style local server, or a `StudyApi` fake) and the
   relay test harness, like `tests/` does for Studio.
