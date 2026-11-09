//! The background loop, in rounds. A round has three steps:
//!   begin_round  (holds the client)  plans: reconcile, encrypt, pick what to send. Local work and disk only.
//!   Round::run   (holds nothing)     talks to the relay. All the waiting happens here.
//!   finish_round (holds the client)  applies what came back. Local work and disk only.
//! So a slow or dead relay never keeps the client from the UI. Anything may happen to the client between the steps:
//! every result is matched to current state by id, and posting twice is harmless because the relay de-duplicates.
use crate::client::*;
use crate::envelope::*;
use crate::state::*;
use crate::transport::*;
use anyhow::Result;
use commons_proto::*;
use ed25519_dalek::{Signer as _, SigningKey};

struct CommitPost { gid: String, msg_id: String, data: Vec<u8>, acl: Option<AclReq> }
struct AclCheck { gid: String, expect: Vec<String>, me: String, grant: Option<String>, transfers: Vec<OwnerStep> }

#[derive(Default)]
pub struct Round {
    net: Option<(Api, SigningKey)>,
    anon: Option<Api>,
    register: Option<(DeviceCert, Option<String>)>,
    kp_count: Option<String>,
    publish: Option<(Vec<Vec<u8>>, Option<Vec<u8>>)>,
    pair_poll: Option<String>,
    pair_ack: Option<(String, String)>,
    joins: Vec<(String, Vec<u8>)>,
    commits: Vec<CommitPost>,
    posts: Vec<(String, String, Vec<u8>)>,
    welcomes: Vec<WelcomeOut>,
    acls: Vec<(AclJob, AclReq)>,
    rejoins: Vec<(String, Vec<u8>)>,
    dirs: Vec<String>,
    kps: Vec<(String, u32)>,
    checks: Vec<AclCheck>,
    sync: Option<SyncReq>,
    live: Option<crate::live::LivePlan>,
    cap: u64,
}

#[derive(Default)]
pub struct RoundDone {
    /// The first thing that went wrong with the relay as a whole.
    failure: Option<anyhow::Error>,
    registered: bool,
    kp_have: Option<u32>,
    published: bool,
    sealed: Option<Vec<u8>>,
    pair_acked: bool,
    joins: Vec<(String, Result<()>)>,
    commits: Vec<(String, String, Result<()>)>,
    posts: Vec<(String, Result<PostResp>)>,
    welcomes: Vec<(String, Result<()>)>,
    acls: Vec<(AclJob, Result<()>)>,
    rejoins: Vec<String>,
    dirs: Vec<(String, Vec<DeviceInfo>)>,
    kps: Vec<(String, Vec<u8>)>,
    repaired: Vec<(String, usize)>,
    sync: Option<(Result<SyncResp>, Vec<String>, Vec<String>)>,
    live: Option<crate::live::LiveDone>,
    cap: u64,
}

/// A network failure stops the round: the next request would only wait for the same timeout again.
fn is_down(e: &anyhow::Error) -> bool { !e.is::<RelayError>() && !is_unreadable(e) }

/// Ask for everything, then for less if the answer cannot be read, and in the end for one group at a time, so that
/// whatever is wrong with one group's entries stays with that group. Returns the budget to start from next time and
/// the groups that could not be read at all.
pub(crate) fn fetch_sync(api: &Api, k: &SigningKey, req: &SyncReq, cap: u64) -> (Result<SyncResp>, u64, Vec<String>) {
    let mut c = cap.clamp(SYNC_MIN_BYTES, SYNC_MAX_BYTES);
    loop {
        let mut r = req.clone();
        r.max_bytes = Some(c);
        match api.sync(k, &r) {
            Ok(resp) => return (Ok(resp), (c * 2).min(SYNC_MAX_BYTES), vec![]),
            Err(e) if !is_unreadable(&e) => return (Err(e), c, vec![]),
            Err(e) => {
                log::warn!("sync answer unreadable at budget {c}: {e}");
                if c == SYNC_MIN_BYTES { break; }
                c = (c / 4).max(SYNC_MIN_BYTES);
            }
        }
    }
    let mut merged = SyncResp::default();
    let mut bad = vec![];
    for (gid, cursor) in &req.cursors {
        let one = SyncReq { cursors: [(gid.clone(), *cursor)].into(), ack_welcomes: vec![], wait_ms: 0, max_bytes: Some(SYNC_MIN_BYTES) };
        match api.sync(k, &one) {
            Ok(r) => {
                merged.messages.extend(r.messages);
                merged.denied.extend(r.denied);
                merged.more.extend(r.more);
            }
            Err(e) if is_unreadable(&e) => bad.push(gid.clone()),
            Err(e) => return (Err(e), c, bad),
        }
    }
    (Ok(merged), c, bad)
}

