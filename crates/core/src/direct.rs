//! One-to-one calls. A direct call lives in a hidden two-person community: the existing machinery (a control group, one
//! voice room, calls, tiles, rings) already keeps a group private to its members, so nothing about groups or the relay
//! changes. This file is only what makes such a community recognisable and safe to join on someone's word.
//!
//! The marker. A community's nonce is 32 hex characters and the community id is `sha256(founder, nonce)`. A direct
//! community's nonce is `salt (16 hex) | tag (16 hex)`, where `tag = sha256("commons/direct/v1", founder, peer, salt)[..8]`.
//! So the id itself commits to exactly two accounts: the founder and one named peer. Nobody can make a community that
//! a third person can be admitted to and still have it count as "the direct community of founder and peer", because the
//! tag does not verify for anybody else, and nobody who lacks the founder's key can make a community under the founder's id.
//! To anyone who does not know the peer the nonce is as random as any other (the relay never sees nonces at all), and an older
//! app has no idea there is a marker: to it this is an ordinary two-person community with one voice room.
//!
//! What the receiver of a call checks before it joins anything is [`offer_check`]; what it checks of the community once it is in,
//! on every change, is [`mark`] (and it leaves a direct community that stops having exactly this shape). What an admin checks
//! before letting a joiner into a direct community is [`admits`]: only the peer.
use crate::browser_update::now_secs;
use crate::client::Client;
use crate::state::*;
use anyhow::{anyhow, bail, Result};
use crate::envelope::*;
use commons_proto::sha256;
use std::collections::BTreeSet;

/// What an older app calls the community: it has no notion of "direct", and must not look like a stray server that means nothing.
pub const DIRECT_NAME: &str = "Direct call (open in a newer Megatron)";
/// The one voice room of a direct community.
pub const DIRECT_ROOM: &str = "call";
/// How long the invite inside a call offer lives.
pub const DIRECT_INVITE_TTL: u64 = 600;
/// Most call offers taken from one sender per minute, most automatic joins per hour (from anybody), most direct communities held.
pub const DIRECT_PER_MIN: usize = 2;
pub const DIRECT_JOINS_PER_HOUR: usize = 6;
pub const DIRECT_MAX: usize = 16;
/// How long a caller waits for the other person to show up in the room before the ring is given up (then it rings no one).
pub const DIRECT_WAIT: u64 = 45;

fn tag(founder: &str, peer: &str, salt: &str) -> String {
    hex::encode(&sha256(&[b"commons/direct/v1", founder.as_bytes(), peer.as_bytes(), salt.as_bytes()])[..8])
}
/// A fresh nonce for the direct community of `founder` and `peer`.
pub fn new_nonce(founder: &str, peer: &str) -> String {
    let salt = crate::client::rand_hex(8);
    format!("{salt}{}", tag(founder, peer, &salt))
}
/// Does this nonce commit to exactly `founder` and `peer`?
pub fn binds(founder: &str, nonce: &str, peer: &str) -> bool {
    nonce.len() == 32 && nonce.is_ascii() && tag(founder, peer, &nonce[..16]) == nonce[16..]
}

/// The two-person shape a direct community must keep: nobody but the founder and the peer, and at most one room, a voice room.
pub fn shape_ok(founder: &str, peer: &str, accounts: &BTreeSet<String>, channels: &[ChanMeta]) -> bool {
    accounts.iter().all(|a| a == founder || a == peer) && channels.len() <= 1 && channels.iter().all(|c| c.kind == ROOM_VOICE)
}

/// Is this a direct community, and is it still one? Returns the other person (from `me`'s side: the founder, or the peer when
/// `me` founded it) and whether it has gone wrong. `known` is the peer this device itself chose when it founded the community
/// (it cannot be read from the members before they join); `accounts` are the current members.
pub fn mark(founder: &str, nonce: &str, me: &str, accounts: &BTreeSet<String>, channels: &[ChanMeta], known: Option<&str>) -> (Option<String>, bool) {
    let mut cands: BTreeSet<&str> = accounts.iter().map(String::as_str).filter(|a| *a != founder).collect();
    if let Some(k) = known { cands.insert(k); }
    if founder != me { cands.insert(me); }
    let Some(peer) = cands.into_iter().find(|a| binds(founder, nonce, a)) else { return (None, false) };
    let bad = !shape_ok(founder, peer, accounts, channels) || (founder != me && peer != me);
    (Some(if founder == me { peer.to_string() } else { founder.to_string() }), bad)
}

