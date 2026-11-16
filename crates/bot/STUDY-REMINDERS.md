# Study reminders through Oracle

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

## What is built

* `crates/bot/src/study.rs`: the planner's answer parsed and checked, the one-message rule, the `sent` body, the settings
  from the environment, an `HttpStudy` client for exactly the two routes (with the reminders-only token), `poll_once`
  (ask, post, record) and `Runner` (one tick a minute: skips a room the owner made quiet without recording anything, keeps the
  planner's clock as the next `since` only after a whole good round). `main.rs::serve` runs it on its own thread when
  `MEGATRON_BOT_STUDY_ROOM` is set; the thread logs a post, and a failure or quiet room once per change.
* `megatron-bot rooms` lists the room ids (and "community / room" names), to find the value for `MEGATRON_BOT_STUDY_ROOM`. Run it on a
  COPY of the data folder so the running bot's folder is not opened twice.
* Tests: unit tests in `study.rs`, and `tests/study.rs` (a fake planner that behaves like the real one, a throwaway relay and real
  accounts): posted once and recorded, not reposted after a restart, stale and muted left out, wrong or missing token and a planner
  that is down post nothing, a quiet room gets nothing and nothing is recorded.

## Install (see the hand-off notes)

1. Planner side: `~/.config/homebase/study-reminders-token` (mode 600, a long random string). The planner reads it on every request, no restart.
2. Bot side: build, swap the binary, add `Environment=MEGATRON_BOT_STUDY_ROOM=<room id>` to `megatron-bot.service`, restart the service.
