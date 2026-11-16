//! Receiving side: relay sync, Welcome handling, strictly ordered log processing, commit authorization.
use crate::client::*;
use crate::envelope::*;
use crate::mls;
use crate::state::*;
use anyhow::{anyhow, bail, Result};
use commons_proto::*;
use openmls::prelude::{tls_codec::*, *};
use std::collections::BTreeMap;

/// Timeline lines for people arriving in and leaving a room. The account id follows the prefix; the name is looked up when
/// the line is shown, because a newcomer's name usually arrives after they do.
pub const MEMBER_CAME: &str = "\u{1}came:";
pub const MEMBER_WENT: &str = "\u{1}went:";

pub const BATCH: usize = 300;

enum Verdict { Ok, Reject(String) }

impl Client {
    pub(crate) fn load_group(&self, gid: &str) -> Option<MlsGroup> {
        let id = GroupId::from_slice(&hex::decode(gid).ok()?);
        MlsGroup::load(self.prov.storage(), &id).ok().flatten()
    }

    pub(crate) fn sync_req(&mut self, wait_ms: u64) -> SyncReq {
        let mut cursors = BTreeMap::new();
        for g in self.st.groups.values().filter(|g| !g.removed) { cursors.insert(g.gid.clone(), g.cursor); }
        SyncReq { cursors, ack_welcomes: std::mem::take(&mut self.st.ack_welcomes), wait_ms, max_bytes: None }
    }

    /// One round trip with the relay plus processing of everything it returned.
    /// `wait_ms` > 0 makes it a long-poll. Used by actions that must see the log right now; the background loop
    /// does the same work in rounds, without holding the client while it waits for the network.
    pub fn sync_once(&mut self, wait_ms: u64) -> Result<()> {
        let k = self.dev_key()?;
        let req = self.sync_req(wait_ms);
        let (r, cap, bad) = crate::round::fetch_sync(&self.api, &k, &req, self.sync_cap);
        self.sync_cap = cap;
        let resp = match r {
            Ok(r) => r,
            Err(e) => { self.st.ack_welcomes.extend(req.ack_welcomes); self.note_failure(&e); return Err(e); }
        };
        self.apply_sync(resp, &bad);
        self.save()
    }

    /// Sync until the relay has nothing more for us, within reason.
    pub(crate) fn catch_up(&mut self) -> Result<()> {
        for _ in 0..8 {
            self.more = false;
            self.sync_once(0)?;
            if !self.more { break; }
        }
        Ok(())
    }