impl Round {
    /// The network part. Holds nothing but its own copy of what it needs.
    pub fn run(self) -> RoundDone {
        let mut d = RoundDone { cap: self.cap, ..Default::default() };
        if let (Some(rid), Some(api)) = (&self.pair_poll, &self.anon) {
            match api.rv_get(rid, "a") { Ok(s) => d.sealed = s, Err(e) => d.failure = Some(e) }
        }
        let Some((api, k)) = &self.net else { return d };
        // Runs `f` unless the relay is already known to be out of reach.
        fn go<T>(d: &mut RoundDone, f: impl FnOnce() -> Result<T>) -> Option<Result<T>> {
            if d.failure.as_ref().is_some_and(is_down) { return None; }
            let r = f();
            if let Err(e) = &r { if is_down(e) || relay_status(e).is_some_and(|s| s == 401 || s == 403) { if d.failure.is_none() { d.failure = Some(clone_err(e)); } } }
            Some(r)
        }
        if let Some((cert, invite)) = &self.register {
            if let Some(Ok(())) = go(&mut d, || api.register(k, cert, invite.clone())) { d.registered = true; }
        }
        if let Some(dev) = &self.kp_count { if let Some(Ok(n)) = go(&mut d, || api.kp_count(k, dev)) { d.kp_have = Some(n); } }
        if let Some((pk, lr)) = self.publish { if let Some(Ok(())) = go(&mut d, || api.publish_kps(k, pk, lr)) { d.published = true; } }
        if let Some((rid, dev)) = &self.pair_ack {
            if let Some(Ok(())) = go(&mut d, || api.rv_put(k, rid, "b", dev.as_bytes())) { d.pair_acked = true; }
        }
        for (cid, data) in &self.joins {
            if let Some(r) = go(&mut d, || api.post_entry(k, cid, &rand_hex(16), data).map(|_| ())) { d.joins.push((cid.clone(), r)); }
        }
        for c in &self.commits {
            let r = go(&mut d, || {
                if let Some(a) = &c.acl { api.acl(k, &c.gid, a)?; }
                api.post_entry(k, &c.gid, &c.msg_id, &c.data).map(|_| ())
            });
            if let Some(r) = r { d.commits.push((c.gid.clone(), c.msg_id.clone(), r)); }
        }
        for (id, gid, data) in &self.posts {
            if let Some(r) = go(&mut d, || api.post_entry(k, gid, id, data)) { d.posts.push((id.clone(), r)); }
        }
        for w in &self.welcomes {
            let data = unb64(&w.data).unwrap_or_default();
            if let Some(r) = go(&mut d, || api.post_welcome(k, &w.device, &w.id, &data)) { d.welcomes.push((w.id.clone(), r)); }
        }
        for (job, req) in self.acls {
            if let Some(r) = go(&mut d, || api.acl(k, &job.gid, &req)) { d.acls.push((job, r)); }
        }
        for (gid, data) in &self.rejoins {
            if let Some(Ok(_)) = go(&mut d, || api.post_entry(k, gid, &rand_hex(16), data)) { d.rejoins.push(gid.clone()); }
        }
        for a in &self.dirs { if let Some(Ok(list)) = go(&mut d, || api.devices(k, a)) { d.dirs.push((a.clone(), list)); } }
        for (dev, n) in &self.kps {
            for _ in 0..*n {
                match go(&mut d, || api.take_kp(k, dev)) { Some(Ok(Some(b))) => d.kps.push((dev.clone(), b)), _ => break }
            }
        }
        for c in &self.checks { if let Some(Ok(n)) = go(&mut d, || check_acl(api, k, c)) { if n > 0 { d.repaired.push((c.gid.clone(), n)); } } }
        // The unstored live channel: its failures never fail the round (a relay without it just says 404).
        if let Some(lp) = self.live { if !d.failure.as_ref().is_some_and(is_down) { d.live = Some(lp.run(0)); } }
        if let Some(req) = self.sync {
            if d.failure.as_ref().is_some_and(is_down) {
                d.sync = Some((Err(clone_err(d.failure.as_ref().unwrap())), req.ack_welcomes, vec![]));
            } else {
                let (r, cap, bad) = fetch_sync(api, k, &req, self.cap);
                d.cap = cap;
                if let Err(e) = &r { if d.failure.is_none() { d.failure = Some(clone_err(e)); } }
                d.sync = Some((r, req.ack_welcomes, bad));
            }
        }
        d
    }
}

