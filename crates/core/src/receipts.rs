//! Read receipts: "I have read this room up to here", told to the room's other members.
//!
//!  * What travels. `Envelope::Read { upto }`, where `upto` is a message's id. It goes through the room's own MLS group like a
//!    typing hint, so it is end-to-end encrypted, padded to the same size as any short message, and the relay learns only that
//!    one more small message was posted. It is in the room's log, so somebody who was away sees it when they come back.
//!  * When. Only when the person looked at the room (the page's `mark_read`, which it calls with the room in view), only while
//!    "Send read receipts" is on, only forward (never for a message older than the last receipt), and at most one per room every
//!    `READ_GAP_MS`: a receipt that comes sooner waits, and only the newest one waiting is sent.
//!  * What is kept. Per room and account, the newest message that account said it has read (`ChannelLog::read_by`, saved with the
//!    history). A receipt only ever moves that forward.
//!  * Off. Somebody who turns the switch off sends none and is shown nobody else's (`read_marks` answers nothing). The second half
//!    is this app's own good manners, not something the others can enforce.
//!  * Bots. A bot marks rooms read through `Client::mark_read`, which sends nothing; only `note_read` does, and the bot also turns
//!    the switch off for itself.
use crate::client::*;
use crate::envelope::*;
use crate::state::*;
use std::collections::HashMap;

/// The shortest pause between two receipts for one room.
pub const READ_GAP_MS: u64 = 3000;
/// Most names one mark carries to the page (which shows fewer and counts the rest).
pub const READ_NAMES_MAX: usize = 60;

fn seq_of(l: &ChannelLog, id: &str) -> Option<u64> { l.msgs.iter().find(|m| m.id == id).map(|m| m.seq).filter(|s| *s > 0) }
fn shown(m: &Msg) -> bool { !m.deleted && !matches!(m.body, MsgBody::Notice(_)) }

impl Client {
    pub fn read_receipts_on(&self) -> bool { !self.st.read_receipts_off }
    /// The "Send read receipts" switch. Turning it off also takes back every receipt that has not left yet.
    pub fn set_read_receipts(&mut self, on: bool) {
        self.st.read_receipts_off = !on;
        if !on {
            for l in self.st.chans.values_mut() { l.read_want = None; }
            self.st.outbox.retain(|i| !matches!(i.env, Envelope::Read { .. }));
        }
        self.activity += 1;
    }
    /// The person has this room in view and has read it. Sends a receipt for the newest delivered message from somebody else,
    /// if that is further than the last one sent; right away, or after the pause if one went a moment ago.
    pub fn note_read(&mut self, gid: &str) { self.note_read_at(gid, crate::spotify::now_ms()) }
    /// `note_read` with the time (in ms) given, so the pause can be tested without waiting for it.
    pub fn note_read_at(&mut self, gid: &str, now_ms: u64) {
        if self.st.read_receipts_off || self.check_chan(gid).is_err() { return; }
        let me = self.my_account();
        let Some(l) = self.st.chans.get_mut(gid) else { return };
        let Some(newest) = l.msgs.iter().filter(|m| m.seq > 0 && !m.sender.is_empty() && m.sender != me && !matches!(m.body, MsgBody::Notice(_))).max_by_key(|m| m.seq) else { return };
        let (id, seq) = (newest.id.clone(), newest.seq);
        let at = |x: &Option<String>| x.as_deref().and_then(|i| seq_of(l, i)).unwrap_or(0);
        if seq <= at(&l.read_sent) || seq <= at(&l.read_want) { return; }
        l.read_want = Some(id);
        self.flush_reads_at(now_ms);
    }
    pub(crate) fn flush_reads(&mut self) { self.flush_reads_at(crate::spotify::now_ms()) }
    /// Send the receipts that are waiting and whose room has had its pause.
    pub fn flush_reads_at(&mut self, now_ms: u64) {
        let waiting: Vec<String> = self.st.chans.iter().filter(|(_, l)| l.read_want.is_some()).map(|(g, _)| g.clone()).collect();
        for gid in waiting {
            let gone = self.st.read_receipts_off || self.check_chan(&gid).is_err();
            // (a clock that went backwards must not hold a receipt for ever)
            if !gone && self.read_sent_at.get(&gid).is_some_and(|t| now_ms >= *t && now_ms < *t + self.read_gap_ms) { continue; }
            let Some(l) = self.st.chans.get_mut(&gid) else { continue };
            let Some(id) = l.read_want.take() else { continue };
            if gone { continue; }
            l.read_sent = Some(id.clone());
            self.read_sent_at.insert(gid.clone(), now_ms);
            // one that never left (offline, keys not ready) is replaced, not followed
            self.st.outbox.retain(|i| !(i.gid == gid && i.cipher.is_none() && matches!(i.env, Envelope::Read { .. })));
            self.queue_env(&gid, Envelope::Read { upto: id });
        }
    }
    /// Somebody's receipt. It is that account's claim about itself and nothing more: it must come from a member of the room, and
    /// it only moves that account's place forward. Kept whether or not our own switch is on (the switch decides what is shown).
    pub(crate) fn take_read(&mut self, gid: &str, acct: &str, upto: String) {
        let Some(g) = self.st.groups.get(gid).filter(|g| !g.removed && g.accounts.contains(acct)) else { return };
        let l = self.st.chans.entry(gid.to_string()).or_default();
        l.read_by.retain(|a, _| g.accounts.contains(a));
        let new = seq_of(l, &upto);
        match l.read_by.get(acct) {
            Some(old) if *old == upto => return,
            Some(old) => match (seq_of(l, old), new) {
                (Some(o), Some(n)) if n <= o => return,   // backwards
                (Some(_), None) => return,                // a message we do not have, against one we do
                _ => {}
            },
            None => {}
        }
        l.read_by.insert(acct.to_string(), upto);
        self.activity += 1;
    }
    /// For the page: of the rows it is about to get, which one each other member has read up to, as `message id -> names`. A
    /// place on a message that shows no row (deleted since) moves to the nearest row before it; a place above these rows is not
    /// shown at all. Nothing when our own receipts are off.
    pub fn read_marks<'a>(&self, gid: &str, rows: &[&'a Msg]) -> HashMap<&'a str, Vec<String>> {
        let mut out: HashMap<&'a str, Vec<String>> = HashMap::new();
        if self.st.read_receipts_off { return out; }
        let (Some(l), Some(g)) = (self.st.chans.get(gid), self.st.groups.get(gid)) else { return out };
        if l.read_by.is_empty() { return out; }
        let me = self.my_account();
        let at: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, m)| (m.id.as_str(), i)).collect();
        for (acct, id) in &l.read_by {
            if *acct == me || !g.accounts.contains(acct) { continue; }
            let Some(&i) = at.get(id.as_str()) else { continue };
            let Some(row) = rows[..=i].iter().rev().find(|m| shown(m)) else { continue };
            out.entry(row.id.as_str()).or_default().push(self.display_name(acct));
        }
        for names in out.values_mut() { names.sort_by_key(|n| n.to_lowercase()); names.truncate(READ_NAMES_MAX); }
        out
    }
}