    /// Everything the relay returned. Local work only. `unreadable` lists groups whose entries could not be fetched:
    /// they say so and wait, and every other group carries on.
    pub(crate) fn apply_sync(&mut self, mut resp: SyncResp, unreadable: &[String]) {
        let mut opened = false;
        for w in &resp.welcomes {
            if !self.seen_welcomes.insert(w.id.clone()) { self.st.ack_welcomes.push(w.id.clone()); continue; }
            let data = match unb64(&w.data) { Ok(d) => d, Err(_) => { self.st.ack_welcomes.push(w.id.clone()); continue; } };
            if self.st.parked_welcomes.iter().any(|p| p.id == w.id) { self.st.ack_welcomes.push(w.id.clone()); continue; }
            // MLS messages start with a version number, our JSON with a brace; history chunks for a new device with their own mark.
            if data.starts_with(crate::history::MAGIC) { self.history_take_chunk(&data[crate::history::MAGIC.len()..]); self.st.ack_welcomes.push(w.id.clone()); continue; }
            if data.starts_with(br#"{"t":"hist-req""#) { self.history_take_request(&data); self.st.ack_welcomes.push(w.id.clone()); continue; }
            if data.first() == Some(&b'{') { self.handle_join_reply(&data); self.st.ack_welcomes.push(w.id.clone()); continue; }
            match self.handle_welcome(&w.id, &data) {
                Ok(true) => opened = true,
                Ok(false) => self.park_welcome(w.id.clone(), w.data.clone()),
                Err(e) => self.notice("welcome", format!("ignored an invitation to a group: {e}")),
            }
            self.st.ack_welcomes.push(w.id.clone());
        }
        self.retry_parked(opened);
        let mut order: Vec<String> = self.st.groups.values().filter(|g| !g.removed).map(|g| g.gid.clone()).collect();
        order.sort_by_key(|g| (self.st.groups[g].kind != Kind::Control, g.clone()));
        let mut ctl_ok: BTreeMap<String, bool> = BTreeMap::new();
        for gid in order {
            let kind = self.st.groups[&gid].kind.clone();
            let comm = self.st.groups[&gid].community.clone();
            if unreadable.contains(&gid) {
                self.synced.remove(&gid);
                self.notice("relay", format!("group {}: the relay's answer could not be read, so this group is paused; the others carry on", &gid[..8.min(gid.len())]));
                if kind == Kind::Control { ctl_ok.insert(comm, false); }
                continue;
            }
            if kind == Kind::Channel && ctl_ok.get(&comm) == Some(&false) { continue; }
            let entries = resp.messages.remove(&gid).unwrap_or_default();
            let ok = self.process_entries(&gid, entries, resp.more.contains(&gid));
            if kind == Kind::Control { ctl_ok.insert(comm, ok); }
        }
        self.more |= !resp.more.is_empty();
        for gid in &resp.denied {
            if self.st.groups.get(gid).is_some_and(|g| !g.removed && !g.denied) {
                self.st.groups.get_mut(gid).unwrap().denied = true;
                self.bump_chan(gid);
                self.notice("relay", format!("the relay no longer lets this account read group {} (you were probably removed while offline, or the relay is misbehaving)", &gid[..8.min(gid.len())]));
            }
        }
        let undeny: Vec<String> = self.st.groups.values_mut().filter(|g| g.denied && !resp.denied.contains(&g.gid)).map(|g| { g.denied = false; g.gid.clone() }).collect();
        for gid in undeny { self.bump_chan(&gid); }
    }

    /// Returns false if the log had a gap or processing could not complete (caller must not trust dependents this round).
    /// `more`: the relay holds further entries for this group, so we are not caught up however few arrived.
    fn process_entries(&mut self, gid: &str, mut entries: Vec<Entry>, more: bool) -> bool {
        entries.sort_by_key(|e| e.seq);
        entries.dedup_by_key(|e| e.seq);
        let total = entries.len();
        let mut complete = true;
        for e in entries {
            let Some(g) = self.st.groups.get(gid) else { return false };
            if g.removed { return true; }
            let cur = g.cursor;
            if e.seq <= cur { continue; }
            if e.seq != cur + 1 {
                self.st.groups.get_mut(gid).unwrap().gaps += 1;
                self.notice("gap", format!("group {}: expected entry {} but relay sent {}; waiting for the missing entries (nothing is skipped)", &gid[..8.min(gid.len())], cur + 1, e.seq));
                complete = false;
                break;
            }
            let data = unb64(&e.data).unwrap_or_default();
            if data.is_empty() { self.notice("parse", format!("empty log entry {} in group {}", e.seq, &gid[..8])); }
            else {
                match data[0] {
                    TAG_MLS | TAG_COMMIT => self.apply_mls(gid, e.seq, &e.id, data[0], &data[1..]),
                    TAG_JOIN_REQUEST => self.queue_join(gid, e.seq, &data[1..]),
                    TAG_REJOIN => {
                        let g = self.st.groups.get_mut(gid).unwrap();
                        if g.requests.iter().filter(|r| r.tag == TAG_REJOIN).count() < 200 { g.requests.push(ReqItem { seq: e.seq, tag: data[0], data: b64(&data[1..]), tries: 0, unverified: false }); }
                        else { self.notice("reject", format!("group {}: too many re-add requests are waiting; dropped one, the device will ask again", &gid[..8])); }
                    }
                    t => self.notice("parse", format!("unknown log entry type {t} in group {} ignored", &gid[..8])),
                }
            }
            self.st.groups.get_mut(gid).unwrap().cursor = e.seq;
            self.activity += 1;
        }
        if complete && total < BATCH && !more { self.synced.insert(gid.to_string()); } else if !complete { self.synced.remove(gid); }
        complete
    }

    /// A knock on the door, from anyone who can reach the relay. Only admins act on these, and only what checks out
    /// takes up room: a signature made with the invite's secret key proves the knocker holds the link. A request for an
    /// invite we no longer list cannot be checked; it is kept apart, because it may be someone we owe an answer.
    fn queue_join(&mut self, gid: &str, seq: u64, payload: &[u8]) {
        let Some(g) = self.st.groups.get(gid) else { return };
        if g.kind != Kind::Control { return; }
        let Some(c) = self.st.communities.get(gid) else { return };
        if c.role_of(&self.my_account()).unwrap_or(Role::Member) < Role::Admin { return; }
        let jr = match JoinRequest::parse(payload) {
            Ok(j) => j,
            Err(_) => { self.notice("join", "ignored malformed join requests"); return }
        };
        let unverified = match c.invites.iter().find(|i| i.id == jr.invite) {
            Some(inv) => {
                if !join_signature_ok(inv, gid, &jr) { self.notice("join", "ignored a join request with a bad invite signature"); return; }
                false
            }
            None => true,
        };
        let dup = g.requests.iter().any(|r| r.tag == TAG_JOIN_REQUEST && JoinRequest::parse(&unb64(&r.data).unwrap_or_default()).is_ok_and(|x| x.cert.device == jr.cert.device && x.invite == jr.invite));
        let held = g.requests.iter().filter(|r| r.tag == TAG_JOIN_REQUEST && r.unverified == unverified).count();
        if dup { return; }
        if held >= if unverified { 200 } else { 1000 } {
            self.notice("join", "too many join requests are waiting; dropped one. A real request is sent again later.");
            return;
        }
        self.st.groups.get_mut(gid).unwrap().requests.push(ReqItem { seq, tag: TAG_JOIN_REQUEST, data: b64(payload), tries: 0, unverified });
    }

    /// An admin says our join will not happen. Believed only if it is for a join we asked for and comes from an admin
    /// of the community the invite link named.
    fn handle_join_reply(&mut self, data: &[u8]) {
        let Ok(r) = JoinReply::parse(data) else { self.notice("welcome", "ignored a malformed reply to a join request"); return };
        let Some(l) = self.st.joining.get(&r.community).cloned() else { return };
        if let Err(e) = r.verify(&l.owner, &self.my_device()) { self.notice("welcome", format!("ignored a reply to a join request: {e}")); return; }
        let text = join_reason_text(&r.reason);
        self.refuse_join(&r.community, text);
    }

    /// The join is over: stop waiting and keep the reason where the person will see it.
    pub(crate) fn refuse_join(&mut self, community: &str, reason: &str) {
        let Some(l) = self.st.joining.remove(community) else { return };
        if let Some(c) = self.st.join_consent.remove(community) { self.withdraw_consent(c.sig); }
        self.st.join_sent.remove(community);
        self.st.join_asked.remove(community);
        self.st.join_retry.remove(community);
        self.st.join_refused.insert(community.to_string(), JoinRefused { name: l.name.clone(), reason: reason.into(), ts: now_secs() });
        self.notice("reject", format!("could not join \"{}\": {reason}", l.name));
    }

    fn park_welcome(&mut self, id: String, data: String) {
        if self.st.parked_welcomes.len() >= 64 {
            self.st.parked_welcomes.remove(0);
            self.notice("welcome", "dropped an old invitation to a room of a community we never joined");
        }
        self.st.parked_welcomes.push(ParkedWelcome { id, data, ts: now_secs() });
    }

    /// Retried when a community may just have arrived, and now and then in case one arrived another way.
    fn retry_parked(&mut self, opened: bool) {
        let now = now_secs();
        if self.st.parked_welcomes.is_empty() || !(opened || now >= self.last_parked_try + 30) { return; }
        self.last_parked_try = now;
        let parked = std::mem::take(&mut self.st.parked_welcomes);
        for p in parked {
            if now.saturating_sub(p.ts) > 14 * 86400 { self.notice("welcome", "dropped an old invitation that never became usable"); continue; }
            let data = unb64(&p.data).unwrap_or_default();
            match self.handle_welcome(&p.id, &data) {
                Ok(true) => {}
                Ok(false) => self.st.parked_welcomes.push(p),
                Err(e) => self.notice("welcome", format!("dropped an invitation: {e}")),
            }
        }
    }

    // ------------------------------------------------------------------ welcome
    /// Ok(true)=joined or deliberately ignored, Ok(false)=cannot place it yet (park), Err=reject.
    fn handle_welcome(&mut self, id: &str, bytes: &[u8]) -> Result<bool> {
        // Opening a welcome uses up the key package behind it, and we only know what it is once it is open. To be able
        // to set one aside for later, the key store is put back the way it was.
        let before = self.prov.dump();
        match self.open_welcome(id, bytes) {
            Ok(false) => { self.prov = mls::Prov::load(&before)?; Ok(false) }
            r => r,
        }
    }

    fn open_welcome(&mut self, _id: &str, bytes: &[u8]) -> Result<bool> {
        let msg = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| anyhow!("malformed welcome: {e}"))?;
        let welcome = match msg.extract() { MlsMessageBodyIn::Welcome(w) => w, _ => bail!("not a welcome message") };
        let jb = StagedWelcome::build_from_welcome(&self.prov, &mls::join_cfg(), welcome).map_err(|e| anyhow!("cannot open welcome: {e}"))?;
        let wgid = hex::encode(jb.processed_welcome().unverified_group_info().group_id().as_slice());
        // A group we fell out of, or left, is still in the MLS store: coming back replaces it.
        let replace = self.st.groups.get(&wgid).is_some_and(|g| g.broken || g.removed);
        let jb = if replace { jb.replace_old_group() } else { jb };
        let staged = jb.build().map_err(|e| anyhow!("cannot open welcome: {e}"))?;
        let gid = hex::encode(staged.group_context().group_id().as_slice());
        let ext = mls::group_ext(staged.group_context().extensions()).ok_or_else(|| anyhow!("group has no commons metadata"))?;
        for m in staged.members() {
            if mls::cert_of(&m.credential, &m.signature_key).is_none() { bail!("group contains a member with an invalid device certificate"); }
        }
        let sender = staged.welcome_sender().map_err(|_| anyhow!("no welcome sender"))?;
        let sender_cert = mls::cert_of(sender.credential(), sender.signature_key().as_slice()).ok_or_else(|| anyhow!("welcome sender has an invalid certificate"))?;
        let me = self.my_account();
        let existing_broken = self.st.groups.get(&gid).is_some_and(|g| g.broken);
        if self.st.groups.get(&gid).is_some_and(|g| !g.broken && !g.removed) { return Ok(true); } // duplicate welcome
        let (kind, community) = match &ext {
            GroupExt::Control { owner: founder, consents, .. } => {
                let (owner, admins) = ext.verify_control(&gid).map_err(|e| anyhow!("control table rejected: {e}"))?;
                let sender_ok = sender_cert.account == owner || admins.contains(&sender_cert.account) || sender_cert.account == me;
                if !sender_ok { bail!("welcome to a community from someone who is not its owner or admin"); }
                let live = self.st.communities.get(&gid).is_some_and(|c| !c.removed);
                // Somebody else's word that we belong here means nothing, and neither does a device of ours being in the
                // group, because whoever runs a community can add every device of an account. So: we asked on this device,
                // or we are in it already, or the table holds this account's own signature for this community. That is
                // what a second or recovered device has to go on, and only a device of ours can have made it.
                let consented = consents.iter().find(|c| c.account == me).is_some_and(|c| verify_consent(&gid, c).is_ok() && !self.st.withdrawn.contains(&c.sig));
                let expected = self.st.joining.get(&gid).is_some_and(|l| &l.owner == founder) || live || consented;
                if !expected { bail!("unsolicited community invitation (no invite link of ours matches it, and this account never agreed to it)"); }
                (Kind::Control, gid.clone())
            }
            GroupExt::Channel { community, .. } => {
                let (_, ch_owner, ch_admins) = ext.verify_channel().map_err(|e| anyhow!("channel table rejected: {e}"))?;
                // A room only means something inside a community we are in, and only its owner and admins hand them
                // out; a device of ours may add this one. The community's own welcome can still be on its way
                // (another admin may send it later), so an unknown one means wait, not refuse.
                if !self.st.communities.get(community).is_some_and(|c| !c.removed) { return Ok(false); }
                let ok = sender_cert.account == ch_owner || ch_admins.contains(&sender_cert.account) || sender_cert.account == me;
                if !ok { bail!("channel welcome from someone who is not an admin"); }
                (Kind::Channel, community.clone())
            }
        };
        let group = staged.into_group(&self.prov).map_err(|e| anyhow!("cannot join: {e}"))?;
        let epoch = group.epoch().as_u64();
        let prev_broken = if existing_broken { self.st.groups.get(&gid).map(|g| (g.broken_count, g.cursor)) } else { None };
        let mut gs = GroupState::new(&gid, &community, kind.clone());
        gs.join_epoch = epoch;
        if let Some(old) = self.st.groups.get(&gid) { gs.cursor = old.cursor; }
        self.st.groups.insert(gid.clone(), gs);
        if kind == Kind::Control {
            let c = self.st.communities.entry(gid.clone()).or_insert_with(|| Community { id: gid.clone(), ..Default::default() });
            c.removed = false; c.removed_reason.clear(); c.leaving.clear(); c.kick_queue.clear();
            self.st.joining.remove(&gid);
            self.st.join_consent.remove(&gid);
            self.st.join_sent.remove(&gid);
            self.st.join_asked.remove(&gid);
            self.st.join_retry.remove(&gid);
            self.st.join_refused.remove(&gid);
            self.st.profile_dirty = true;
        }
        self.post_merge(&gid, 0, &[]);
        if let Some((n, _)) = prev_broken {
            self.add_system_msg(&gid, format!("This device was re-added to the group after its saved state fell behind. {n} messages in between cannot be read on this device (their keys were lost with the old state). Other devices still have them."));
            self.notice("rejoin", format!("re-added to group {}", &gid[..8]));
        } else {
            self.notice("joined", format!("joined group {} ({})", &gid[..8], if kind == Kind::Control { "community" } else { "channel" }));
        }
        self.synced.remove(&gid);
        // Rotate to fresh keys right away so the adder's welcome secrets stop being the only protection.
        Ok(true)
    }