/// anyhow errors do not clone; the kind is what matters later, so keep that.
fn clone_err(e: &anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<RelayError>() {
        Some(r) => RelayError { what: r.what.clone(), status: r.status, body: r.body.clone() }.into(),
        None => anyhow::anyhow!("{e:#}"),
    }
}

/// Compare the relay's list for a group with the group's own. Someone who is in the group but not on the list has been
/// cut off (by a bad admin, or a relay that lost something): put them back. Returns how many.
fn check_acl(api: &Api, k: &SigningKey, c: &AclCheck) -> Result<usize> {
    let grant = c.grant.clone();
    let cur = match api.acl_get(k, &c.gid) {
        Ok(r) => r,
        // An admin who was cut off themselves cannot even look. Their grant lets them back in.
        Err(e) if relay_status(&e) == Some(403) => {
            api.acl(k, &c.gid, &AclReq { add: vec![c.me.clone()], grant: grant.clone(), ..Default::default() })?;
            api.acl_get(k, &c.gid)?
        }
        Err(e) => return Err(e),
    };
    // The relay may not have seen the latest handover yet; anyone holding the signed steps can show them.
    for t in c.transfers.iter().filter(|t| t.seq > cur.owner_seq) {
        api.acl(k, &c.gid, &AclReq { transfer: Some(t.clone()), grant: grant.clone(), ..Default::default() })?;
    }
    let missing: Vec<String> = c.expect.iter().filter(|a| !cur.members.contains(a)).cloned().collect();
    for part in missing.chunks(MAX_ACL_BATCH) {
        api.acl(k, &c.gid, &AclReq { add: part.to_vec(), grant: grant.clone(), ..Default::default() })?;
    }
    Ok(missing.len())
}

impl Client {
    /// Plans one round. Local work and disk only.
    pub fn begin_round(&mut self, wait_ms: u64) -> Round {
        let mut r = Round { cap: self.sync_cap, ..Default::default() };
        if !self.has_identity() {
            r.anon = Some(self.api.clone());
            r.pair_poll = self.st.pairing.as_ref().map(|p| p.rid.clone());
            return r;
        }
        if self.st.self_revoked { return r; } // the relay will not talk to this device any more, and it knows why
        let Ok(k) = self.dev_key() else { return r };
        r.net = Some((self.api.clone(), k.clone()));
        let now = now_secs();
        if self.last_dir_check.get("kp").is_none_or(|t| now >= t + 60) || !self.st.registered {
            self.last_dir_check.insert("kp".into(), now);
            if !self.st.registered { if let Ok(c) = self.cert() { r.register = Some((c, self.register_invite())); } }
            r.kp_count = Some(self.my_device());
        }
        if self.kp_refill {
            match self.build_key_packages() { Ok(p) => r.publish = Some(p), Err(e) => log::warn!("key packages: {e}") }
        }
        r.pair_ack = self.pair_ack.clone();
        self.plan_joins(&mut r, now);
        if self.st.profile_dirty && !self.st.name.is_empty() {
            self.st.profile_dirty = false;
            let name = self.st.name.clone();
            let me = self.my_account();
            // A profile refresh carries the listening status (or says plainly there is none) and the one picture per
            // account. A joining device gets both with the next broadcast. Older apps ignore the extra fields.
            let listen = if self.st.share_listening { self.listening_of(&me).cloned() } else { None };
            let avatar = self.st.profiles.get(&me).and_then(|p| p.avatar.clone());
            let avatar_cleared = std::mem::replace(&mut self.st.avatar_removed, false);
            let ts = self.st.profiles.get(&me).map_or_else(now_secs, |p| p.ts.max(now_secs()));
            let ctls: Vec<String> = self.st.groups.values().filter(|g| g.kind == Kind::Control && !g.removed && !g.broken).map(|g| g.gid.clone()).collect();
            for g in ctls { self.queue_env(&g, Envelope::Profile { name: name.clone(), ts, listen: listen.clone(), avatar: avatar.clone(), avatar_cleared }); }
        }
        self.flush_live();
        self.reconcile();
        self.plan_relay_secret();
        // nothing goes out unless the state behind it is on disk
        if let Err(e) = self.encrypt_outbox().and_then(|_| self.save()) { log::warn!("save before sending: {e}"); r.sync = Some(self.sync_req(wait_ms)); return r; }
        for g in self.st.groups.values() {
            if let Some(p) = g.pending.as_ref().filter(|p| !p.posted) {
                let acl = (!p.acl_add.is_empty()).then(|| AclReq { add: p.acl_add.clone(), grant: self.my_grant(&g.community), ..Default::default() });
                r.commits.push(CommitPost { gid: g.gid.clone(), msg_id: p.msg_id.clone(), data: unb64(&p.data).unwrap_or_default(), acl });
            }
        }
        for it in self.st.outbox.iter().filter(|i| i.cipher.is_some()).take(200) {
            r.posts.push((it.id.clone(), it.gid.clone(), unb64(it.cipher.as_ref().unwrap()).unwrap_or_default()));
        }
        r.welcomes = self.st.welcome_out.clone();
        for j in self.st.acl_jobs.iter().filter(|j| now >= j.after) {
            let cid = self.st.groups.get(&j.gid).map(|g| g.community.clone()).unwrap_or_else(|| j.gid.clone());
            r.acls.push((j.clone(), AclReq { add: j.add.clone(), remove: j.remove.clone(), demote: j.demote.clone(), promote: j.promote.clone(), call_remove: j.call_remove.clone(), transfer: j.transfer.clone(), grant: self.my_grant(&cid), ..Default::default() }));
        }
        self.plan_rejoins(&mut r, &k, now);
        self.dir_wanted.insert(self.my_account());
        r.dirs = std::mem::take(&mut self.dir_wanted).into_iter().collect();
        r.kps = std::mem::take(&mut self.kp_wanted).into_iter().take(64).collect();
        if self.last_dir_check.get("acl").is_none_or(|t| now >= t + self.acl_check_secs) {
            self.last_dir_check.insert("acl".into(), now);
            self.plan_acl_checks(&mut r);
        }
        r.live = self.live_plan(!self.live.background);
        r.sync = Some(self.sync_req(wait_ms));
        r
    }

