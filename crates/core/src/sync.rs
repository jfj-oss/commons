//! Receiving side: relay sync, Welcome handling, strictly ordered log processing, commit authorization.
use crate::client::*;
use crate::envelope::*;
use crate::mls;
use crate::state::*;
use anyhow::{anyhow, bail, Result};
use commons_proto::*;
use openmls::prelude::{tls_codec::*, *};
use std::collections::BTreeMap;

pub const BATCH: usize = 300;

enum Verdict { Ok, Reject(String) }

impl Client {
    pub(crate) fn load_group(&self, gid: &str) -> Option<MlsGroup> {
        let id = GroupId::from_slice(&hex::decode(gid).ok()?);
        MlsGroup::load(self.prov.storage(), &id).ok().flatten()
    }

    /// One round trip with the relay plus processing of everything it returned.
    /// `wait_ms` > 0 makes it a long-poll (the UI loop uses it; tests use 0).
    pub fn sync_once(&mut self, wait_ms: u64) -> Result<()> {
        let k = self.dev_key()?;
        let mut cursors = BTreeMap::new();
        for g in self.st.groups.values().filter(|g| !g.removed) { cursors.insert(g.gid.clone(), g.cursor); }
        let req = SyncReq { cursors, ack_welcomes: std::mem::take(&mut self.st.ack_welcomes), wait_ms };
        let resp = match self.api.sync(&k, &req) {
            Ok(r) => r,
            Err(e) => { self.st.ack_welcomes = req.ack_welcomes; return Err(e); }
        };
        let mut resp = resp;
        for w in &resp.welcomes {
            let data = match unb64(&w.data) { Ok(d) => d, Err(_) => { self.st.ack_welcomes.push(w.id.clone()); continue; } };
            if self.st.parked_welcomes.iter().any(|p| p.id == w.id) { self.st.ack_welcomes.push(w.id.clone()); continue; }
            match self.handle_welcome(&w.id, &data) {
                Ok(true) => {}
                Ok(false) => self.st.parked_welcomes.push(ParkedWelcome { id: w.id.clone(), data: w.data.clone(), ts: now_secs() }),
                Err(e) => self.notice("welcome", format!("ignored an invitation to a group: {e}")),
            }
            self.st.ack_welcomes.push(w.id.clone());
        }
        self.retry_parked();
        let mut order: Vec<String> = self.st.groups.values().filter(|g| !g.removed).map(|g| g.gid.clone()).collect();
        order.sort_by_key(|g| (self.st.groups[g].kind != Kind::Control, g.clone()));
        let mut ctl_ok: BTreeMap<String, bool> = BTreeMap::new();
        for gid in order {
            let kind = self.st.groups[&gid].kind.clone();
            let comm = self.st.groups[&gid].community.clone();
            if kind == Kind::Channel && ctl_ok.get(&comm) == Some(&false) { continue; }
            let entries = resp.messages.remove(&gid).unwrap_or_default();
            let ok = self.process_entries(&gid, entries);
            if kind == Kind::Control { ctl_ok.insert(comm, ok); }
        }
        for gid in &resp.denied {
            if self.st.groups.get(gid).is_some_and(|g| !g.removed && !g.denied) {
                self.st.groups.get_mut(gid).unwrap().denied = true;
                self.notice("relay", format!("the relay no longer lets this account read group {} (you were probably removed while offline, or the relay is misbehaving)", &gid[..8.min(gid.len())]));
            }
        }
        for g in self.st.groups.values_mut() { if g.denied && !resp.denied.contains(&g.gid) { g.denied = false; } }
        self.save()
    }