/// An admin's rule for a join request to a community: in a direct community only the named peer is let in, and a community
/// that looks like one (it has the name this app gives them) is held to the same, so that a device which does not yet know
/// who the peer is (a second device of the founder, before the peer has joined) refuses everyone it cannot verify.
/// `Some(false)` = a third person, refuse; `None` = not a direct community at all (the ordinary rules apply).
pub fn admits(name: &str, marked: bool, founder: &str, nonce: &str, joiner: &str) -> Option<bool> {
    if marked || name == DIRECT_NAME { Some(binds(founder, nonce, joiner)) } else { None }
}

/// What a receiver does with one call offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectVerdict { Go, Stale, Mine, NotMember, NotForMe, Refused, BadInvite, NotDirect, Already, TooMany, TooManyHeld }

/// Everything the receiver knows about an offer, so that the rules are one pure function that tests can reach.
pub struct Offer<'a> {
    pub now: u64, pub ts: u64,
    /// The account that sent it, as vouched for by the room's group, and whether it is a current member of that room.
    pub sender: &'a str, pub sender_is_member: bool,
    pub me: &'a str, pub to: &'a str,
    pub nonce: &'a str, pub link: &'a InviteLink, pub relay: &'a str,
    /// Offers already taken from this sender in the last minute (times), automatic joins in the last hour, direct communities held.
    pub recent: &'a [u64], pub joins_last_hour: usize, pub held: usize,
    /// We already belong to this community, or the person has told us to refuse this sender, or an earlier join to it was refused.
    pub already_in: bool, pub sender_refused: bool, pub join_was_refused: bool,
}

pub fn offer_check(o: &Offer) -> DirectVerdict {
    use DirectVerdict::*;
    if o.sender == o.me { return Mine; }                                       // our own other devices never offer to us
    if !o.sender_is_member { return NotMember; }
    if o.ts + RING_MAX_AGE < o.now || o.ts > o.now + RING_MAX_AHEAD { return Stale; }
    if o.to != o.me { return NotForMe; }
    if o.sender_refused || o.join_was_refused { return Refused; }
    // The invite must be for a direct community that the sender founded, with us as its one peer, on our relay.
    let l = o.link;
    if l.owner != o.sender || l.relay.trim_end_matches('/') != o.relay.trim_end_matches('/') { return BadInvite; }
    if community_id(&l.owner, o.nonce) != l.community || !binds(&l.owner, o.nonce, o.me) { return NotDirect; }
    if o.already_in { return Already; }
    if o.recent.iter().filter(|t| **t + 60 > o.now).count() >= DIRECT_PER_MIN { return TooMany; }
    if o.joins_last_hour >= DIRECT_JOINS_PER_HOUR || o.held >= DIRECT_MAX { return TooManyHeld; }
    Go
}

/// A call this device started and is waiting to ring: the other person is not in the room until their app has taken the offer,
/// joined, and been added to the voice room. The ring goes out the moment they are, and is given up after [`DIRECT_WAIT`].
#[derive(Debug, Clone)]
pub struct DirectOut { pub cid: String, pub room: String, pub peer: String, pub started: u64 }

/// What `direct_call` did.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectStarted { pub cid: String, pub room: String, pub peer: String, /// The other person's app was sent an invite (they were not in the community yet); otherwise they are rung straight away.
    pub invited: bool }

/// One entry of the "direct" area.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectInfo { pub cid: String, pub peer: String, pub room: String, pub last: u64, pub members: usize, pub founder_me: bool }

impl Client {
    /// Is this community hidden from every list? A one-to-one community, or one this account founded under the name this app
    /// gives them while the other person has not joined yet (so that the founder's other devices hide it too).
    pub fn is_direct(&self, c: &Community) -> bool {
        c.direct.is_some() || (c.name == DIRECT_NAME && c.founder == self.my_account())
    }
    pub fn is_direct_cid(&self, cid: &str) -> bool { self.st.communities.get(cid).is_some_and(|c| self.is_direct(c)) }
    fn direct_room_of(c: &Community) -> Option<String> { c.channels.iter().find(|ch| ch.kind == ROOM_VOICE).map(|ch| ch.id.clone()) }