    fn plan_joins(&mut self, r: &mut Round, now: u64) {
        // Asked long ago and nothing came of it: the request may have been dropped by a busy admin or lost by the relay.
        let stale: Vec<String> = self.st.join_sent.iter().filter(|c| self.st.join_asked.get(*c).is_some_and(|t| now >= t + 3600)).cloned().collect();
        for c in stale { self.st.join_sent.remove(&c); }
        let pending: Vec<InviteLink> = self.st.joining.values().filter(|l| !self.st.join_sent.contains(&l.community) && self.st.join_retry.get(&l.community).is_none_or(|t| now >= *t)).cloned().collect();
        for l in pending {
            match self.join_request_data(&l) { Ok(d) => r.joins.push((l.community.clone(), d)), Err(e) => log::warn!("join request: {e}") }
        }
    }

    fn plan_rejoins(&mut self, r: &mut Round, k: &SigningKey, now: u64) {
        let broken: Vec<String> = self.st.groups.values().filter(|g| g.broken && !g.removed && (!g.rejoin_sent || now > g.rejoin_ts + 300)).map(|g| g.gid.clone()).collect();
        for gid in broken {
            let Ok(cert) = self.cert() else { return };
            let nonce = rand_hex(8);
            let mut m = b"commons/rejoin/v1".to_vec(); m.extend_from_slice(&sha256(&[gid.as_bytes(), nonce.as_bytes()]));
            let sig = k.sign(&m);
            let body = serde_json::json!({"cert": cert, "nonce": nonce, "sig": hex::encode(sig.to_bytes())});
            let mut data = vec![TAG_REJOIN]; data.extend(serde_json::to_vec(&body).unwrap_or_default());
            r.rejoins.push((gid, data));
        }
    }

    /// Admins keep an eye on the relay's ACL for every group they are caught up on.
    fn plan_acl_checks(&mut self, r: &mut Round) {
        let me = self.my_account();
        for c in self.st.communities.values().filter(|c| !c.removed && c.role_of(&me).unwrap_or(Role::Member) >= Role::Admin) {
            let grant = self.my_grant(&c.id);
            for g in self.st.groups.values().filter(|g| g.community == c.id && !g.removed && !g.broken && g.pending.is_none() && self.synced.contains(&g.gid)) {
                r.checks.push(AclCheck { gid: g.gid.clone(), expect: g.accounts.iter().cloned().collect(), me: me.clone(), grant: grant.clone(), transfers: c.transfers.clone() });
            }
        }
    }

