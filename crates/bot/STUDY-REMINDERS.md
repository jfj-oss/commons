# Study reminders through Oracle (design; the bot side is a stub)

The owner's study planner (`~/projects/homebase/study`, port 4575) keeps a schedule of due reminders: for every piece of
graded work with a date, a week before, two days before and the morning it is due (the days and the hour are settings in the
planner, and a course can be muted there). The dates and weights come from the syllabus tables; no model is involved on either side.

## How a reminder gets into mega

```
planner (4575)                          megatron-bot (Oracle)                      room
  GET /api/reminders/due?since=T0  <--- every 60 s, X-Homebase-Token (the reminders-only token)
  {now: T1, reminders: [...]}      --->
                                         one message ("Reminder: ..." or a list) -->  posted as Oracle
  POST /api/reminders/sent {ids}   <--- only after the room took it
                                         keep T1 as the next since
```

* The planner remembers what was sent (`reminders_sent` table), so a bot restart never repeats a reminder, and the bot keeps no
  list of its own beyond the last `since`. A reminder older than 36 hours is dropped by the planner itself, so a bot that was
  down for a day posts nothing stale. The 36 hours run across midnight: the "due today, 8 am" reminder of a thing due on Friday is
  still posted on Saturday morning (worded "was due Fri Nov 13", not "is due today"), and gone by Saturday 8 pm.
* **Delivery is at least once.** The bot posts, then tells the planner `sent`. If the post worked and the `sent` call fails (the
  planner restarts at that moment), the next poll offers the same reminders and they are posted again. A double reminder is
  accepted; a lost one is not.
* **The room must be private.** Reminders are posted as Oracle in one room, and everyone in that room sees the student's
  deadlines and the weight of each piece of work. Name a room that holds only the student (and the bot).
* **The planner filters.** Work worth less than 3% (a setting in the planner, "Skip work worth less than") gets no reminder,
  so ten weekly 1.5% quizzes do not make thirty messages; a kind can also be switched off there.
* The bot only reads the planner and records "sent". It never changes a date, a weight or a mute.
* One message per poll: several reminders due at the same minute become one list.
* Off by default. It runs only when `MEGATRON_BOT_STUDY_ROOM=<gid>` names a room; `MEGATRON_BOT_STUDY_URL` (default
  `http://127.0.0.1:4575`) and `MEGATRON_BOT_STUDY_TOKEN_FILE` (default `~/.config/homebase/study-reminders-token`) complete it.
  The token is read once and never logged, like the Studio token.
* **A token of its own.** The bot never holds the shared homebase token (read and write on every homebase app). It gets a
  separate file, `study-reminders-token`, that the planner accepts only for `GET /api/reminders/due` and
  `POST /api/reminders/sent`; every other route answers 403 to it. The planner only reads that file, so whoever installs it
  creates it (`install -m 600`), see the planner's `INSTALL` notes.

## Why this shape

* **Poll, not push.** The planner is meant to run in a strict systemd sandbox (`ProtectHome=read-only`, its own data folder
  only; the unit in the repo has it, check that the installed one does too) and has no mega keys; the bot already keeps a client open to the relay. One loopback GET a minute is the smallest change.
* **Not a file.** The bot's data folder is private to it and the planner's sandbox cannot write there; a shared file would
  need a third place and two writers.
* **The planner owns the schedule and the "sent" list.** Mutes, days and the hour live where the student sees them (the
  planner's Settings, "Reminders in mega"), and the planner's `now` (which a check can fix with `--today`) is the clock.

## What is stubbed here

* `crates/bot/src/study.rs`: the planner's answer parsed and checked, the one-message rule, the `sent` body, the settings
  from the environment, an `HttpStudy` client for exactly the two routes (with the reminders-only token), and `poll_once`
  (ask, post, record), with unit tests on fakes. Compiled and tested as a module of the library (`pub mod study;`), but not
  called from `main.rs`, so the running bot is unchanged.

## To wire it in (not done)

1. (`pub mod study;` is already in `lib.rs`.)
2. In `main.rs::serve`, after the GPU thread: if `study::settings(&home(), env_string)` is `Some`, spawn a thread that every
   `study::POLL_EVERY` calls `study::poll_once(&api, &mut |text| SharedClient(&shared).send(&room, text, None).map(|_| ()), &since)`
   and keeps the returned `since`; log a failure once per change, like the GPU watcher does.
3. Respect the room's `oracle_quiet` look: when the owner has switched Oracle off in that room, skip the post and do not record it as sent.
4. A `study_reminders_tests` integration test with a fake planner (an `httptest`-style local server, or a `StudyApi` fake) and the
   relay test harness, like `tests/` does for Studio.