    /// The direct communities, newest call first. Not the ones we left or were removed from.
    pub fn direct_list(&self) -> Vec<DirectInfo> {
        let me = self.my_account();
        let mut v: Vec<DirectInfo> = self.st.communities.values().filter(|c| !c.removed && !c.direct_bad).filter_map(|c| {
            let peer = c.direct.clone()?;
            Some(DirectInfo { cid: c.id.clone(), peer, room: Self::direct_room_of(c).unwrap_or_default(), last: self.st.direct_last.get(&c.id).copied().unwrap_or(0), members: self.st.groups.get(&c.id).map_or(0, |g| g.accounts.len()), founder_me: c.founder == me })
        }).collect();
        v.sort_by(|a, b| b.last.cmp(&a.last).then(a.peer.cmp(&b.peer)));
        v
    }
    /// Does this person's app take part in one-to-one calls? `None` = we have not seen a profile from them.
    pub fn direct_support(&self, peer: &str) -> Option<bool> {
        self.st.profiles.get(peer).filter(|p| p.ts > 0).map(|p| p.direct)
    }
    /// Take calls from (or refuse them from) one account. Refusing also leaves the direct community they founded.
    pub fn direct_refuse(&mut self, account: &str, on: bool) -> Result<()> {
        if on {
            self.st.direct_refused.insert(account.to_string());
            let theirs: Vec<String> = self.st.communities.values().filter(|c| !c.removed && c.direct.as_deref() == Some(account) && c.founder == account).map(|c| c.id.clone()).collect();
            for cid in theirs { let _ = self.leave(&cid); }
        } else { self.st.direct_refused.remove(account); }
        self.save()
    }

    /// A room we share with `peer` that can carry the offer: an ordinary room (never a direct community's), ready to encrypt.
    fn shared_room_with(&self, peer: &str) -> Option<String> {
        let mut rooms: Vec<(bool, String)> = self.st.groups.values()
            .filter(|g| g.kind == Kind::Channel && !g.removed && !g.broken && g.accounts.contains(peer) && g.accounts.contains(&self.my_account()))
            .filter(|g| self.st.communities.get(&g.community).is_some_and(|c| !c.removed && !self.is_direct(c)))
            .filter(|g| self.ready_to_encrypt(&g.gid))
            .map(|g| (self.st.communities.get(&g.community).is_some_and(|c| c.channels.iter().any(|ch| ch.id == g.gid && ch.kind == ROOM_VOICE)), g.gid.clone())).collect();
        rooms.sort();
        rooms.into_iter().next().map(|(_, g)| g)
    }