    /// What a failed request tells us about our standing with the relay.
    pub(crate) fn note_failure(&mut self, e: &anyhow::Error) {
        if relay_says(e, 403, "not registered") {
            if self.st.registered { self.relay_forgot_us(); }
        } else if relay_says(e, 403, "device revoked") {
            self.revoked_here();
        } else if relay_says(e, 401, "clock skew") {
            let theirs = e.downcast_ref::<RelayError>().and_then(|r| r.body.rsplit(' ').next().and_then(|t| t.parse::<u64>().ok()));
            let off = theirs.map(|t| t.abs_diff(now_secs()) / 60).unwrap_or(0);
            let text = format!("This computer's clock is about {off} minutes away from the relay's, and the relay refuses requests that far off. Set the correct time and date; nothing is lost in the meantime.");
            self.net = NetStatus { state: NetState::Offline, detail: "wrong clock: set the correct time".into() };
            self.notice("clock", text);
        }
    }

    pub(crate) fn revoked_here(&mut self) {
        if self.st.self_revoked { return; }
        self.st.self_revoked = true;
        self.notice("revoked", "This device has been revoked from your account, so it can no longer send or receive. What it already received stays readable here. If you did not do this, someone else holds your account key: use another device or your recovery key.");
    }

    /// Applies what came back. Local work and disk only.
    pub fn finish_round(&mut self, d: RoundDone) -> Result<()> {
        self.sync_cap = d.cap.max(SYNC_MIN_BYTES);
        self.more = false;
        if let Some(sealed) = &d.sealed { let _ = self.pairing_apply(sealed)?; }
        if d.pair_acked { self.pair_ack = None; }
        if d.registered { self.st.registered = true; self.activity += 1; }
        if d.kp_have.is_some_and(|n| n < 10) { self.kp_refill = true; self.more = true; }
        if d.published { self.kp_refill = false; self.st.lr_published = true; }
        for (cid, r) in d.joins { self.join_posted(&cid, r); }
        for (gid, msg_id, r) in d.commits { self.commit_posted(&gid, &msg_id, r); }
        let mut keep_going = false;
        for (id, r) in d.posts { keep_going |= self.item_posted(&id, r); }
        self.more |= keep_going;
        for (id, r) in d.welcomes {
            match r {
                Ok(()) => { self.st.welcome_out.retain(|w| w.id != id); self.activity += 1; }
                Err(e) if relay_status(&e) == Some(404) => self.st.welcome_out.retain(|w| w.id != id), // that device is gone
                Err(e) => log::warn!("welcome post failed, will retry: {e}"),
            }
        }
        for (job, r) in d.acls {
            match r {
                Ok(()) => self.st.acl_jobs.retain(|j| *j != job),
                Err(e) if relay_status(&e).is_some_and(|s| s / 100 == 4 && s != 429) => {
                    self.notice("relay", format!("the relay refused a change to group {}: {e}", &job.gid[..8.min(job.gid.len())]));
                    self.st.acl_jobs.retain(|j| *j != job);
                }
                Err(e) => log::warn!("acl change failed, will retry: {e}"),
            }
        }
        for gid in d.rejoins { if let Some(g) = self.st.groups.get_mut(&gid) { g.rejoin_sent = true; g.rejoin_ts = now_secs(); } }
        for (a, list) in d.dirs {
            let merged = self.merge_directory(&a, list);
            let same = self.dir_cache.get(&a).is_some_and(|(_, old)| old.len() == merged.len() && old.iter().zip(&merged).all(|(x, y)| x.0.device == y.0.device && x.1 == y.1));
            if !same { self.activity += 1; self.more = true; }
            self.dir_cache.insert(a, (now_secs(), merged));
        }
        if !d.kps.is_empty() { self.activity += 1; self.more = true; }
        for (dev, b) in d.kps { self.kp_cache.entry(dev).or_default().push(b); }
        for (gid, n) in d.repaired {
            self.notice("relay", format!("the relay had cut {n} member(s) off from group {}; let them back in", &gid[..8.min(gid.len())]));
        }
        if let Some(e) = &d.failure { self.note_failure(e); }
        if let Some(l) = d.live { self.live_finish(l); }
        let mut result = Ok(());
        match d.sync {
            Some((Ok(resp), _, bad)) => {
                self.net = NetStatus { state: NetState::Online, detail: String::new() };
                self.apply_sync(resp, &bad);
            }
            Some((Err(e), acks, _)) => {
                self.st.ack_welcomes.extend(acks);
                if self.net.detail.is_empty() || is_down(&e) { self.net = NetStatus { state: NetState::Offline, detail: format!("{e:#}") }; }
                result = Err(e);
            }
            None => if let Some(e) = d.failure { result = Err(e); },
        }
        self.save()?;
        result
    }