    // ------------------------------------------------------------------ log entry: MLS message
    fn apply_mls(&mut self, gid: &str, seq: u64, id: &str, tag: u8, bytes: &[u8]) {
        let gs = self.st.groups[gid].clone();
        let Some(mut group) = self.load_group(gid) else { self.notice("state", format!("group {} missing from local MLS state", &gid[..8])); return };
        let msg_in = match MlsMessageIn::tls_deserialize_exact(bytes) {
            Ok(m) => m,
            Err(e) => { self.fail_visible(gid, seq, None, format!("a malformed message was in the log ({e})")); return }
        };
        let pm = match msg_in.try_into_protocol_message() {
            Ok(p) => p,
            Err(_) => { self.fail_visible(gid, seq, None, "an unexpected message type was in the log".into()); return }
        };
        if pm.group_id() != group.group_id() { self.notice("parse", format!("entry {seq} in {} names another group; ignored", &gid[..8])); return; }
        let epoch = pm.epoch().as_u64();
        if gs.broken { self.st.groups.get_mut(gid).unwrap().broken_count += 1; return; }
        if epoch < gs.join_epoch { return; } // predates our membership: never addressed to us
        // A commit for an epoch we have left lost the race to the one that got there first. Everyone skips it alike, and
        // it held no message, so there is nothing to tell the room. (Our own pending one is dropped when the winner lands.)
        if tag == TAG_COMMIT && epoch < group.epoch().as_u64() { log::info!("group {}: commit {seq} lost the race for epoch {epoch}", &gid[..8]); return; }
        let is_own = self.st.own_ids.iter().any(|x| x == id);
        match group.process_message(&self.prov, pm) {
            Ok(p) => self.handle_processed(gid, seq, id, tag, is_own, &mut group, p),
            Err(e) => {
                let s = format!("{e:?}");
                let cur = group.epoch().as_u64();
                if s.contains("WrongEpoch") || s.contains("NoPastEpochData") || s.contains("PastEpoch") {
                    if epoch > cur {
                        if gs.tainted {
                            self.mark_broken(gid, seq, "this device's saved state is older than the group's (a commit made from the lost state cannot be followed)");
                        } else {
                            self.fail_visible(gid, seq, None, format!("a message for epoch {epoch} arrived but this device is at epoch {cur}; the commit in between was rejected as unauthorized or is missing"));
                        }
                    } else {
                        self.fail_visible(gid, seq, None, format!("a message from an older key epoch ({epoch}, we keep {} back) arrived too late to decrypt", mls::MAX_PAST_EPOCHS));
                    }
                } else if is_own {
                    // our own message/commit that we can no longer process
                    self.mark_broken(gid, seq, "our own earlier commit cannot be re-read (device state was rolled back)");
                } else if s.contains("GenerationOutOfBound") || s.contains("TooDistantInThePast") || s.contains("SecretReuseError") || s.contains("AeadError") {
                    self.fail_visible(gid, seq, None, "a message could not be decrypted (duplicate, replayed, or its key was already used or expired)".into());
                } else {
                    self.fail_visible(gid, seq, None, format!("a message could not be processed: {}", s.chars().take(120).collect::<String>()));
                }
            }
        }
    }