    /// Call one person. Finds the direct community with them, or founds one (one voice room, us as owner), and then either rings
    /// them straight away (they are in it) or sends them an offer in a room we share and rings the moment they arrive.
    pub fn direct_call(&mut self, peer: &str) -> Result<DirectStarted> { self.direct_call_with(peer, true) }
    /// Test bridge only: call even a person whose app is known to be older, to show what such an app does with the offer.
    #[cfg(feature = "devbridge")]
    #[doc(hidden)]
    pub fn debug_direct_call_anyway(&mut self, peer: &str) -> Result<DirectStarted> { self.direct_call_with(peer, false) }
    fn direct_call_with(&mut self, peer: &str, check_support: bool) -> Result<DirectStarted> {
        let me = self.my_account();
        if peer == me { bail!("you cannot call yourself"); }
        if peer.len() != 64 || !peer.bytes().all(|b| b.is_ascii_hexdigit()) { bail!("not an account"); }
        if self.st.direct_refused.contains(peer) { bail!("you have stopped calls from this person; allow them again first"); }
        if check_support && self.direct_support(peer) == Some(false) { bail!("{} is using an older Megatron that cannot take direct calls. Ask them to update.", self.display_name(peer)); }
        let now = now_secs();
        let key = format!("direct|{peer}");
        if self.ring_sent.get(&key).is_some_and(|t| now < *t + 5) { bail!("you called a moment ago"); }
        // an existing one we can use: they are in its room, or we founded it and can invite them again
        let existing = self.st.communities.values().filter(|c| !c.removed && !c.direct_bad && c.direct.as_deref() == Some(peer))
            .filter(|c| Self::direct_room_of(c).is_some())
            .filter(|c| c.founder == me || self.st.groups.get(&c.id).is_some_and(|g| g.accounts.contains(peer)))
            .max_by_key(|c| (self.st.direct_last.get(&c.id).copied().unwrap_or(0), c.id.clone())).map(|c| c.id.clone());
        if let Some(cid) = &existing {
            let room = Self::direct_room_of(&self.st.communities[cid]).unwrap();
            if self.st.groups.get(&room).is_some_and(|g| g.accounts.contains(peer)) {
                self.ring_sent.insert(key, now);
                self.st.direct_last.insert(cid.clone(), now);
                self.save()?;
                self.direct_out.push(DirectOut { cid: cid.clone(), room: room.clone(), peer: peer.into(), started: now });
                self.direct_ring_due();
                return Ok(DirectStarted { cid: cid.clone(), room, peer: peer.into(), invited: false });
            }
        }
        // They are not in it: an offer travels in a room we share, which must exist before anything is created.
        let carrier = self.shared_room_with(peer).ok_or_else(|| anyhow!("you and {} do not share a room that is ready right now", self.display_name(peer)))?;
        let cid = match existing {
            Some(c) => c,
            None => {
                let nonce = new_nonce(&me, peer);
                let cid = community_id(&me, &nonce);
                self.st.direct_peers.insert(cid.clone(), peer.to_string());
                match self.create_community_with(DIRECT_NAME, nonce, DIRECT_ROOM, ROOM_VOICE) {
                    Ok(c) => c,
                    Err(e) => { self.st.direct_peers.remove(&cid); let _ = self.save(); return Err(e); }
                }
            }
        };
        let (room, nonce, banned) = { let c = &self.st.communities[&cid]; (Self::direct_room_of(c), c.nonce.clone(), c.banned.iter().any(|a| a == peer)) };
        if banned { bail!("this person is banned from your direct community"); }
        let room = room.ok_or_else(|| anyhow!("the call room is not ready yet; try again in a moment"))?;
        let made = self.create_invite_checked(&cid, Some(2), DIRECT_INVITE_TTL)?;
        // The offer is readable by everyone in the room it travels in. It is built for the one person it names, and carries
        // no more than they need: the relay's sign-up secret stays out (they are on this relay already).
        let mut link = InviteLink::decode(&made.link)?;
        link.secret.clear();
        self.ring_sent.insert(key, now);
        self.queue_env(&carrier, Envelope::Direct { id: crate::client::rand_hex(16), ts: now, to: peer.into(), nonce, link: link.encode() });
        self.st.direct_last.insert(cid.clone(), now);
        self.direct_out.push(DirectOut { cid: cid.clone(), room: room.clone(), peer: peer.into(), started: now });
        self.save()?;
        Ok(DirectStarted { cid, room, peer: peer.into(), invited: true })
    }

    /// Ring the people we called as soon as they are in the room (and give up on the ones that never came). Runs every round.
    pub(crate) fn direct_ring_due(&mut self) {
        let now = now_secs();
        let outs = std::mem::take(&mut self.direct_out);
        for o in outs {
            if now > o.started + DIRECT_WAIT { continue; }
            let here = self.st.groups.get(&o.room).is_some_and(|g| !g.removed && g.accounts.contains(&o.peer)) && self.ready_to_encrypt(&o.room);
            if !here { self.direct_out.push(o); continue; }
            match self.ring_send(&o.room, Some(&o.peer)) {
                Ok(_) => { self.st.direct_last.insert(o.cid.clone(), now); }
                Err(e) => { log::info!("direct ring: {e}"); if now < o.started + 10 { self.direct_out.push(o); } }
            }
        }
    }