    fn join_posted(&mut self, cid: &str, r: Result<()>) {
        match r {
            Ok(()) => {
                let name = self.st.joining.get(cid).map(|l| l.name.clone()).unwrap_or_default();
                if self.st.join_sent.insert(cid.to_string()) { self.notice("join", format!("asked to join \"{name}\"; waiting for an admin to let you in")); }
                self.st.join_asked.insert(cid.to_string(), now_secs());
                self.st.join_retry.remove(cid);
            }
            // The community does not exist here, or this account may not knock on it: no point in asking again.
            Err(e) if relay_status(&e) == Some(403) && !relay_says(&e, 403, "not registered") && !relay_says(&e, 403, "revoked") => self.refuse_join(cid, "the relay does not know this community, so the invite may be out of date"),
            Err(e) if relay_status(&e) == Some(429) => { self.st.join_retry.insert(cid.to_string(), now_secs() + 120); log::info!("join request limited: {e}"); }
            Err(e) => { self.st.join_retry.insert(cid.to_string(), now_secs() + 10); log::warn!("join request: {e}"); }
        }
    }

    fn commit_posted(&mut self, gid: &str, msg_id: &str, r: Result<()>) {
        if self.st.groups.get(gid).and_then(|g| g.pending.as_ref()).is_none_or(|p| p.msg_id != msg_id) { return; }
        match r {
            Ok(()) => self.st.groups.get_mut(gid).unwrap().pending.as_mut().unwrap().posted = true,
            // A refusal that will not change. A commit left waiting would stop this group from sending anything.
            Err(e) if relay_status(&e).is_some_and(|s| s / 100 == 4 && s != 429) => {
                self.abandon_pending(gid, &format!("the relay refused it ({e})"));
            }
            Err(e) => log::warn!("commit post failed, will retry: {e}"),
        }
    }

    /// Returns true if more can go right away.
    fn item_posted(&mut self, id: &str, r: Result<PostResp>) -> bool {
        let Some(pos) = self.st.outbox.iter().position(|i| i.id == id) else { return false };
        match r {
            Ok(resp) => {
                self.activity += 1;
                let it = self.st.outbox.remove(pos);
                let me = self.my_account();
                let tid = match &it.env { Envelope::Text { id, .. } | Envelope::File { id, .. } => Some(id.clone()), _ => None };
                if let Some(tid) = tid {
                    if let Some(m) = self.st.chans.get_mut(&it.gid).and_then(|l| l.msgs.iter_mut().find(|m| m.id == tid && m.sender == me)) { m.seq = resp.seq; m.status = "sent".into(); }
                }
                self.st.outbox.iter().any(|i| i.cipher.is_some())
            }
            Err(_) if matches!(self.st.outbox[pos].env, Envelope::Live { .. }) => { self.st.outbox.remove(pos); false } // live data is never retried
            Err(e) => {
                match relay_status(&e) {
                    Some(400) | Some(413) => { self.st.outbox.remove(pos); self.notice("outbox", format!("the relay refused an unsent message, so it was dropped: {e}")); }
                    Some(429) | Some(507) => self.notice("relay", format!("the relay is refusing messages for now: {e}")),
                    _ => log::warn!("post failed, will retry: {e}"),
                }
                false
            }
        }
    }

    /// Everything a running client does periodically, in one go. The app runs the same rounds without holding the client
    /// during `run`. Errors are logged, never fatal: a failed sync does not stop what can still be sent.
    pub fn tick(&mut self, wait_ms: u64) -> Result<()> {
        for _ in 0..8 {
            let r = self.begin_round(wait_ms).run();
            self.finish_round(r)?;
            if !self.more { break; }
        }
        Ok(())
    }
}