    fn mark_broken(&mut self, gid: &str, seq: u64, why: &str) {
        let g = self.st.groups.get_mut(gid).unwrap();
        if !g.broken { g.broken = true; g.broken_from = seq; g.broken_count = 1; g.rejoin_sent = false; }
        self.notice("broken", format!("group {}: {why}. Asking to be re-added.", &gid[..8]));
        self.add_system_msg(gid, "This device fell out of sync with the group (its saved state was older than what the group had already done). It has asked to be re-added; until then new messages cannot be read here.".into());
    }

    /// Never silent: every failure becomes a timeline line in the affected channel and a notice.
    pub(crate) fn fail_visible(&mut self, gid: &str, seq: u64, sender: Option<String>, why: String) {
        self.notice("decrypt", format!("group {} entry {}: {}", &gid[..8.min(gid.len())], seq, why));
        let _ = sender;
        self.add_system_msg(gid, format!("Message #{seq}: {why}."));
    }

    pub(crate) fn add_system_msg(&mut self, gid: &str, text: String) { self.add_system_msg_at(gid, 0, text) }
    /// `seq` 0 puts the line after everything delivered (failures, which belong at the end); a log position puts it there.
    pub(crate) fn add_system_msg_at(&mut self, gid: &str, seq: u64, text: String) {
        let Some(g) = self.st.groups.get(gid) else { return };
        if g.kind != Kind::Channel { return; }
        let m = Msg { id: rand_hex(16), sender: String::new(), device: String::new(), ts: now_secs(), seq, body: MsgBody::Notice(text), reply: None, edited: false, deleted: false, reactions: Default::default(), status: "system".into() };
        self.st.chans.entry(gid.to_string()).or_default().msgs.push(m);
        self.bump_chan(gid);
    }