    /// Housekeeping, every round: ring who is due, forget offers we joined that went nowhere, and leave a one-to-one community
    /// that is no longer exactly one.
    pub(crate) fn direct_tick(&mut self) {
        self.direct_ring_due();
        let now = now_secs();
        // joins taken on a caller's word: done once we are in; dropped (and the refusal remembered) when refused or stuck
        let waiting: Vec<(String, u64)> = self.st.direct_joining.iter().map(|(c, t)| (c.clone(), *t)).collect();
        for (cid, t) in waiting {
            if self.st.communities.get(&cid).is_some_and(|c| !c.removed) { self.st.direct_joining.remove(&cid); continue; }
            if let Some(r) = self.st.join_refused.get(&cid) {
                if r.reason.contains("banned") { self.st.direct_refused.insert(format!("c:{cid}")); }
                let _ = self.cancel_join(&cid);
                self.st.direct_joining.remove(&cid);
            } else if now > t + 3600 {
                let _ = self.cancel_join(&cid);
                self.st.direct_joining.remove(&cid);
            }
        }
        // A direct community that stopped being a direct community: never kept, and the person who did it is told nothing.
        let bad: Vec<String> = self.st.communities.values().filter(|c| !c.removed && c.direct_bad && c.founder != self.my_account()).map(|c| c.id.clone()).collect();
        for cid in bad {
            if self.last_dir_check.get(&format!("direct-bad|{cid}")).is_some_and(|t| now < *t + 120) { continue; }
            self.last_dir_check.insert(format!("direct-bad|{cid}"), now);
            self.notice("direct", "left a one-to-one call community that was changed after it was made");
            let _ = self.leave(&cid);
        }
    }