    /// Returns false if the log had a gap or processing could not complete (caller must not trust dependents this round).
    fn process_entries(&mut self, gid: &str, mut entries: Vec<Entry>) -> bool {
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
                    TAG_REJOIN | TAG_JOIN_REQUEST => {
                        let g = self.st.groups.get_mut(gid).unwrap();
                        if g.requests.len() < 200 { g.requests.push(ReqItem { seq: e.seq, tag: data[0], data: b64(&data[1..]), tries: 0 }); }
                    }
                    t => self.notice("parse", format!("unknown log entry type {t} in group {} ignored", &gid[..8])),
                }
            }
            self.st.groups.get_mut(gid).unwrap().cursor = e.seq;
            self.activity += 1;
        }
        if complete && total < BATCH { self.synced.insert(gid.to_string()); } else if !complete { self.synced.remove(gid); }
        complete
    }

    fn retry_parked(&mut self) {
        let parked = std::mem::take(&mut self.st.parked_welcomes);
        let now = now_secs();
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
    fn handle_welcome(&mut self, _id: &str, bytes: &[u8]) -> Result<bool> {
        let msg = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| anyhow!("malformed welcome: {e}"))?;
        let welcome = match msg.extract() { MlsMessageBodyIn::Welcome(w) => w, _ => bail!("not a welcome message") };
        let jb = StagedWelcome::build_from_welcome(&self.prov, &mls::join_cfg(), welcome).map_err(|e| anyhow!("cannot open welcome: {e}"))?;
        let wgid = hex::encode(jb.processed_welcome().unverified_group_info().group_id().as_slice());
        let replace = self.st.groups.get(&wgid).is_some_and(|g| g.broken);
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
        let my_dev = self.my_device();
        // A community we never asked for is only accepted when another device of ours is already in it
        // (that is what an authentic 'second device' or 'recovered device' welcome looks like).
        let other_device_of_mine = staged.members().any(|m| mls::cert_of(&m.credential, &m.signature_key).is_some_and(|c| c.account == me && c.device != my_dev));
        let existing_broken = self.st.groups.get(&gid).is_some_and(|g| g.broken);
        if self.st.groups.get(&gid).is_some_and(|g| !g.broken && !g.removed) { return Ok(true); } // duplicate welcome
        let (kind, community) = match &ext {
            GroupExt::Control { .. } => {
                let (owner, admins) = ext.verify_control(&gid).map_err(|e| anyhow!("control table rejected: {e}"))?;
                let sender_ok = sender_cert.account == owner || admins.contains(&sender_cert.account) || sender_cert.account == me;
                if !sender_ok { bail!("welcome to a community from someone who is not its owner or admin"); }
                let expected = self.st.joining.get(&gid).is_some_and(|l| l.owner == owner) || self.st.communities.get(&gid).is_some_and(|c| c.owner == owner) || other_device_of_mine;
                if !expected { bail!("unsolicited community invitation (no invite link of ours matches it)"); }
                (Kind::Control, gid.clone())
            }
            GroupExt::Channel { community, .. } => {
                let (_, ch_owner, ch_admins) = ext.verify_channel().map_err(|e| anyhow!("channel table rejected: {e}"))?;
                if let Some(c) = self.st.communities.get(community) { if c.owner != ch_owner { bail!("channel names a different owner than its community"); } }
                // The key material of a welcome is consumed when it is opened, so it cannot be parked and retried.
                // If the community table has not arrived yet we join anyway; the channel stays invisible until
                // the (authenticated) community table lists it, and commits in it are judged by that table.
                if let Some(c) = self.st.communities.get(community) {
                    let _ = c;
                    let ok = sender_cert.account == ch_owner || ch_admins.contains(&sender_cert.account) || sender_cert.account == me;
                    if !ok { bail!("channel welcome from someone who is not an admin"); }
                }
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
            self.st.communities.entry(gid.clone()).or_insert_with(|| Community { id: gid.clone(), ..Default::default() });
            self.st.joining.remove(&gid);
            self.st.join_sent.remove(&gid);
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

    pub(crate) fn add_system_msg(&mut self, gid: &str, text: String) {
        let Some(g) = self.st.groups.get(gid) else { return };
        if g.kind != Kind::Channel { return; }
        let m = Msg { id: rand_hex(16), sender: String::new(), device: String::new(), ts: now_secs(), seq: 0, body: MsgBody::Notice(text), reply: None, edited: false, deleted: false, reactions: Default::default(), status: "system".into() };
        self.st.chans.entry(gid.to_string()).or_default().msgs.push(m);
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

    fn drop_pending(&mut self, gid: &str, why: &str) {
        let g = self.st.groups.get_mut(gid).unwrap();
        if let Some(p) = g.pending.take() { self.notice("commit", format!("group {}: our pending change ({}) was not applied: {why}; it will be re-evaluated", &gid[..8], p.desc)); }
    }

    fn finish_own_commit(&mut self, gid: &str, seq: u64) {
        let mut added = vec![];
        self.st.groups.get_mut(gid).unwrap().tainted = false; // a fresh epoch means fresh sender keys
        if let Some(p) = self.st.groups.get_mut(gid).unwrap().pending.take() {
            added = p.recipients.clone();
            self.applied.insert(p.msg_id.clone());
            if let Some(w) = p.welcome {
                for dev in p.recipients { self.st.welcome_out.push(WelcomeOut { id: rand_hex(16), device: dev, data: w.clone(), gid: gid.into() }); }
            }
            if !p.acl_remove.is_empty() { self.st.acl_jobs.push(AclJob { gid: gid.into(), remove: p.acl_remove, after: now_secs() + self.acl_delay_secs }); }
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
        gs.accounts = accounts;
        let kind = gs.kind.clone();
        let comm = gs.community.clone();
        if !active { gs.removed = true; }
        let my_dev = self.my_device();
        if kind == Kind::Control {
            let certs = members.iter().filter(|(_, c)| added.contains(&c.device) && c.device != my_dev).map(|(_, c)| c.clone()).collect::<Vec<_>>();
            for c in certs { let n = self.display_name(&c.account); self.notice("device", format!("{} has a new device ({}...) in this community", n, &c.device[..8])); }
        }
        if (new_accounts > 0 || added.iter().any(|d| *d != my_dev)) && kind == Kind::Control { self.st.profile_dirty = true; }
        if kind == Kind::Control {
            if let Some(ext) = mls::group_ext(group.extensions()) {
                if let Ok((owner, admins)) = ext.verify_control(gid) {
                    if let GroupExt::Control { name, nonce, channels, invites, banned, .. } = ext {
                        let c = self.st.communities.entry(gid.to_string()).or_insert_with(|| Community { id: gid.into(), ..Default::default() });
                        c.name = name; c.owner = owner; c.admins = admins; c.nonce = nonce; c.channels = channels; c.invites = invites; c.banned = banned;
                        let leaving: Vec<String> = c.leaving.iter().cloned().collect();
                        let acct_set = self.st.groups[gid].accounts.clone();
                        let c = self.st.communities.get_mut(gid).unwrap();
                        for a in leaving { if !acct_set.contains(&a) { c.leaving.remove(&a); } }
                        c.kick_queue.retain(|a| acct_set.contains(a));
                    }
                }
            }
            if !active {
                if let Some(c) = self.st.communities.get_mut(&comm) { c.removed = true; c.removed_reason = "You were removed from this community.".into(); }
                let chans: Vec<String> = self.st.groups.values().filter(|g| g.community == comm).map(|g| g.gid.clone()).collect();
                for g in chans { if let Some(x) = self.st.groups.get_mut(&g) { x.removed = true; } }
            }
        }
        self.activity += 1;
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
                    if let Err(w) = ext_change_allowed(gid, &old, &new, srole) { return Reject(w); }
                }
                _ => return Reject("unsupported proposal type".into()),
            }
        }
        Ok
    }

    // ------------------------------------------------------------------ application envelopes
    fn apply_env(&mut self, gid: &str, seq: u64, cert: &DeviceCert, env: Envelope) {
        let kind = self.st.groups[gid].kind.clone();
        let comm = self.st.groups[gid].community.clone();
        let acct = cert.account.clone();
        match (&kind, env) {
            (Kind::Control, Envelope::Profile { name, ts }) => {
                let p = self.st.profiles.entry(acct).or_default();
                if ts >= p.ts { p.name = name; p.ts = ts; }
            }
            (Kind::Control, Envelope::Leave) => {
                if let Some(c) = self.st.communities.get_mut(&comm) {
                    if acct != c.owner { c.leaving.insert(acct); }
                }
            }
            (Kind::Control, Envelope::Revoked { account, device, sig }) => {
                if verify_revocation(&account, &device, &sig).is_ok() { self.st.revoked.insert(device, sig); }
                else { self.notice("reject", "ignored a device revocation with a bad signature".to_string()); }
            }
            (Kind::Channel, Envelope::Typing) => { self.typing.insert(format!("{gid}|{acct}"), now_secs()); }
            (Kind::Channel, env @ (Envelope::Text { .. } | Envelope::File { .. } | Envelope::Edit { .. } | Envelope::Delete { .. } | Envelope::React { .. })) => {
                self.apply_chat(gid, seq, cert, env, false);
            }
            (k, e) => self.notice("reject", format!("ignored a {:?} message of the wrong kind for a {:?} group", std::mem::discriminant(&e), k)),
        }
    }

    pub(crate) fn apply_chat(&mut self, gid: &str, seq: u64, cert: &DeviceCert, env: Envelope, local: bool) {
        let comm = self.st.groups.get(gid).map(|g| g.community.clone()).unwrap_or_default();
        let is_admin = self.st.communities.get(&comm).and_then(|c| c.role_of(&cert.account)) >= Some(Role::Admin);
        let me = self.my_account();
        let log = self.st.chans.entry(gid.to_string()).or_default();
        let status = if cert.account == me { "sent" } else { "received" };
        match env {
            Envelope::Text { id, text, reply, ts } => {
                if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) { if local { return; } if m.sender == cert.account { m.seq = seq; m.status = "sent".into(); } return; }
                log.msgs.push(Msg { id: id.clone(), sender: cert.account.clone(), device: cert.device.clone(), ts, seq, body: MsgBody::Text(text), reply, edited: false, deleted: false, reactions: Default::default(), status: if local { "sending".into() } else { status.into() } });
                Self::drain_orphans(log, &id, gid);
            }
            Envelope::File { id, name, mime, size, blob, key, nonce, caption, reply, ts } => {
                if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) { if local { return; } if m.sender == cert.account { m.seq = seq; m.status = "sent".into(); } return; }
                log.msgs.push(Msg { id: id.clone(), sender: cert.account.clone(), device: cert.device.clone(), ts, seq, body: MsgBody::File { name, mime, size, blob, key, nonce, caption }, reply, edited: false, deleted: false, reactions: Default::default(), status: if local { "sending".into() } else { status.into() } });
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
                    Some(m) if !m.deleted => {
                        let s = m.reactions.entry(emoji.clone()).or_default();
                        if on { s.insert(cert.account.clone()); } else { s.remove(&cert.account); }
                        if s.is_empty() { m.reactions.remove(emoji); }
                    }
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
    fn drain_orphans(log: &mut ChannelLog, id: &str, _gid: &str) {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut log.orphans).into_iter().partition(|(e, _, _)| match e {
            Envelope::Edit { target, .. } | Envelope::Delete { target } | Envelope::React { target, .. } => target == id, _ => false });
        log.orphans = rest;
        for (e, acct, _) in mine {
            if let Some(m) = log.msgs.iter_mut().find(|m| m.id == id) {
                match e {
                    Envelope::Edit { text, .. } if m.sender == acct => { if let MsgBody::Text(t) = &mut m.body { *t = text; m.edited = true; } }
                    Envelope::Delete { .. } if m.sender == acct => { m.deleted = true; m.body = MsgBody::Text(String::new()); }
                    Envelope::React { emoji, on, .. } => { let s = m.reactions.entry(emoji.clone()).or_default(); if on { s.insert(acct); } else { s.remove(&acct); } if s.is_empty() { m.reactions.remove(&emoji); } }
                    _ => {}
                }
            }
        }
    }
}

/// Rules for editing a group's authenticated table (shared by receivers and tests).
pub fn ext_change_allowed(gid: &str, old: &GroupExt, new: &GroupExt, sender: Role) -> std::result::Result<(), String> {
    match (old, new) {
        (GroupExt::Control { owner: o1, nonce: n1, admins: a1, name: nm1, channels: c1, invites: i1, banned: b1 }, GroupExt::Control { owner: o2, nonce: n2, admins: a2, name: nm2, channels: c2, invites: i2, banned: b2 }) => {
            if o1 != o2 || n1 != n2 { return Err("owner cannot change".into()); }
            new.verify_control(gid).map_err(|e| e.to_string())?;
            if a1 != a2 && sender != Role::Owner { return Err("only the owner can change admins".into()); }
            if (nm1 != nm2 || c1 != c2 || i1 != i2 || b1 != b2) && sender < Role::Admin { return Err("only admins can change channels, invites or the name".into()); }
            Ok(())
        }
        (GroupExt::Channel { community: c1, owner: o1, nonce: n1, admins: a1, name: nm1 }, GroupExt::Channel { community: c2, owner: o2, nonce: n2, admins: a2, name: nm2 }) => {
            if c1 != c2 || o1 != o2 || n1 != n2 { return Err("channel identity cannot change".into()); }
            new.verify_channel().map_err(|e| e.to_string())?;
            if a1 != a2 && sender != Role::Owner { return Err("only the owner can change admins".into()); }
            if nm1 != nm2 && sender < Role::Admin { return Err("only admins can rename".into()); }
            Ok(())
        }
        _ => Err("a group cannot change kind".into()),
    }
}