    fn handle_processed(&mut self, gid: &str, seq: u64, id: &str, tag: u8, is_own: bool, group: &mut MlsGroup, p: ProcessedMessage) {
        let sender_idx = match p.sender() { Sender::Member(i) => Some(*i), _ => None };
        let cert = sender_idx.and_then(|i| group.member_at(i)).and_then(|m| mls::cert_of(&m.credential, &m.signature_key));
        let pending_id = self.st.groups[gid].pending.as_ref().map(|p| p.msg_id.clone());
        match p.into_content() {
            ProcessedMessageContent::OwnPrivateMessage | ProcessedMessageContent::OwnPendingCommit => {
                if pending_id.as_deref() == Some(id) {
                    let verdict = match (group.pending_commit(), &cert) {
                        (Some(pc), Some(c)) => self.authorize(gid, group, c, pc),
                        _ => Verdict::Ok,
                    };
                    if let Verdict::Reject(why) = verdict {
                        let _ = group.clear_pending_commit(self.prov.storage());
                        self.drop_pending(gid, &format!("not allowed by the group's own rules: {why}"));
                    } else if group.pending_commit().is_some() {
                        match group.merge_pending_commit(&self.prov) {
                            Ok(()) => self.finish_own_commit(gid, seq),
                            Err(e) => { self.notice("commit", format!("could not merge our own commit: {e}")); }
                        }
                    } else {
                        self.drop_pending(gid, "lost the race to another commit");
                    }
                } else if !is_own && tag == TAG_COMMIT {
                    self.mark_broken(gid, seq, "a commit made by this device is missing from its saved state (restored from an older backup)");
                } else if !is_own {
                    // We sent something from a state newer than the one we are running now.
                    self.st.groups.get_mut(gid).unwrap().tainted = true;
                    self.notice("rollback", format!("group {}: entry {seq} was sent by this device from a newer state than the one loaded (restored from a backup?)", &gid[..8]));
                    self.add_system_msg(gid, format!("Message #{seq} was sent from this device but this copy of its state is older, so its content is not available here. Sending is paused until this device moves to a fresh key epoch."));
                }
            }
            ProcessedMessageContent::ApplicationMessage(a) => {
                let Some(cert) = cert else { self.fail_visible(gid, seq, None, "message from a sender with an invalid certificate".into()); return };
                match Envelope::parse(&a.into_bytes()) {
                    Ok(env) => self.apply_env(gid, seq, &cert, env),
                    Err(e) => self.fail_visible(gid, seq, Some(cert.account.clone()), format!("a decrypted message was not understood ({e})")),
                }
            }
            ProcessedMessageContent::StagedCommitMessage(sc) => {
                let Some(cert) = cert else { self.notice("reject", "commit from a sender with an invalid certificate rejected".to_string()); return };
                match self.authorize(gid, group, &cert, &sc) {
                    Verdict::Ok => {
                        if let Some(w) = self.style_warning(gid, group, &cert, &sc) { self.notice("style", w); }
                        let self_removed = sc.self_removed();
                        let added: Vec<String> = sc.add_proposals().filter_map(|a| { let k = a.add_proposal().key_package(); mls::cert_of(k.leaf_node().credential(), k.leaf_node().signature_key().as_slice()).map(|c| c.device) }).collect();
                        match group.merge_staged_commit(&self.prov, *sc) {
                            Ok(()) => {
                                if self.st.groups[gid].pending.is_some() && group.pending_commit().is_none() { self.drop_pending(gid, "another commit was accepted first"); }
                                if self_removed { self.notice("removed", format!("removed from group {}", &gid[..8])); }
                                self.post_merge(gid, seq, &added);
                            }
                            Err(e) => self.notice("commit", format!("could not merge commit {seq}: {e}")),
                        }
                    }
                    Verdict::Reject(why) => {
                        self.notice("reject", format!("group {}: commit {seq} by {} rejected: {why}", &gid[..8], &cert.account[..8]));
                    }
                }
            }
            ProcessedMessageContent::ProposalMessage(_) | ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                self.notice("reject", format!("group {}: standalone proposal {seq} ignored (this app only uses commits)", &gid[..8]));
            }
        }
    }

    pub(crate) fn drop_pending(&mut self, gid: &str, why: &str) {
        let g = self.st.groups.get_mut(gid).unwrap();
        if let Some(p) = g.pending.take() { self.notice("commit", format!("group {}: our pending change ({}) was not applied: {why}; it will be re-evaluated", &gid[..8], p.desc)); self.bump_chan(gid); }
    }

    fn finish_own_commit(&mut self, gid: &str, seq: u64) {
        self.bump_chan(gid);   // pending → applied, and tainted clears: the room's status line changed
        let mut added = vec![];
        self.st.groups.get_mut(gid).unwrap().tainted = false; // a fresh epoch means fresh sender keys
        if let Some(p) = self.st.groups.get_mut(gid).unwrap().pending.take() {
            added = p.recipients.clone();
            self.applied.insert(p.msg_id.clone());
            if let Some(w) = p.welcome {
                for dev in p.recipients { self.st.welcome_out.push(WelcomeOut { id: rand_hex(16), device: dev, data: w.clone(), gid: gid.into() }); }
            }
            // call tokens stop at once (same authority as the removal itself); read access keeps its grace period below
            if !p.acl_remove.is_empty() { self.st.acl_jobs.push(AclJob { gid: gid.into(), call_remove: p.acl_remove.clone(), ..Default::default() }); }
            if !p.acl_remove.is_empty() { self.st.acl_jobs.push(AclJob { gid: gid.into(), remove: p.acl_remove, after: now_secs() + self.acl_delay_secs, ..Default::default() }); }
            if !p.acl_demote.is_empty() { self.st.acl_jobs.push(AclJob { gid: gid.into(), demote: p.acl_demote, ..Default::default() }); }
            for s in p.handled { self.drop_request(gid, s); }
            self.notice("commit", format!("group {}: applied {}", &gid[..8], p.desc));
        }
        self.post_merge(gid, seq, &added);
    }

    /// Refresh caches derived from the MLS group after its epoch changed.
    pub(crate) fn post_merge(&mut self, gid: &str, seq: u64, added: &[String]) {
        let Some(group) = self.load_group(gid) else { return };
        let me = self.my_account();
        let members = mls::members_of(&group);
        let accounts: std::collections::BTreeSet<String> = members.iter().map(|(_, c)| c.account.clone()).collect();
        let active = group.is_active();
        let gs = self.st.groups.get_mut(gid).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for (_, c) in &members { seen.insert(c.device.clone()); gs.dev_seq.entry(c.device.clone()).or_insert(seq); }
        gs.dev_seq.retain(|d, _| seen.contains(d));
        for d in added { gs.dev_seq.insert(d.clone(), seq); }
        let new_accounts = accounts.difference(&gs.accounts).filter(|a| **a != me).count();
        // Who came and went, for the room's own timeline. Not on the first merge (seq 0: creating or joining), which only
        // sets the baseline, and not for a group this device has just been taken out of.
        let came: Vec<String> = accounts.difference(&gs.accounts).filter(|a| **a != me).cloned().collect();
        let went: Vec<String> = gs.accounts.difference(&accounts).filter(|a| **a != me).cloned().collect();
        let had_baseline = !gs.accounts.is_empty();
        gs.accounts = accounts;
        let kind = gs.kind.clone();
        let comm = gs.community.clone();
        if !active { gs.removed = true; }
        if seq > 0 && had_baseline && active && kind == Kind::Channel {
            // At the commit's place in the log, so the line sits between the messages around it.
            for a in came { self.add_system_msg_at(gid, seq, format!("{MEMBER_CAME}{a}")); }
            for a in went { self.add_system_msg_at(gid, seq, format!("{MEMBER_WENT}{a}")); }
        }
        let my_dev = self.my_device();
        if kind == Kind::Control {
            let certs = members.iter().filter(|(_, c)| added.contains(&c.device) && c.device != my_dev).map(|(_, c)| c.clone()).collect::<Vec<_>>();
            for c in certs { let n = self.display_name(&c.account); self.notice("device", format!("{} has a new device ({}...) in this community", n, &c.device[..8])); }
        }
        if (new_accounts > 0 || added.iter().any(|d| *d != my_dev)) && kind == Kind::Control { self.st.profile_dirty = true; }
        if kind == Kind::Control {
            if let Some(ext) = mls::group_ext(group.extensions()) {
                if let Ok((owner, admins)) = ext.verify_control(gid) {
                    if let GroupExt::Control { owner: founder, name, nonce, channels, invites, banned, transfers, consents, style, .. } = ext {
                        // Is this a hidden one-to-one community, and is it still exactly one? (direct.rs)
                        let accts = self.st.groups[gid].accounts.clone();
                        let known = self.st.direct_peers.get(gid).cloned();
                        let (direct, direct_bad) = crate::direct::mark(&founder, &nonce, &me, &accts, &channels, known.as_deref());
                        let c = self.st.communities.entry(gid.to_string()).or_insert_with(|| Community { id: gid.into(), ..Default::default() });
                        c.name = name; c.owner = owner; c.admins = admins; c.nonce = nonce; c.channels = channels; c.invites = invites; c.banned = banned; c.transfers = transfers;
                        c.founder = founder; c.direct = direct; c.direct_bad = direct_bad;
                        if let Some(ss) = style.as_ref() { if let Ok(doc) = verify_style(&c.owner, gid, ss) { c.adopt_style(doc, ss); } }
                        // The removal itself may take our entry out of the table: what we knew before it is what gets withdrawn.
                        if let Some(mine) = consents.iter().find(|x| x.account == me && verify_consent(gid, x).is_ok()) { c.my_consent = Some(mine.clone()); }
                        else if active { c.my_consent = None; }
                        c.consent_queue.retain(|a, q| !consents.iter().any(|x| &x.account == a && x.ts >= q.ts));
                        let leaving: Vec<String> = c.leaving.iter().cloned().collect();
                        let acct_set = self.st.groups[gid].accounts.clone();
                        let c = self.st.communities.get_mut(gid).unwrap();
                        for a in leaving { if !acct_set.contains(&a) { c.leaving.remove(&a); } }
                        c.kick_queue.retain(|a| acct_set.contains(a));
                        c.consent_queue.retain(|a, _| acct_set.contains(a));
                    }
                }
            }
            if !active {
                let mut taken_back = None;
                if let Some(c) = self.st.communities.get_mut(&comm) { c.removed = true; c.removed_reason = "You were removed from this community.".into(); taken_back = c.my_consent.take(); }
                if let Some(c) = taken_back { self.withdraw_consent(c.sig); }
                let chans: Vec<String> = self.st.groups.values().filter(|g| g.community == comm).map(|g| g.gid.clone()).collect();
                for g in chans { if let Some(x) = self.st.groups.get_mut(&g) { x.removed = true; } }
            }
        }
        self.activity += 1;
        // Membership, roles and the room's own status all show in the `messages` window.
        self.bump_chan(gid);
        self.bump_comm_chans(&comm);
    }

    // ------------------------------------------------------------------ authorization of membership changes
    /// Owner and admins as authenticated by this group's OWN group-context extension. Authorization must never
    /// depend on other groups' state: otherwise two honest clients that processed logs in different interleavings
    /// could disagree about one commit and fork.
    fn roles(&self, gid: &str, group: &MlsGroup) -> (String, Vec<String>) {
        let gs = &self.st.groups[gid];
        if let Some(ext) = mls::group_ext(group.extensions()) {
            if gs.kind == Kind::Control { if let Ok(r) = ext.verify_control(gid) { return r; } }
            else if let Ok((_, owner, admins)) = ext.verify_channel() { return (owner, admins); }
        }
        (String::new(), vec![])
    }

    fn authorize(&self, gid: &str, group: &MlsGroup, sender: &DeviceCert, sc: &StagedCommit) -> Verdict {
        use Verdict::*;
        let (owner, admins) = self.roles(gid, group);
        let role = |a: &str| if a == owner { Role::Owner } else if admins.iter().any(|x| x == a) { Role::Admin } else { Role::Member };
        let srole = role(&sender.account);
        let members = mls::members_of(group);
                let sender_leaf = members.iter().find(|(_, c)| c.device == sender.device).map(|(i, _)| *i);
        for qp in sc.queued_proposals() {
            if !matches!(qp.sender(), Sender::Member(i) if Some(*i) == sender_leaf) { return Reject("commit includes a proposal from someone else".into()); }
            match qp.proposal() {
                Proposal::Add(a) => {
                    let kp = a.key_package();
                    let Some(c) = mls::cert_of(kp.leaf_node().credential(), kp.leaf_node().signature_key().as_slice()) else { return Reject("added member has an invalid certificate".into()) };
                    if srole < Role::Admin && c.account != sender.account { return Reject(format!("{} may not add other people", srole.name())); }
                }
                Proposal::Remove(r) => {
                    let Some((_, t)) = members.iter().find(|(i, _)| *i == r.removed()) else { return Reject("removes unknown member".into()) };
                    let trole = role(&t.account);
                    let ok = t.account == sender.account
                        || (srole == Role::Owner && trole != Role::Owner)
                        || (srole == Role::Admin && trole == Role::Member);
                    if !ok { return Reject(format!("{} may not remove a {}", srole.name(), trole.name())); }
                }
                Proposal::GroupContextExtensions(e) => {
                    let Some(new) = mls::group_ext(e.extensions()) else { return Reject("bad group extension".into()) };
                    let Some(old) = mls::group_ext(group.extensions()) else { return Reject("no old extension".into()) };
                    if let Err(w) = ext_change_allowed(gid, &old, &new, srole, &sender.account) { return Reject(w); }
                }
                _ => return Reject("unsupported proposal type".into()),
            }
        }
        Ok
    }

    /// A commit by someone other than the owner that takes the owner's signed style document out of the table, or puts an older one
    /// back (audit M1). It is merged all the same, because versions before 0.3.6 merge it and refusing would split the community;
    /// this device keeps the newer document it holds, and admins put it back (ops.rs). The owner can see who did it and act.
    fn style_warning(&self, gid: &str, group: &MlsGroup, sender: &DeviceCert, sc: &StagedCommit) -> Option<String> {
        if self.st.groups.get(gid)?.kind != Kind::Control { return None; }
        let (owner, _) = self.roles(gid, group);
        if sender.account == owner { return None; }
        let old = mls::group_ext(group.extensions())?;
        let new = sc.queued_proposals().find_map(|qp| match qp.proposal() { Proposal::GroupContextExtensions(e) => mls::group_ext(e.extensions()), _ => None })?;
        let (GroupExt::Control { style: st1, .. }, GroupExt::Control { style: st2, .. }) = (&old, &new) else { return None };
        let od = st1.as_ref().and_then(|s| verify_style(&owner, gid, s).ok())?;
        let regressed = match st2.as_ref().and_then(|s| verify_style(&owner, gid, s).ok()) {
            None => true,
            Some(nd) => (nd.ver, nd.ts) < (od.ver, od.ts),
        };
        regressed.then(|| format!("{} put an older version of the owner's colours and wall settings in the table (or none); this device keeps the newer one and the admins put it back", self.display_name(&sender.account)))
    }

    // ------------------------------------------------------------------ application envelopes
    fn apply_env(&mut self, gid: &str, seq: u64, cert: &DeviceCert, env: Envelope) {
        let kind = self.st.groups[gid].kind.clone();
        let comm = self.st.groups[gid].community.clone();
        let acct = cert.account.clone();
        match (&kind, env) {
            (Kind::Control, Envelope::Profile { name, ts, listen, avatar, avatar_cleared, direct, direct_sealed }) => {
                let ts = sane_ts(ts);
                let newer = {
                    let p = self.st.profiles.entry(acct.clone()).or_default();
                    if ts >= p.ts {
                        p.name = name;
                        p.ts = ts;
                        p.direct = direct;
                        p.direct_sealed = direct_sealed;
                        // An older app sends this same message with neither picture field. That must not wipe a picture
                        // set from a newer one, so only a picture or an explicit removal changes it.
                        if avatar_cleared { p.avatar = None; }
                        else if avatar.is_some() { p.avatar = avatar; }
                        true
                    } else { false }
                };
                // A status is only as good as the profile it rode in on: an older one, arriving late, changes nothing.
                if newer {
                    // A name shows on every message of that person, in every room: bump them all.
                    self.bump_all_chans();
                    match listen {
                        Some(l) => {
                            let now = crate::spotify::now_ms();
                            // The same sample again (a profile re-sent for another reason) is not a new reading of the position.
                            let rx_ms = match self.st.listening.get(&acct) {
                                Some(h) if l.spotify.is_some() && h.what.spotify.as_ref().map(|s| (&s.id, s.at_ms, s.paused)) == l.spotify.as_ref().map(|s| (&s.id, s.at_ms, s.paused)) && h.rx_ms > 0 => h.rx_ms,
                                _ => now,
                            };
                            self.st.listening.insert(acct, Heard { what: l, ts: now_secs(), rx_ms });
                        }
                        None => { self.st.listening.remove(&acct); }
                    }
                }
            }
            (Kind::Control, Envelope::Leave) => {
                if let Some(c) = self.st.communities.get_mut(&comm) {
                    if acct != c.owner { c.leaving.insert(acct); }
                }
            }
            (Kind::Control, Envelope::Revoked { account, device, sig }) => {
                // Only the account that owns the device may revoke it, and the device must be one of that account's
                // leaves in this group: otherwise any member could have the rest of the community drop a stranger's device.
                let target_ok = account == cert.account
                    && self.load_group(gid).is_some_and(|g| mls::members_of(&g).iter().any(|(_, c)| c.device == device && c.account == account));
                if !target_ok { self.notice("reject", "ignored a device revocation that is not from the device's own account".to_string()); }
                else if verify_revocation(&account, &device, &sig).is_ok() {
                    if device == self.my_device() { self.revoked_here(); }
                    self.st.revoked.insert(device, sig);
                } else { self.notice("reject", "ignored a device revocation with a bad signature".to_string()); }
            }
            (Kind::Control, Envelope::Consent { ts, sig, relay_secret, relay_secret_want }) => {
                // Only for one's own account, and it has to carry that account's signature, whoever passes it on.
                let con = Consent { account: acct.clone(), ts, sig };
                if verify_consent(gid, &con).is_err() { self.notice("reject", "ignored a consent with a bad signature".to_string()); }
                else {
                    // The relay's sign-up secret rides in the same envelope, sealed so that the rest of the community
                    // can read nothing. Only this account's own other devices may hand it to us, or ask for it.
                    if acct == self.my_account() && cert.device != self.my_device() {
                        if let Some(blob) = relay_secret {
                            if self.st.relay_invite.is_empty() {
                                if let Some(secret) = self.open_relay_secret(&blob) {
                                    self.st.relay_invite = secret;
                                    self.st.relay_secret_shared.clear();
                                    self.st.relay_secret_asked.remove(&comm);
                                    self.notice("relay", "this device received this server's sign-up secret from another of your devices");
                                }
                            }
                        }
                        if relay_secret_want && !self.st.relay_invite.is_empty() {
                            if let Err(e) = self.queue_relay_secret(&comm) { log::warn!("answer a request for the relay secret: {e}"); }
                        }
                    }
                    if let Some(c) = self.st.communities.get_mut(&comm) { c.consent_queue.insert(acct, con); }
                }
            }
            (Kind::Channel, Envelope::Live { kind, data, ts }) => self.take_live(gid, &acct, kind, data, ts),
            (Kind::Channel, Envelope::Ring { id, ts, to, end }) => self.take_ring(gid, &acct, id, ts, to, end),
            (Kind::Channel, Envelope::Direct { id, ts, to, nonce, link, sealed }) => self.take_direct(gid, &acct, id, ts, to, nonce, link, sealed),
            (Kind::Channel, Envelope::Read { upto }) => self.take_read(gid, &acct, upto),
            (Kind::Channel, Envelope::Typing) => { self.typing.insert(format!("{gid}|{acct}"), now_secs()); self.bump_chan(gid); }
            (Kind::Channel, env @ (Envelope::Text { .. } | Envelope::File { .. } | Envelope::Edit { .. } | Envelope::Delete { .. } | Envelope::React { .. })) => {
                self.apply_chat(gid, seq, cert, env, false);
            }
            (_, Envelope::Unknown) => self.notice("newer", "skipped a message of a kind this version of Megatron does not know: someone has a newer version. Update Megatron to see messages like it"),
            (k, e) => self.notice("reject", format!("ignored a {:?} message of the wrong kind for a {:?} group", std::mem::discriminant(&e), k)),
        }
    }

    /// Nothing in a live event is trusted: the kind and size were checked when it was parsed, the time is the sender's claim,
    /// and one sender cannot fill the buffer. It is kept for the UI to read once and never touches the history.
    fn take_live(&mut self, gid: &str, acct: &str, kind: String, data: String, ts: u64) { self.take_live_limited(gid, acct, kind, data, ts, LIVE_TAKE_PER_SEC) }
    /// The same for the unstored channel, whose senders may send more per second (`per_sec` is what we take from one).
    pub(crate) fn take_live_limited(&mut self, gid: &str, acct: &str, kind: String, data: String, ts: u64, per_sec: u32) {
        let now = now_secs();
        if ts + LIVE_MAX_AGE < now || ts > now + LIVE_MAX_AHEAD { return; }
        if !self.st.groups.get(gid).is_some_and(|g| !g.removed && g.accounts.contains(acct)) { return; }
        let slot = self.live_rate.entry((gid.to_string(), acct.to_string())).or_insert((now, 0));
        if slot.0 != now { *slot = (now, 0); }
        if slot.1 >= per_sec { return; }
        slot.1 += 1;
        if matches!(kind.as_str(), "wall.d" | "wall.s" | "cwall.d" | "cwall.s") { self.take_wall(gid, acct, &kind, &data); return; }
        if self.live_rate.len() > 512 { self.live_rate.retain(|_, v| v.0 + 5 >= now); }
        self.live_next += 1;
        self.live_in.push_back(LiveEvent { id: self.live_next, gid: gid.to_string(), account: acct.to_string(), kind, data, ts });
        while self.live_in.len() > LIVE_BUFFER { self.live_in.pop_front(); }
    }

    /// A ring is the sender's claim and nothing else: it must come from a current member of this voice room, be fresh, not be
    /// for someone else, and not exceed the sender's rate. Never from ourselves (our other devices ring nobody: we know). It is kept
    /// for the UI to read once; it never touches the history.
    fn take_ring(&mut self, gid: &str, acct: &str, id: String, ts: u64, to: Option<String>, end: bool) {
        let now = now_secs();
        let me = self.my_account();
        let member = self.st.groups.get(gid).is_some_and(|g| !g.removed && g.accounts.contains(acct));
        let voice = self.st.groups.get(gid).and_then(|g| self.st.communities.get(&g.community)).is_some_and(|c| c.channels.iter().any(|ch| ch.id == gid && ch.kind == ROOM_VOICE));
        if !voice { return; }
        if end {
            // taking a ring back: same freshness and membership rules, a looser rate, no "for me" (it just ends a ring we may hold)
            if acct == me || !member || ts + RING_MAX_AGE < now || ts > now + RING_MAX_AHEAD { return; }
            let q = self.ring_ends.entry(acct.to_string()).or_default();
            while q.front().is_some_and(|t| *t + 60 <= now) { q.pop_front(); }
            if q.len() >= RING_ENDS_PER_MIN { return; }
            q.push_back(now);
            self.push_ring(RingEvent { id: 0, gid: gid.to_string(), account: acct.to_string(), ring: id, ts, end: true, direct: to.is_some() });
            return;
        }
        let recent: Vec<u64> = self.ring_rate.get(acct).map(|q| q.iter().copied().collect()).unwrap_or_default();
        if ring_check(now, ts, acct, &me, to.as_deref(), member, &recent) != RingVerdict::Show { return; }
        if self.ring_seen.contains_key(&id) { return; }
        self.ring_seen.retain(|_, t| *t + 120 > now);
        self.ring_seen.insert(id.clone(), now);
        let q = self.ring_rate.entry(acct.to_string()).or_default();
        while q.front().is_some_and(|t| *t + 60 <= now) { q.pop_front(); }
        q.push_back(now);
        if self.ring_rate.len() > 256 { self.ring_rate.retain(|_, q| !q.is_empty()); }
        self.push_ring(RingEvent { id: 0, gid: gid.to_string(), account: acct.to_string(), ring: id, ts, end: false, direct: to.is_some() });
    }
    fn push_ring(&mut self, mut ev: RingEvent) {
        self.ring_next += 1;
        ev.id = self.ring_next;
        self.ring_in.push_back(ev);
        while self.ring_in.len() > RING_BUFFER { self.ring_in.pop_front(); }
        self.activity += 1;
    }

    pub(crate) fn apply_chat(&mut self, gid: &str, seq: u64, cert: &DeviceCert, env: Envelope, local: bool) {
        self.bump_chan(gid);   // whatever this changes, the room's window is no longer what a page may have cached
        let comm = self.st.groups.get(gid).map(|g| g.community.clone()).unwrap_or_default();
        let is_admin = self.st.communities.get(&comm).and_then(|c| c.role_of(&cert.account)) >= Some(Role::Admin);
        let me = self.my_account();
        let log = self.st.chans.entry(gid.to_string()).or_default();
        let status = if cert.account == me { "sent" } else { "received" };
        match env {
            Envelope::Text { id, text, reply, ts } => {
                let ts = sane_ts(ts);
                if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) { if local { return; } if m.sender == cert.account { m.seq = seq; m.status = "sent".into(); } return; }
                log.msgs.push(Msg { id: id.clone(), sender: cert.account.clone(), device: cert.device.clone(), ts, seq, body: MsgBody::Text(text), reply, edited: false, deleted: false, reactions: Default::default(), status: if local { "sending".into() } else { status.into() } });
                Self::drain_orphans(log, &id, gid);
            }
            Envelope::File { id, name, mime, size, blob, key, nonce, caption, reply, ts, large, voice } => {
                let ts = sane_ts(ts);
                if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) { if local { return; } if m.sender == cert.account { m.seq = seq; m.status = "sent".into(); } return; }
                log.msgs.push(Msg { id: id.clone(), sender: cert.account.clone(), device: cert.device.clone(), ts, seq, body: MsgBody::File { name, mime, size, blob, key, nonce, caption, large, voice }, reply, edited: false, deleted: false, reactions: Default::default(), status: if local { "sending".into() } else { status.into() } });
                Self::drain_orphans(log, &id, gid);
            }
            Envelope::Edit { ref target, ref text, .. } => {
                match log.msgs.iter_mut().find(|m| &m.id == target) {
                    Some(m) if m.sender == cert.account && !m.deleted => { if let MsgBody::Text(t) = &mut m.body { *t = text.clone(); m.edited = true; } }
                    Some(_) => {}
                    None => Self::orphan(log, env, &cert.account, seq),
                }
            }
            Envelope::Delete { ref target } => {
                match log.msgs.iter_mut().find(|m| &m.id == target) {
                    Some(m) if m.sender == cert.account || is_admin => { m.deleted = true; m.body = MsgBody::Text(String::new()); m.reactions.clear(); }
                    Some(_) => {}
                    None => Self::orphan(log, env, &cert.account, seq),
                }
            }
            Envelope::React { ref target, ref emoji, on } => {
                match log.msgs.iter_mut().find(|m| &m.id == target) {
                    Some(m) if !m.deleted => set_reaction(m, emoji, &cert.account, on),
                    Some(_) => {}
                    None => Self::orphan(log, env, &cert.account, seq),
                }
            }
            _ => {}
        }
    }
    fn orphan(log: &mut ChannelLog, env: Envelope, acct: &str, seq: u64) {
        if log.orphans.len() < 500 { log.orphans.push((env, acct.to_string(), seq)); }
    }
    pub(crate) fn drain_orphans(log: &mut ChannelLog, id: &str, _gid: &str) {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut log.orphans).into_iter().partition(|(e, _, _)| match e {
            Envelope::Edit { target, .. } | Envelope::Delete { target } | Envelope::React { target, .. } => target == id, _ => false });
        log.orphans = rest;
        for (e, acct, _) in mine {
            if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) {
                match e {
                    Envelope::Edit { text, .. } if m.sender == acct => { if let MsgBody::Text(t) = &mut m.body { *t = text; m.edited = true; } }
                    Envelope::Delete { .. } if m.sender == acct => { m.deleted = true; m.body = MsgBody::Text(String::new()); }
                    Envelope::React { emoji, on, .. } => set_reaction(m, &emoji, &acct, on),
                    _ => {}
                }
            }
        }
    }
}