    /// An offer to call, from a member of a room we share. Joins the community in it only if everything in [`offer_check`] holds.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn take_direct(&mut self, gid: &str, acct: &str, id: String, ts: u64, to: String, nonce: String, link: String) {
        let now = now_secs();
        let me = self.my_account();
        let Ok(l) = InviteLink::decode(&link) else { return };
        // Only from a room of an ordinary community: a direct community's own room is not a place to be offered another.
        let Some(g) = self.st.groups.get(gid) else { return };
        let member = !g.removed && g.accounts.contains(acct);
        if self.st.communities.get(&g.community).is_none_or(|c| c.removed || self.is_direct(c)) { return; }
        let recent: Vec<u64> = self.direct_rate.get(acct).map(|q| q.iter().copied().collect()).unwrap_or_default();
        self.direct_joined.retain(|t| *t + 3600 > now);
        let held = self.st.communities.values().filter(|c| !c.removed && c.direct.is_some()).count();
        let already_in = self.st.communities.get(&l.community).is_some_and(|c| !c.removed) || self.st.joining.contains_key(&l.community);
        let offer = Offer { now, ts, sender: acct, sender_is_member: member, me: &me, to: &to, nonce: &nonce, link: &l, relay: &self.st.relay,
            recent: &recent, joins_last_hour: self.direct_joined.len(), held, already_in,
            sender_refused: self.st.direct_refused.contains(acct) || self.st.direct_refused.contains(&format!("c:{}", l.community)), join_was_refused: self.st.join_refused.contains_key(&l.community) };
        let verdict = offer_check(&offer);
        if verdict != DirectVerdict::Go { log::info!("call offer from {} not taken: {verdict:?}", &acct[..6.min(acct.len())]); return; }
        if self.direct_seen.contains_key(&id) { return; }
        self.direct_seen.retain(|_, t| *t + 300 > now);
        self.direct_seen.insert(id, now);
        let q = self.direct_rate.entry(acct.to_string()).or_default();
        while q.front().is_some_and(|t| *t + 60 <= now) { q.pop_front(); }
        q.push_back(now);
        if self.direct_rate.len() > 256 { self.direct_rate.retain(|_, q| !q.is_empty()); }
        match self.join_with_link(&link) {
            Ok(cid) => {
                self.direct_joined.push_back(now);
                self.st.direct_joining.insert(cid.clone(), now);
                self.st.direct_last.insert(cid, now);
                self.activity += 1;
            }
            Err(e) => log::info!("call offer: could not join: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const F: &str = "f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0";
    fn k(n: u8) -> String { format!("{:064x}", n) }
    fn link(owner: &str, nonce: &str) -> InviteLink {
        InviteLink { owner: owner.into(), relay: "http://r".into(), community: community_id(owner, nonce), invite: "a".repeat(32), sk: "b".repeat(64), name: "x".into(), secret: String::new() }
    }
    #[test]
    fn the_marker_commits_to_exactly_two_accounts() {
        let (a, b, c) = (k(1), k(2), k(3));
        let n = new_nonce(&a, &b);
        assert_eq!(n.len(), 32);
        assert!(binds(&a, &n, &b));
        assert!(!binds(&a, &n, &c), "a third account does not verify");
        assert!(!binds(&b, &n, &a), "nor does the same pair the other way round: the founder is part of it");
        assert!(!binds(&a, &"0".repeat(32), &b) && !binds(&a, "short", &b) && !binds(&a, &n[..31], &b));
        assert_ne!(new_nonce(&a, &b), new_nonce(&a, &b), "salted");
        // a plain random community is not one
        assert!(!binds(&a, &crate::client::rand_hex(16), &b));
        assert_eq!(mark(&a, &crate::client::rand_hex(16), &b, &[a.clone(), b.clone()].into(), &[], None), (None, false));
    }
    #[test]
    fn mark_finds_the_peer_and_notices_a_changed_shape() {
        let (a, b, c) = (k(1), k(2), k(3));
        let n = new_nonce(&a, &b);
        let voice = ChanMeta { id: "ab".repeat(16), name: "call".into(), kind: ROOM_VOICE.into() };
        let text = ChanMeta { id: "cd".repeat(16), name: "chat".into(), kind: String::new() };
        let two: BTreeSet<String> = [a.clone(), b.clone()].into();
        // the peer's side and the founder's side both see the other person, and both see a healthy community
        assert_eq!(mark(&a, &n, &b, &two, &[voice.clone()], None), (Some(a.clone()), false));
        assert_eq!(mark(&a, &n, &a, &two, &[voice.clone()], None), (Some(b.clone()), false));
        // the founder before the peer joined only knows who it asked
        assert_eq!(mark(&a, &n, &a, &[a.clone()].into(), &[], Some(&b)), (Some(b.clone()), false));
        // a third account, a text room or a second room: still marked, and no longer right
        let three: BTreeSet<String> = [a.clone(), b.clone(), c.clone()].into();
        assert_eq!(mark(&a, &n, &b, &three, &[voice.clone()], None), (Some(a.clone()), true));
        assert_eq!(mark(&a, &n, &b, &two, &[voice.clone(), voice.clone()], None).1, true);
        assert_eq!(mark(&a, &n, &b, &two, &[text], None).1, true);
        // somebody the nonce does not name sees an ordinary community (it was never offered to them, and they can leave it)
        assert_eq!(mark(&a, &n, &c, &[a.clone(), c.clone()].into(), &[voice], None), (None, false));
    }
    #[test]
    fn only_the_peer_is_admitted() {
        let (a, b, c) = (k(1), k(2), k(3));
        let n = new_nonce(&a, &b);
        assert_eq!(admits(DIRECT_NAME, true, &a, &n, &b), Some(true));
        assert_eq!(admits(DIRECT_NAME, true, &a, &n, &c), Some(false));
        assert_eq!(admits(DIRECT_NAME, false, &a, &n, &c), Some(false), "a device that cannot yet tell who the peer is refuses by the nonce alone");
        assert_eq!(admits("friends", false, &a, &crate::client::rand_hex(16), &c), None, "an ordinary community keeps its ordinary rules");
    }
    fn offer<'a>(l: &'a InviteLink, nonce: &'a str, me: &'a str, recent: &'a [u64]) -> Offer<'a> {
        Offer { now: 1000, ts: 1000, sender: &l.owner, sender_is_member: true, me, to: me, nonce, link: l, relay: "http://r/", recent, joins_last_hour: 0, held: 0, already_in: false, sender_refused: false, join_was_refused: false }
    }
    #[test]
    fn an_offer_is_taken_only_when_everything_checks_out() {
        use DirectVerdict::*;
        let (a, b, c) = (k(1), k(2), k(3));
        let n = new_nonce(&a, &b);
        let l = link(&a, &n);
        assert_eq!(offer_check(&offer(&l, &n, &b, &[])), Go);
        // not for me / from me / not a member / stale
        assert_eq!(offer_check(&Offer { to: &c, ..offer(&l, &n, &b, &[]) }), NotForMe);
        assert_eq!(offer_check(&Offer { sender: &b, ..offer(&l, &n, &b, &[]) }), Mine);
        assert_eq!(offer_check(&Offer { sender_is_member: false, ..offer(&l, &n, &b, &[]) }), NotMember);
        assert_eq!(offer_check(&Offer { ts: 1000 - RING_MAX_AGE - 1, ..offer(&l, &n, &b, &[]) }), Stale);
        assert_eq!(offer_check(&Offer { ts: 1000 + RING_MAX_AHEAD + 1, ..offer(&l, &n, &b, &[]) }), Stale);
        // an invite to somebody else's community, to a community that is not direct, to one made for a different pair, or on another relay
        let other = link(&c, &new_nonce(&c, &b));
        assert_eq!(offer_check(&Offer { link: &other, nonce: &other.community, ..offer(&l, &n, &b, &[]) }), BadInvite, "the link's owner must be the sender");
        let plain = crate::client::rand_hex(16);
        let lp = link(&a, &plain);
        assert_eq!(offer_check(&offer(&lp, &plain, &b, &[])), NotDirect, "an ordinary community is never joined on a call's word");
        let for_c = new_nonce(&a, &c);
        let lc = link(&a, &for_c);
        assert_eq!(offer_check(&offer(&lc, &for_c, &b, &[])), NotDirect, "a direct community of the sender and somebody else is not mine");
        assert_eq!(offer_check(&offer(&l, &for_c, &b, &[])), NotDirect, "the nonce must be the one the community id was made from");
        let far = InviteLink { relay: "http://elsewhere".into(), ..link(&a, &n) };
        assert_eq!(offer_check(&offer(&far, &n, &b, &[])), BadInvite);
        // the person's own choices and what is already so
        assert_eq!(offer_check(&Offer { sender_refused: true, ..offer(&l, &n, &b, &[]) }), Refused);
        assert_eq!(offer_check(&Offer { join_was_refused: true, ..offer(&l, &n, &b, &[]) }), Refused);
        assert_eq!(offer_check(&Offer { already_in: true, ..offer(&l, &n, &b, &[]) }), Already);
    }
    #[test]
    fn offers_are_rate_limited() {
        use DirectVerdict::*;
        let (a, b) = (k(1), k(2));
        let n = new_nonce(&a, &b);
        let l = link(&a, &n);
        let two = [999, 990];
        assert_eq!(offer_check(&offer(&l, &n, &b, &two)), TooMany, "{DIRECT_PER_MIN} a minute from one sender");
        let old = [900, 800];
        assert_eq!(offer_check(&offer(&l, &n, &b, &old)), Go, "a minute later it is open again");
        assert_eq!(offer_check(&Offer { joins_last_hour: DIRECT_JOINS_PER_HOUR, ..offer(&l, &n, &b, &[]) }), TooManyHeld);
        assert_eq!(offer_check(&Offer { held: DIRECT_MAX, ..offer(&l, &n, &b, &[]) }), TooManyHeld);
        let _ = F;
    }
    #[test]
    fn the_offer_envelope_parses_and_an_old_app_skips_it() {
        let (a, b) = (k(1), k(2));
        let n = new_nonce(&a, &b);
        let e = Envelope::Direct { id: "1".repeat(32), ts: 5, to: b.clone(), nonce: n.clone(), link: link(&a, &n).encode() };
        assert_eq!(Envelope::parse(&e.to_bytes()).unwrap(), e);
        let bad = |x: Envelope| assert!(Envelope::parse(&x.to_bytes()).is_err());
        bad(Envelope::Direct { id: "x".into(), ts: 5, to: b.clone(), nonce: n.clone(), link: link(&a, &n).encode() });
        bad(Envelope::Direct { id: "1".repeat(32), ts: 5, to: "nope".into(), nonce: n.clone(), link: link(&a, &n).encode() });
        bad(Envelope::Direct { id: "1".repeat(32), ts: 5, to: b.clone(), nonce: "short".into(), link: link(&a, &n).encode() });
        bad(Envelope::Direct { id: "1".repeat(32), ts: 5, to: b.clone(), nonce: n.clone(), link: "commons://join/garbage".into() });
        bad(Envelope::Direct { id: "1".repeat(32), ts: 5, to: b, nonce: n, link: "x".repeat(4000) });
        #[derive(serde::Deserialize, Debug, PartialEq)]
        #[serde(tag = "t")]
        enum Old { Typing, #[serde(other)] Unknown }
        assert_eq!(serde_json::from_slice::<Old>(&e.to_bytes()).unwrap(), Old::Unknown, "an app from before direct calls skips it");
    }
}