/// A sender's clock is the sender's claim: it may be a little ahead, but a message cannot come from the future.
/// Old timestamps are fine, a message can wait in an outbox for days. Order always comes from the log.
pub fn sane_ts(ts: u64) -> u64 { ts.min(now_secs() + 300) }

/// One member cannot pile up reactions without limit. Everyone applies the same rule to the same log order, so
/// everyone ends up with the same reactions.
fn set_reaction(m: &mut Msg, emoji: &str, acct: &str, on: bool) {
    if on {
        let mine = m.reactions.values().filter(|w| w.contains(acct)).count();
        let known = m.reactions.contains_key(emoji);
        if (!known && m.reactions.len() >= MAX_REACTIONS) || mine >= MAX_REACTIONS_PER_ACCOUNT { return; }
    }
    let s = m.reactions.entry(emoji.to_string()).or_default();
    if on { s.insert(acct.to_string()); } else { s.remove(acct); }
    if s.is_empty() { m.reactions.remove(emoji); }
}

/// Made with the invite's secret key, so whoever made it holds the link.
pub(crate) fn join_signature_ok(inv: &InviteMeta, community: &str, jr: &JoinRequest) -> bool {
    use ed25519_dalek::Verifier as _;
    (|| -> Option<bool> {
        let pk = ed25519_dalek::VerifyingKey::from_bytes(&unhex32(&inv.pk).ok()?).ok()?;
        let sig = ed25519_dalek::Signature::from_slice(&hex::decode(&jr.sig).ok()?).ok()?;
        Some(pk.verify(&join_msg(community, &jr.cert.device, &jr.invite), &sig).is_ok())
    })().unwrap_or(false)
}

/// Off while versions before this rule are in use: they accept what this would refuse, so a hostile commit could split a
/// community between old and new apps, which is far worse than a stale colour. Until then the owner's device repairs a
/// rolled-back document within seconds (ops.rs). Turn on once older versions are gone.
const ENFORCE_STYLE_FORWARD: bool = false;

/// Rules for editing a group's authenticated table (shared by receivers and tests).
/// A step in the chain of owners speaks for itself: it is signed by the owner before, so the new owner may install it
/// in a group whose table has not caught up. Nothing else about a table can be changed that way.
pub fn ext_change_allowed(gid: &str, old: &GroupExt, new: &GroupExt, sender: Role, sender_account: &str) -> std::result::Result<(), String> {
    match (old, new) {
        (GroupExt::Control { owner: o1, nonce: n1, admins: a1, name: nm1, channels: c1, invites: i1, banned: b1, transfers: t1, consents: s1, style: st1, .. }, GroupExt::Control { owner: o2, nonce: n2, admins: a2, name: nm2, channels: c2, invites: i2, banned: b2, transfers: t2, consents: s2, style: st2, .. }) => {
            if o1 != o2 || n1 != n2 { return Err("the founder cannot change".into()); }
            let (new_owner, _) = new.verify_control(gid).map_err(|e| e.to_string())?;
            if !t2.starts_with(t1) { return Err("the chain of owners cannot be rewritten".into()); }
            let owner = sender == Role::Owner || new_owner == sender_account;
            if t1 != t2 && !owner { return Err("only the owner can hand over a community".into()); }
            if a1 != a2 && !owner { return Err("only the owner can change admins".into()); }
            if (nm1 != nm2 || c1 != c2 || i1 != i2 || b1 != b2 || s1 != s2) && sender < Role::Admin { return Err("only admins can change channels, invites or the name".into()); }
            if s2.iter().filter(|c| !s1.contains(c)).any(|c| verify_consent(gid, c).is_err()) { return Err("a consent in the table is not signed by its account".into()); }
            // The owner's colours, wall locks and wall generations. Anyone but the owner may carry a document only forward: one the
            // owner signed, newer than the one in the table. Putting an older one back would hand it to every device that reads the
            // table afresh (a new member, a re-added device). Leaving the field out is still allowed, because an older app that does
            // not know it rewrites the table without it; the owner's device puts it back (ops.rs) and members keep the one they hold.
            if ENFORCE_STYLE_FORWARD && st1 != st2 && !owner {
                if let Some(n) = st2 {
                    let nd = verify_style(&new_owner, gid, n).map_err(|_| "only the owner can change the colours".to_string())?;
                    if let Some(od) = st1.as_ref().and_then(|o| verify_style(&new_owner, gid, o).ok()) {
                        if (nd.ver, nd.ts) <= (od.ver, od.ts) { return Err("the colours can only move forward".into()); }
                    }
                }
            }
            Ok(())
        }
        (GroupExt::Channel { community: c1, owner: o1, nonce: n1, admins: a1, name: nm1, transfers: t1 }, GroupExt::Channel { community: c2, owner: o2, nonce: n2, admins: a2, name: nm2, transfers: t2 }) => {
            if c1 != c2 || o1 != o2 || n1 != n2 { return Err("channel identity cannot change".into()); }
            let (_, new_owner, _) = new.verify_channel().map_err(|e| e.to_string())?;
            if !t2.starts_with(t1) { return Err("the chain of owners cannot be rewritten".into()); }
            let owner = sender == Role::Owner || new_owner == sender_account;
            if (a1 != a2 || t1 != t2) && !owner { return Err("only the owner can change admins".into()); }
            if nm1 != nm2 && sender < Role::Admin { return Err("only admins can rename".into()); }
            Ok(())
        }
        _ => Err("a group cannot change kind".into()),
    }
}
