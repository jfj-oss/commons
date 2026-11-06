//! Sending side: communities, channels, invites, membership changes, messages, attachments, reconcile loop.
use crate::client::*;
use crate::envelope::*;
use crate::mls;
use crate::state::*;
use crate::sync::BATCH;
use anyhow::{anyhow, bail, Result};
use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
use commons_proto::*;
use ed25519_dalek::{Signer as _, SigningKey, Verifier as _};
use openmls::prelude::{tls_codec::*, *};
use rand::RngCore;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Plan {
    adds: Vec<(DeviceCert, KeyPackage)>,
    removes: Vec<LeafNodeIndex>,
    gce: Option<GroupExt>,
    desc: String,
    acl_remove: Vec<String>,
    force_update: bool,
}
impl Plan {
    fn empty(desc: &str) -> Plan { Plan { adds: vec![], removes: vec![], gce: None, desc: desc.into(), acl_remove: vec![], force_update: false } }
}
#[derive(Debug, PartialEq)]
pub enum Outcome { Applied, Pending, Lost }

type DirCache = BTreeMap<String, Vec<(DeviceCert, bool)>>;

impl Client {
    fn ctl_ext(&self, cid: &str) -> Result<GroupExt> {
        let g = self.load_group(cid).ok_or_else(|| anyhow!("unknown community"))?;
        mls::group_ext(g.extensions()).ok_or_else(|| anyhow!("community has no metadata"))
    }
    fn my_role(&self, cid: &str) -> Role {
        self.st.communities.get(cid).and_then(|c| c.role_of(&self.my_account())).unwrap_or(Role::Member)
    }
    fn short(g: &str) -> &str { &g[..8.min(g.len())] }

    // ------------------------------------------------------------------ communities and channels
    pub fn create_community(&mut self, name: &str) -> Result<String> {
        if name.trim().is_empty() || name.len() > 64 { bail!("community name must be 1-64 characters"); }
        let ak = self.acct_key()?;
        let owner = self.my_account();
        let nonce = rand_hex(16);
        let cid = community_id(&owner, &nonce);
        let ext = GroupExt::Control { name: name.trim().into(), owner: owner.clone(), nonce: nonce.clone(), admins: vec![], channels: vec![], invites: vec![], banned: vec![] };
        let _ = ak;
        let signer = self.signer()?;
        let cfg = mls::create_cfg(&ext)?;
        let group = MlsGroup::new_with_group_id(&self.prov, &signer, &cfg, GroupId::from_slice(&hex::decode(&cid)?), mls::cred_with_key(&self.cert()?)?)?;
        drop(group);
        let k = self.dev_key()?;
        self.api.acl(&k, &cid, vec![owner.clone()], vec![])?;
        let mut gs = GroupState::new(&cid, &cid, Kind::Control);
        gs.join_epoch = 0;
        self.st.groups.insert(cid.clone(), gs);
        self.st.communities.insert(cid.clone(), Community { id: cid.clone(), ..Default::default() });
        self.post_merge(&cid, 0, &[]);
        self.synced.insert(cid.clone());
        self.st.profile_dirty = true;
        self.save()?;
        self.create_channel(&cid, "general")?;
        Ok(cid)
    }

    pub fn create_channel(&mut self, cid: &str, name: &str) -> Result<String> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can create channels"); }
        let name = name.trim().to_lowercase().replace(' ', "-");
        if name.is_empty() || name.len() > 64 { bail!("channel name must be 1-64 characters"); }
        let gid = rand_hex(16);
        let GroupExt::Control { owner: c_owner, nonce: c_nonce, admins: c_admins, .. } = self.ctl_ext(cid)? else { bail!("bad table") };
        let ext = GroupExt::Channel { community: cid.into(), name: name.clone(), owner: c_owner, nonce: c_nonce, admins: c_admins };
        let signer = self.signer()?;
        let cfg = mls::create_cfg(&ext)?;
        let group = MlsGroup::new_with_group_id(&self.prov, &signer, &cfg, GroupId::from_slice(&hex::decode(&gid)?), mls::cred_with_key(&self.cert()?)?)?;
        let epoch = group.epoch().as_u64();
        let k = self.dev_key()?;
        let mut accounts = vec![self.my_account()];
        if let Some(g) = self.st.groups.get(cid) { accounts.extend(g.accounts.iter().cloned()); }
        accounts.sort(); accounts.dedup();
        self.api.acl(&k, &gid, accounts, vec![])?;
        let mut gs = GroupState::new(&gid, cid, Kind::Channel);
        gs.join_epoch = epoch;
        self.st.groups.insert(gid.clone(), gs);
        self.post_merge(&gid, 0, &[]);
        self.synced.insert(gid.clone());
        self.save()?;
        let GroupExt::Control { name: n, owner, nonce, admins, mut channels, invites, banned } = self.ctl_ext(cid)? else { bail!("bad table") };
        channels.push(ChanMeta { id: gid.clone(), name });
        let mut plan = Plan::empty(&format!("create channel"));
        plan.gce = Some(GroupExt::Control { name: n, owner, nonce, admins, channels, invites, banned });
        match self.run_commit(cid, plan)? {
            Outcome::Lost => bail!("another admin changed the community at the same time; try again"),
            _ => {}
        }
        self.reconcile();
        Ok(gid)
    }

    pub fn create_invite(&mut self, cid: &str, uses: Option<u32>, ttl_secs: u64) -> Result<String> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can create invites"); }
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let id = rand_hex(16);
        let GroupExt::Control { name, owner, nonce, admins, channels, mut invites, banned } = self.ctl_ext(cid)? else { bail!("bad table") };
        let now = now_secs();
        invites.retain(|i| i.expires > now);
        invites.push(InviteMeta { id: id.clone(), pk: hex::encode(sk.verifying_key().to_bytes()), expires: now + ttl_secs, uses_left: uses });
        let cname = name.clone();
        let mut plan = Plan::empty("create invite");
        plan.gce = Some(GroupExt::Control { name, owner: owner.clone(), nonce, admins, channels, invites, banned });
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("another admin changed the community at the same time; try again"); }
        let link = InviteLink { owner, relay: self.st.relay.clone(), community: cid.into(), invite: id.clone(), sk: hex::encode(sk.to_bytes()), name: cname };
        self.st.communities.get_mut(cid).unwrap().invite_sks.insert(id, link.sk.clone());
        self.save()?;
        Ok(link.encode())
    }

    pub fn revoke_invite(&mut self, cid: &str, invite: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can revoke invites"); }
        let GroupExt::Control { name, owner, nonce, admins, channels, mut invites, banned } = self.ctl_ext(cid)? else { bail!("bad table") };
        invites.retain(|i| i.id != invite);
        let mut plan = Plan::empty("revoke invite");
        plan.gce = Some(GroupExt::Control { name, owner, nonce, admins, channels, invites, banned });
        self.run_commit(cid, plan).map(|_| ())
    }

    /// Joining only records the intent and sends a signed request; an admin must be online (now or later) to add us.
    pub fn join_with_link(&mut self, link: &str) -> Result<String> {
        let l = InviteLink::decode(link)?;
        if l.relay.trim_end_matches('/') != self.st.relay.trim_end_matches('/') { bail!("this invite is for a different relay ({})", l.relay); }
        if self.st.communities.get(&l.community).is_some_and(|c| !c.removed) { bail!("you are already in this community"); }
        self.st.joining.insert(l.community.clone(), l.clone());
        self.st.join_sent.remove(&l.community);
        self.save()?;
        let _ = self.send_join_requests();
        Ok(l.community)
    }

    pub(crate) fn send_join_requests(&mut self) -> Result<()> {
        let k = self.dev_key()?;
        let pending: Vec<InviteLink> = self.st.joining.values().filter(|l| !self.st.join_sent.contains(&l.community)).cloned().collect();
        for l in pending {
            self.ensure_registered()?;
            let isk = SigningKey::from_bytes(&hex32(&l.sk)?);
            let cert = self.cert()?;
            let sig = isk.sign(&join_msg(&l.community, &cert.device, &l.invite));
            let jr = JoinRequest { invite: l.invite.clone(), cert, sig: hex::encode(sig.to_bytes()) };
            let mut data = vec![TAG_JOIN_REQUEST]; data.extend(serde_json::to_vec(&jr)?);
            self.api.post_entry(&k, &l.community, &rand_hex(16), &data)?;
            self.st.join_sent.insert(l.community.clone());
            self.notice("join", format!("asked to join \"{}\"; waiting for an admin to let you in", l.name));
        }
        Ok(())
    }

    pub fn kick(&mut self, cid: &str, account: &str) -> Result<()> {
        let c = self.st.communities.get(cid).ok_or_else(|| anyhow!("unknown community"))?.clone();
        let mine = self.my_role(cid);
        let theirs = c.role_of(account).unwrap_or(Role::Member);
        if account == self.my_account() { bail!("use leave instead"); }
        if !self.st.groups.get(cid).is_some_and(|g| g.accounts.contains(account)) { bail!("that person is not a member"); }
        if theirs == Role::Owner { bail!("the owner cannot be removed"); }
        if mine < Role::Admin || (mine == Role::Admin && theirs != Role::Member) { bail!("you are not allowed to remove a {}", theirs.name()); }
        self.st.communities.get_mut(cid).unwrap().kick_queue.insert(account.to_string());
        self.save()?;
        self.reconcile();
        Ok(())
    }

    pub fn set_role(&mut self, cid: &str, account: &str, role: Role) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can change roles"); }
        if role == Role::Owner || account == self.my_account() { bail!("ownership cannot be transferred in this version"); }
        let ak = self.acct_key()?;
        let GroupExt::Control { name, owner, nonce, mut admins, channels, invites, banned } = self.ctl_ext(cid)? else { bail!("bad table") };
        admins.retain(|a| a.account != account);
        if role == Role::Admin {
            if !self.st.groups.get(cid).is_some_and(|g| g.accounts.contains(account)) { bail!("that person is not in the community"); }
            admins.push(sign_grant(&ak, cid, account));
        }
        let mut plan = Plan::empty(&format!("set role {}", role.name()));
        plan.gce = Some(GroupExt::Control { name, owner, nonce, admins, channels, invites, banned });
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        Ok(())
    }

    pub fn leave(&mut self, cid: &str) -> Result<()> {
        if self.my_role(cid) == Role::Owner { bail!("the owner cannot leave; ownership transfer is not in this version"); }
        self.queue_env(cid, Envelope::Leave);
        for _ in 0..3 { let _ = self.tick(0); }
        Ok(())
    }

    pub fn rename_community(&mut self, cid: &str, new_name: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can rename"); }
        let GroupExt::Control { owner, nonce, admins, channels, invites, banned, .. } = self.ctl_ext(cid)? else { bail!("bad table") };
        let mut plan = Plan::empty("rename");
        plan.gce = Some(GroupExt::Control { name: new_name.trim().into(), owner, nonce, admins, channels, invites, banned });
        self.run_commit(cid, plan).map(|_| ())
    }

    // ------------------------------------------------------------------ messages
    pub(crate) fn queue_env(&mut self, gid: &str, env: Envelope) {
        let id = match &env { Envelope::Text { id, .. } | Envelope::File { id, .. } => id.clone(), _ => rand_hex(16) };
        if let Ok(cert) = self.cert() {
            match &env {
                Envelope::Profile { name, ts } => { let p = self.st.profiles.entry(cert.account.clone()).or_default(); if *ts >= p.ts { p.name = name.clone(); p.ts = *ts; } }
                Envelope::Leave | Envelope::Revoked { .. } => {}
                _ => self.apply_chat(gid, 0, &cert, env.clone(), true),
            }
        }
        self.st.outbox.push(OutItem { id: if matches!(env, Envelope::Text { .. } | Envelope::File { .. }) { rand_hex(16) } else { id }, gid: gid.into(), env, cipher: None, tries: 0 });
        self.activity += 1;
    }
    fn check_chan(&self, gid: &str) -> Result<()> {
        match self.st.groups.get(gid) { Some(g) if g.kind == Kind::Channel && !g.removed => Ok(()), _ => bail!("not a channel you are in") }
    }
    pub fn send_text(&mut self, gid: &str, text: &str, reply: Option<String>) -> Result<String> {
        self.check_chan(gid)?;
        if text.trim().is_empty() || text.len() > MAX_TEXT { bail!("message must be 1-{MAX_TEXT} bytes"); }
        let id = rand_hex(16);
        self.queue_env(gid, Envelope::Text { id: id.clone(), text: text.into(), reply, ts: now_secs() });
        self.save()?;
        Ok(id)
    }
    pub fn edit_message(&mut self, gid: &str, id: &str, text: &str) -> Result<()> {
        self.check_chan(gid)?;
        let mine = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == id)).is_some_and(|m| m.sender == self.my_account() && !m.deleted);
        if !mine { bail!("you can only edit your own messages"); }
        self.queue_env(gid, Envelope::Edit { target: id.into(), text: text.into(), ts: now_secs() }); self.save()
    }
    pub fn delete_message(&mut self, gid: &str, id: &str) -> Result<()> {
        self.check_chan(gid)?;
        let cid = self.st.groups[gid].community.clone();
        let m_sender = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == id)).map(|m| m.sender.clone());
        match m_sender { Some(s) if s == self.my_account() || self.my_role(&cid) >= Role::Admin => {}, _ => bail!("you cannot delete that message") }
        self.queue_env(gid, Envelope::Delete { target: id.into() }); self.save()
    }
    pub fn react(&mut self, gid: &str, id: &str, emoji: &str, on: bool) -> Result<()> {
        self.check_chan(gid)?;
        self.queue_env(gid, Envelope::React { target: id.into(), emoji: emoji.into(), on }); self.save()
    }
    pub fn send_file(&mut self, gid: &str, name: &str, mime: &str, data: &[u8], caption: &str) -> Result<String> {
        self.check_chan(gid)?;
        if data.len() > MAX_BLOB_BYTES - 64 { bail!("file is larger than the {} MB limit", MAX_BLOB_BYTES / 1024 / 1024); }
        let mut key = [0u8; 32]; let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut key); rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = ChaCha20Poly1305::new((&key).into()).encrypt(Nonce::from_slice(&nonce), data).map_err(|_| anyhow!("encrypt"))?;
        let blob = rand_hex(16);
        let k = self.dev_key()?;
        self.api.put_blob(&k, &blob, &ct)?;
        let id = rand_hex(16);
        let safe: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
        self.queue_env(gid, Envelope::File { id: id.clone(), name: if safe.is_empty() { "file".into() } else { safe }, mime: mime.chars().take(100).collect(), size: data.len() as u64, blob, key: hex::encode(key), nonce: hex::encode(nonce), caption: caption.into(), reply: None, ts: now_secs() });
        self.save()?;
        Ok(id)
    }
    pub fn fetch_file(&mut self, gid: &str, msg_id: &str) -> Result<(String, String, Vec<u8>)> {
        let m = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == msg_id)).cloned().ok_or_else(|| anyhow!("no such message"))?;
        let MsgBody::File { name, mime, blob, key, nonce, .. } = m.body else { bail!("not a file") };
        let cache = self.dir.join("blobs").join(&blob);
        if let Ok(sealed) = std::fs::read(&cache) { if let Ok(p) = self.vault.unseal(&sealed) { return Ok((name, mime, p)); } }
        let k = self.dev_key()?;
        let ct = self.api.get_blob(&k, &blob)?;
        let pt = ChaCha20Poly1305::new((&hex32(&key)?).into()).decrypt(Nonce::from_slice(&hex::decode(&nonce)?), ct.as_slice())
            .map_err(|_| anyhow!("the attachment is corrupt or was tampered with on the relay (authentication failed)"))?;
        let _ = std::fs::create_dir_all(self.dir.join("blobs"));
        let _ = std::fs::write(&cache, self.vault.seal(&pt)?);
        Ok((name, mime, pt))
    }
    /// Best-effort typing hint. Off by default (see THREAT-MODEL.md).
    pub fn send_typing(&mut self, gid: &str) {
        if !self.st.typing_enabled || self.check_chan(gid).is_err() { return; }
        let key = format!("{gid}|me");
        let now = now_secs();
        if self.typing.get(&key).is_some_and(|t| now < t + 4) { return; }
        self.typing.insert(key, now);
        let _ = self.send_ephemeral(gid, Envelope::Typing);
    }
    fn send_ephemeral(&mut self, gid: &str, env: Envelope) -> Result<()> {
        if !self.ready_to_encrypt(gid) { return Ok(()); }
        let (data, id) = self.encrypt_env(gid, &env)?;
        self.save()?;
        let k = self.dev_key()?;
        self.api.post_entry(&k, gid, &id, &data).map(|_| ())
    }
    fn ready_to_encrypt(&self, gid: &str) -> bool {
        self.st.groups.get(gid).is_some_and(|g| !g.removed && !g.broken && !g.tainted && g.pending.is_none()) && self.synced.contains(gid)
    }
    fn encrypt_env(&mut self, gid: &str, env: &Envelope) -> Result<(Vec<u8>, String)> {
        let mut group = self.load_group(gid).ok_or_else(|| anyhow!("group state missing"))?;
        if !group.is_active() { bail!("not a member any more"); }
        let signer = self.signer()?;
        let out = group.create_message(&self.prov, &signer, &env.to_bytes())?;
        let mut data = vec![TAG_MLS]; data.extend(out.to_bytes()?);
        let id = rand_hex(16);
        self.remember_own(&id);
        Ok((data, id))
    }
    fn remember_own(&mut self, id: &str) {
        self.st.own_ids.push_back(id.to_string());
        while self.st.own_ids.len() > 4000 { self.st.own_ids.pop_front(); }
    }

    fn flush_outbox(&mut self) {
        let items = std::mem::take(&mut self.st.outbox);
        let mut keep = vec![];
        let k = match self.dev_key() { Ok(k) => k, Err(_) => { self.st.outbox = items; return } };
        for mut it in items {
            let Some(gs) = self.st.groups.get(&it.gid).cloned() else { continue }; // group gone
            if gs.removed { self.notice("outbox", "dropped an unsent message: you are no longer in that group".to_string()); continue; }
            if it.cipher.is_none() {
                if gs.tainted && gs.pending.is_none() && self.synced.contains(&it.gid) && !gs.broken {
                    let mut p = Plan::empty("refresh keys after restore"); p.force_update = true;
                    let _ = self.run_commit(&it.gid, p);
                }
                if !self.ready_to_encrypt(&it.gid) { keep.push(it); continue; }
                match self.encrypt_env(&it.gid, &it.env) {
                    Ok((data, id)) => { it.cipher = Some(b64(&data)); it.id = id; }
                    Err(e) => { it.tries += 1; if it.tries > 20 { self.notice("outbox", format!("could not encrypt a message, giving up: {e}")); } else { keep.push(it); } continue; }
                }
                // persist ratchet + ciphertext BEFORE touching the network: never encrypt twice with one key
                self.st.outbox = keep.clone(); self.st.outbox.push(it.clone());
                if self.save().is_err() { keep.push(it); continue; }
            }
            let data = unb64(it.cipher.as_ref().unwrap()).unwrap_or_default();
            match self.api.post_entry(&k, &it.gid, &it.id, &data) {
                Ok(r) => {
                    self.activity += 1;
                    let me = self.my_account();
                    let tid = match &it.env { Envelope::Text { id, .. } | Envelope::File { id, .. } => Some(id.clone()), _ => None };
                    if let Some(tid) = tid {
                        if let Some(m) = self.st.chans.get_mut(&it.gid).and_then(|l| l.msgs.iter_mut().find(|m| m.id == tid && m.sender == me)) { m.seq = r.seq; m.status = "sent".into(); }
                    }
                }
                Err(e) => { log::warn!("post failed, will retry: {e}"); keep.push(it); }
            }
        }
        let mut now = std::mem::take(&mut self.st.outbox);
        // items queued by reentrant calls (e.g. run_commit -> tick) while we iterated
        now.retain(|n| !keep.iter().any(|k| k.id == n.id));
        self.st.outbox = keep; self.st.outbox.extend(now.into_iter().filter(|n| n.cipher.is_none()));
    }

    // ------------------------------------------------------------------ commit machinery
    /// Create, persist, post and (try to) confirm one commit. Never merges before the relay log shows it won.
    pub(crate) fn run_commit(&mut self, gid: &str, plan: Plan) -> Result<Outcome> {
        if self.st.groups.get(gid).is_some_and(|g| g.pending.is_some()) {
            self.try_post_pending(gid);
            let _ = self.sync_once(0);
            if self.st.groups[gid].pending.is_some() { return Ok(Outcome::Pending); }
        }
        self.sync_once(0)?;
        if !self.synced.contains(gid) { bail!("not caught up with the group yet"); }
        let gs = self.st.groups.get(gid).cloned().ok_or_else(|| anyhow!("unknown group"))?;
        if gs.removed || gs.broken { bail!("cannot change a group we are not a working member of"); }
        let mut group = self.load_group(gid).ok_or_else(|| anyhow!("group state missing"))?;
        if group.pending_commit().is_some() { group.clear_pending_commit(self.prov.storage())?; }
        let signer = self.signer()?;
        let kps: Vec<KeyPackage> = plan.adds.iter().map(|(_, k)| k.clone()).collect();
        let mut cb = group.commit_builder();
        if !kps.is_empty() { cb = cb.propose_adds(kps); }
        if !plan.removes.is_empty() { cb = cb.propose_removals(plan.removes.clone()); }
        if let Some(g) = &plan.gce { cb = cb.propose_group_context_extensions(mls::ext_of(g)?)?; }
        if plan.force_update { cb = cb.force_self_update(true); }
        let bundle = cb.load_psks(self.prov.storage())?.build(self.prov.rand(), self.prov.crypto(), &signer, |_| true)?.stage_commit(&self.prov)?;
        let mut data = vec![TAG_COMMIT]; data.extend(bundle.commit().to_bytes()?);
        let welcome = match bundle.to_welcome_msg() { Some(w) => Some(b64(&w.to_bytes()?)), None => None };
        let msg_id = rand_hex(16);
        self.remember_own(&msg_id);
        let recipients: Vec<String> = plan.adds.iter().map(|(c, _)| c.device.clone()).collect();
        let new_accounts: Vec<String> = { let mut v: Vec<String> = plan.adds.iter().map(|(c, _)| c.account.clone()).collect(); v.sort(); v.dedup(); v };
        self.st.groups.get_mut(gid).unwrap().pending = Some(PendingCommit { msg_id: msg_id.clone(), data: b64(&data), welcome, recipients, acl_remove: plan.acl_remove.clone(), posted: false, desc: plan.desc.clone() });
        self.save()?; // the staged commit and advanced key schedule hit disk before the network
        let k = self.dev_key()?;
        if !new_accounts.is_empty() {
            if let Err(e) = self.api.acl(&k, gid, new_accounts, vec![]) {
                let mut g = self.load_group(gid).unwrap(); let _ = g.clear_pending_commit(self.prov.storage());
                self.st.groups.get_mut(gid).unwrap().pending = None; self.save()?;
                return Err(e);
            }
        }
        self.try_post_pending(gid);
        for _ in 0..4 {
            let _ = self.sync_once(0);
            if self.st.groups.get(gid).is_none_or(|g| g.pending.is_none()) { break; }
        }
        Ok(if self.applied.contains(&msg_id) { Outcome::Applied } else if self.st.groups[gid].pending.is_some() { Outcome::Pending } else { Outcome::Lost })
    }

    fn try_post_pending(&mut self, gid: &str) {
        let Some(p) = self.st.groups.get(gid).and_then(|g| g.pending.clone()) else { return };
        let Ok(k) = self.dev_key() else { return };
        match self.api.post_entry(&k, gid, &p.msg_id, &unb64(&p.data).unwrap_or_default()) {
            Ok(_) => { if let Some(g) = self.st.groups.get_mut(gid) { if let Some(pp) = g.pending.as_mut() { pp.posted = true; } } }
            Err(e) => log::warn!("commit post failed, will retry: {e}"),
        }
    }

    fn flush_welcomes(&mut self) {
        let Ok(k) = self.dev_key() else { return };
        let items = std::mem::take(&mut self.st.welcome_out);
        for w in items {
            match self.api.post_welcome(&k, &w.device, &w.id, &unb64(&w.data).unwrap_or_default()) {
                Ok(()) => { self.activity += 1; }
                Err(e) => { log::warn!("welcome post failed, will retry: {e}"); self.st.welcome_out.push(w); }
            }
        }
        let jobs = std::mem::take(&mut self.st.acl_jobs);
        for j in jobs {
            if now_secs() < j.after { self.st.acl_jobs.push(j); continue; }
            if self.api.acl(&k, &j.gid, vec![], j.remove.clone()).is_err() { self.st.acl_jobs.push(j); }
        }
    }

    // ------------------------------------------------------------------ the tick
    /// Everything a running client does periodically. Errors are logged, never fatal.
    pub fn tick(&mut self, wait_ms: u64) -> Result<()> {
        if !self.has_identity() { return self.pairing_poll().map(|_| ()); }
        let now = now_secs();
        if self.last_dir_check.get("kp").is_none_or(|t| now >= t + 60) || !self.st.registered {
            self.last_dir_check.insert("kp".into(), now);
            if let Err(e) = self.ensure_registered() { log::warn!("register/keypackages: {e}"); }
        }
        if let Err(e) = self.send_join_requests() { log::warn!("join request: {e}"); }
        let pend: Vec<String> = self.st.groups.values().filter(|g| g.pending.as_ref().is_some_and(|p| !p.posted)).map(|g| g.gid.clone()).collect();
        for g in pend { self.try_post_pending(&g); }
        self.sync_once(wait_ms)?;
        self.flush_welcomes();
        self.send_rejoin_requests();
        if self.st.profile_dirty {
            self.st.profile_dirty = false;
            let name = self.st.name.clone();
            let ctls: Vec<String> = self.st.groups.values().filter(|g| g.kind == Kind::Control && !g.removed && !g.broken).map(|g| g.gid.clone()).collect();
            for g in ctls { self.queue_env(&g, Envelope::Profile { name: name.clone(), ts: now_secs() }); }
        }
        self.reconcile();
        self.flush_outbox();
        self.flush_welcomes();
        self.save()
    }

    fn send_rejoin_requests(&mut self) {
        let Ok(k) = self.dev_key() else { return };
        let now = now_secs();
        let broken: Vec<String> = self.st.groups.values().filter(|g| g.broken && !g.removed && (!g.rejoin_sent || now > g.rejoin_ts + 300)).map(|g| g.gid.clone()).collect();
        for gid in broken {
            let cert = match self.cert() { Ok(c) => c, Err(_) => return };
            let nonce = rand_hex(8);
            let mut m = b"commons/rejoin/v1".to_vec(); m.extend_from_slice(&sha256(&[gid.as_bytes(), nonce.as_bytes()]));
            let sig = k.sign(&m);
            let body = serde_json::json!({"cert": cert, "nonce": nonce, "sig": hex::encode(sig.to_bytes())});
            let mut data = vec![TAG_REJOIN]; data.extend(serde_json::to_vec(&body).unwrap_or_default());
            if self.api.post_entry(&k, &gid, &rand_hex(16), &data).is_ok() {
                let g = self.st.groups.get_mut(&gid).unwrap(); g.rejoin_sent = true; g.rejoin_ts = now;
            }
        }
    }

    // ------------------------------------------------------------------ reconcile
    /// Bring group membership in line with the roster (control group) and the device directory.
    /// Admins/owner reconcile everyone; plain members only reconcile their own devices.
    pub fn reconcile(&mut self) {
        let cids: Vec<String> = self.st.communities.values().filter(|c| !c.removed).map(|c| c.id.clone()).collect();
        let mut cache = DirCache::new();
        for cid in cids {
            if let Err(e) = self.reconcile_community(&cid, &mut cache) { log::warn!("reconcile {}: {e}", Self::short(&cid)); }
        }
    }

    fn dir_for(&mut self, cache: &mut DirCache, acct: &str) -> Result<Vec<(DeviceCert, bool)>> {
        if let Some(d) = cache.get(acct) { return Ok(d.clone()); }
        let d = self.directory(acct)?;
        cache.insert(acct.to_string(), d.clone());
        Ok(d)
    }

    fn reconcile_community(&mut self, cid: &str, cache: &mut DirCache) -> Result<()> {
        let c = self.st.communities[cid].clone();
        let me = self.my_account();
        let role = self.my_role(cid);
        let Some(ctl) = self.st.groups.get(cid).cloned() else { return Ok(()) };
        if ctl.removed || ctl.broken || !self.synced.contains(cid) { return Ok(()); }
        // admins turn valid join requests into joiners
        let mut joiners: Vec<(DeviceCert, String)> = vec![];
        if role >= Role::Admin {
            let reqs = ctl.requests.clone();
            for r in reqs.iter().filter(|r| r.tag == TAG_JOIN_REQUEST) {
                match self.check_join(&c, r) {
                    Ok(Some(j)) => joiners.push(j),
                    Ok(None) => { self.drop_request(cid, r.seq); }
                    Err(e) => { self.notice("join", format!("ignored a join request: {e}")); self.drop_request(cid, r.seq); }
                }
            }
        }
        let mut remove_accounts: BTreeSet<String> = BTreeSet::new();
        if role >= Role::Admin {
            remove_accounts.extend(c.kick_queue.iter().cloned());
            remove_accounts.extend(c.leaving.iter().cloned());
            remove_accounts.remove(&c.owner);
        }
        let mut order = vec![cid.to_string()];
        order.extend(c.channels.iter().map(|ch| ch.id.clone()));
        for gid in order {
            let is_ctl = gid == cid;
            let Some(gs) = self.st.groups.get(&gid).cloned() else { continue };
            if gs.removed || gs.broken || gs.pending.is_some() || !self.synced.contains(&gid) { continue; }
            if let Err(e) = self.reconcile_group(&c, &gid, is_ctl, role, &me, &remove_accounts, if is_ctl { &joiners } else { &[] }, cache) {
                log::warn!("reconcile group {}: {e}", Self::short(&gid));
            }
        }
        Ok(())
    }

    fn drop_request(&mut self, gid: &str, seq: u64) {
        if let Some(g) = self.st.groups.get_mut(gid) { g.requests.retain(|r| r.seq != seq); }
    }

    /// Validate a join request against the current control table. Ok(None) = stale/duplicate.
    fn check_join(&mut self, c: &Community, r: &ReqItem) -> Result<Option<(DeviceCert, String)>> {
        let jr = JoinRequest::parse(&unb64(&r.data)?)?;
        let inv = c.invites.iter().find(|i| i.id == jr.invite).ok_or_else(|| anyhow!("invite no longer valid"))?;
        if inv.expires < now_secs() { bail!("invite expired"); }
        if inv.uses_left == Some(0) { bail!("invite used up"); }
        let pk = ed25519_dalek::VerifyingKey::from_bytes(&hex32(&inv.pk)?)?;
        let sig = ed25519_dalek::Signature::from_slice(&hex::decode(&jr.sig)?)?;
        pk.verify(&join_msg(&c.id, &jr.cert.device, &jr.invite), &sig).map_err(|_| anyhow!("bad invite signature"))?;
        if c.banned.contains(&jr.cert.account) { bail!("that account is banned from this community"); }
        if self.st.revoked.contains_key(&jr.cert.device) { bail!("device revoked"); }
        if self.st.groups.get(&c.id).is_some_and(|g| g.dev_seq.contains_key(&jr.cert.device)) { return Ok(None); }
        Ok(Some((jr.cert, jr.invite)))
    }

    #[allow(clippy::too_many_arguments)]
    fn reconcile_group(&mut self, c: &Community, gid: &str, is_ctl: bool, role: Role, me: &str, remove_accounts: &BTreeSet<String>, joiners: &[(DeviceCert, String)], cache: &mut DirCache) -> Result<()> {
        let group = self.load_group(gid).ok_or_else(|| anyhow!("no group"))?;
        let members = mls::members_of(&group);
        let ctl_accounts = self.st.groups[&c.id].accounts.clone();
        let mut desired: BTreeSet<String> = ctl_accounts.difference(remove_accounts).cloned().collect();
        let gs = self.st.groups[gid].clone();
        let may = |a: &str| role >= Role::Admin || a == me;
        let mut plan = Plan::empty("");
        let mut descs: Vec<String> = vec![];
        let mut removed_devices: BTreeSet<String> = BTreeSet::new();
        // resync requests from devices whose state fell behind
        let mut handled_reqs: Vec<u64> = vec![];
        let mut rejoin_devices: Vec<DeviceCert> = vec![];
        for r in gs.requests.iter().filter(|r| r.tag == TAG_REJOIN) {
            let v: serde_json::Value = match serde_json::from_slice(&unb64(&r.data).unwrap_or_default()) { Ok(v) => v, Err(_) => { handled_reqs.push(r.seq); continue } };
            let cert: Option<DeviceCert> = serde_json::from_value(v["cert"].clone()).ok();
            let (Some(cert), Some(nonce), Some(sig)) = (cert, v["nonce"].as_str(), v["sig"].as_str()) else { handled_reqs.push(r.seq); continue };
            let mut m = b"commons/rejoin/v1".to_vec(); m.extend_from_slice(&sha256(&[gid.as_bytes(), nonce.as_bytes()]));
            let ok = cert.verify().is_ok() && hex32(&cert.device).ok().and_then(|d| ed25519_dalek::VerifyingKey::from_bytes(&d).ok())
                .zip(hex::decode(sig).ok().and_then(|s| ed25519_dalek::Signature::from_slice(&s).ok())).is_some_and(|(vk, s)| vk.verify(&m, &s).is_ok());
            if !ok || !may(&cert.account) || !desired.contains(&cert.account) { handled_reqs.push(r.seq); continue; }
            match gs.dev_seq.get(&cert.device) { Some(&s) if s >= r.seq => { handled_reqs.push(r.seq); continue } _ => {} }
            rejoin_devices.push(cert); handled_reqs.push(r.seq);
        }
        if is_ctl { for (cert, _) in joiners { desired.insert(cert.account.clone()); } }
        // removals
        for (idx, cert) in &members {
            let acct_removed = !desired.contains(&cert.account) && role >= Role::Admin && cert.account != c.owner
                && (is_ctl || ctl_accounts.contains(&cert.account) || true)
                && (role == Role::Owner || c.role_of(&cert.account) == Some(Role::Member));
            let revoked = self.st.revoked.contains_key(&cert.device) && may(&cert.account);
            let rejoin = rejoin_devices.iter().any(|d| d.device == cert.device);
            if acct_removed || revoked || rejoin {
                plan.removes.push(*idx); removed_devices.insert(cert.device.clone());
                if acct_removed && !members.iter().any(|(i, m)| m.account == cert.account && !plan.removes.contains(i)) { plan.acl_remove.push(cert.account.clone()); }
            }
        }
        if !plan.removes.is_empty() { descs.push(format!("remove {} device(s)", plan.removes.len())); }
        // additions
        let have: BTreeSet<String> = members.iter().filter(|(i, _)| !plan.removes.contains(i)).map(|(_, c)| c.device.clone()).collect();
        let mut want: Vec<DeviceCert> = vec![];
        for acct in desired.iter().filter(|a| may(a)) {
            if is_ctl && !ctl_accounts.contains(acct) {
                for (j, _) in joiners.iter().filter(|(j, _)| &j.account == acct) { want.push(j.clone()); }
                continue;
            }
            if c.banned.contains(acct) { continue; }
            for (cert, revoked) in self.dir_for(cache, acct)? { if !revoked && !have.contains(&cert.device) { want.push(cert); } }
        }
        for j in &rejoin_devices { if !want.iter().any(|w| w.device == j.device) { want.push(j.clone()); } }
        if is_ctl {
            for (j, _) in joiners { if ctl_accounts.contains(&j.account) && !have.contains(&j.device) && !want.iter().any(|w| w.device == j.device) && role >= Role::Admin { want.push(j.clone()); } }
        }
        let mut seen = BTreeSet::new();
        want.retain(|w| seen.insert(w.device.clone()) && !self.st.revoked.contains_key(&w.device));
        for w in want {
            match self.api.take_kp(&self.dev_key()?, &w.device) {
                Ok(Some(b)) => match mls::parse_kp(&self.prov, &b) {
                    Ok((kp, cert)) if cert.device == w.device && cert.account == w.account => plan.adds.push((cert, kp)),
                    Ok(_) => self.notice("keypackage", format!("relay returned a key package for the wrong device (device {}); ignored", &w.device[..8])),
                    Err(e) => self.notice("keypackage", format!("invalid key package for device {}: {e}", &w.device[..8])),
                },
                Ok(None) => log::info!("device {} has no key package yet", &w.device[..8]),
                Err(e) => log::warn!("key package fetch: {e}"),
            }
        }
        if !plan.adds.is_empty() { descs.push(format!("add {} device(s)", plan.adds.len())); }
        // control table: consume invites, record bans
        if is_ctl && role >= Role::Admin {
            let GroupExt::Control { name, owner, nonce, admins, channels, mut invites, mut banned } = self.ctl_ext(&c.id)? else { bail!("bad table") };
            let mut changed = false;
            for (j, inv) in joiners {
                if plan.adds.iter().any(|(c2, _)| c2.device == j.device) {
                    if let Some(i) = invites.iter_mut().find(|i| &i.id == inv) { if let Some(n) = i.uses_left.as_mut() { *n = n.saturating_sub(1); changed = true; } }
                }
            }
            let before = invites.len();
            invites.retain(|i| i.uses_left != Some(0));
            if invites.len() != before { changed = true; }
            for a in &c.kick_queue { if !banned.contains(a) && members.iter().any(|(_, m)| &m.account == a) { banned.push(a.clone()); changed = true; } }
            if changed { plan.gce = Some(GroupExt::Control { name, owner, nonce, admins, channels, invites, banned }); descs.push("update table".into()); }
        }
        // keep each channel's own copy of the owner-signed admin list current (only the owner may change it)
        if !is_ctl && role == Role::Owner {
            if let (Some(GroupExt::Channel { community, name, owner, nonce, admins }), Ok(GroupExt::Control { admins: ctl_admins, .. })) = (mls::group_ext(group.extensions()), self.ctl_ext(&c.id)) {
                if admins != ctl_admins { plan.gce = Some(GroupExt::Channel { community, name, owner, nonce, admins: ctl_admins }); descs.push("update channel admins".into()); }
            }
        }
        if plan.removes.is_empty() && plan.adds.is_empty() && plan.gce.is_none() {
            for s in handled_reqs { self.drop_request(gid, s); }
            return Ok(());
        }
        plan.desc = descs.join(", ");
        let out = self.run_commit(gid, plan)?;
        if out != Outcome::Lost { for s in handled_reqs { self.drop_request(gid, s); } }
        // joiners are handled (or will be re-validated from the table) once the commit is applied
        if out == Outcome::Applied && is_ctl { for r in self.st.groups[gid].requests.clone().iter().filter(|r| r.tag == TAG_JOIN_REQUEST) { if let Ok(jr) = JoinRequest::parse(&unb64(&r.data).unwrap_or_default()) { if joiners.iter().any(|(j, _)| j.device == jr.cert.device) { self.drop_request(gid, r.seq); } } } }
        Ok(())
    }

    // ------------------------------------------------------------------ queries for the UI
    pub fn display_name(&self, acct: &str) -> String {
        if acct == self.my_account() && !self.st.name.is_empty() { return self.st.name.clone(); }
        self.st.profiles.get(acct).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("user-{}", &acct[..6.min(acct.len())]))
    }
    pub fn channels_of(&self, cid: &str) -> Vec<ChannelInfo> {
        let Some(c) = self.st.communities.get(cid) else { return vec![] };
        c.channels.iter().map(|ch| {
            let joined = self.st.groups.get(&ch.id).is_some_and(|g| !g.removed);
            ChannelInfo { gid: ch.id.clone(), name: ch.name.clone(), joined, unread: self.unread(&ch.id) }
        }).collect()
    }
    pub fn unread(&self, gid: &str) -> usize {
        let me = self.my_account();
        self.st.chans.get(gid).map(|l| l.msgs.iter().skip(l.read_upto).filter(|m| m.sender != me && !m.deleted && !matches!(m.body, MsgBody::Notice(_))).count()).unwrap_or(0)
    }
    pub fn mark_read(&mut self, gid: &str) { if let Some(l) = self.st.chans.get_mut(gid) { l.read_upto = l.msgs.len(); } }
    pub fn messages(&self, gid: &str) -> Vec<Msg> {
        let mut v = self.st.chans.get(gid).map(|l| l.msgs.clone()).unwrap_or_default();
        // relay order for delivered messages; local unsent ones stay at the end in the order sent
        let (mut sent, pending): (Vec<Msg>, Vec<Msg>) = v.drain(..).partition(|m| m.seq > 0 || m.status == "system");
        sent.sort_by_key(|m| (if m.seq > 0 { m.seq } else { u64::MAX }, m.ts));
        sent.extend(pending);
        sent
    }
    pub fn members(&self, cid: &str) -> Vec<MemberInfo> {
        let (Some(c), Some(g)) = (self.st.communities.get(cid), self.st.groups.get(cid)) else { return vec![] };
        let Some(group) = self.load_group(cid) else { return vec![] };
        let all = mls::members_of(&group);
        let me = self.my_account();
        g.accounts.iter().map(|a| MemberInfo {
            account: a.clone(), name: self.display_name(a), role: c.role_of(a).unwrap_or(Role::Member),
            devices: all.iter().filter(|(_, m)| &m.account == a).count(), verified: self.st.verified.contains(a), is_me: *a == me,
        }).collect()
    }
    pub fn search(&self, q: &str) -> Vec<(String, String, Msg)> {
        let q = q.to_lowercase();
        if q.trim().is_empty() { return vec![]; }
        let mut out = vec![];
        for (gid, l) in &self.st.chans {
            let Some(g) = self.st.groups.get(gid) else { continue };
            for m in &l.msgs {
                if m.deleted { continue; }
                let hay = match &m.body { MsgBody::Text(t) => t.to_lowercase(), MsgBody::File { name, caption, .. } => format!("{name} {caption}").to_lowercase(), _ => continue };
                if hay.contains(&q) { out.push((g.community.clone(), gid.clone(), m.clone())); }
            }
        }
        out.sort_by_key(|(_, _, m)| std::cmp::Reverse(m.ts));
        out.truncate(100);
        out
    }
    pub fn set_verified(&mut self, acct: &str, v: bool) -> Result<()> { if v { self.st.verified.insert(acct.into()); } else { self.st.verified.remove(acct); } self.save() }
    pub fn typing_now(&self, gid: &str) -> Vec<String> {
        let now = now_secs();
        self.typing.iter().filter(|(k, t)| k.starts_with(&format!("{gid}|")) && !k.ends_with("|me") && now < **t + 6).map(|(k, _)| self.display_name(k.split('|').nth(1).unwrap_or(""))).collect()
    }
    pub fn qr_svg(data: &str) -> String {
        qrcode::QrCode::new(data.as_bytes()).map(|q| q.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build()).unwrap_or_default()
    }
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        if name.trim().is_empty() || name.len() > 64 { bail!("name must be 1-64 characters"); }
        self.st.name = name.trim().into(); self.st.profile_dirty = true; self.save()
    }
    pub fn group_status(&self, gid: &str) -> String {
        match self.st.groups.get(gid) {
            None => "not joined yet".into(),
            Some(g) if g.removed => "removed".into(),
            Some(g) if g.broken => "out of sync, waiting to be re-added".into(),
            Some(g) if g.tainted => "refreshing keys".into(),
            Some(g) if g.pending.is_some() => "applying a change".into(),
            Some(_) => "ok".into(),
        }
    }
    // ---- hooks used by the test harness and diagnostics (they do not weaken any check on the receiving side)
    #[doc(hidden)]
    pub fn epoch_fingerprint(&self, gid: &str) -> String {
        use openmls_traits::OpenMlsProvider;
        match self.load_group(gid) {
            Some(g) => format!("{}:{}", g.epoch().as_u64(), hex::encode(g.export_secret(self.prov.crypto(), "commons-fingerprint", &[], 16).unwrap_or_default())),
            None => "none".into(),
        }
    }
    #[doc(hidden)]
    pub fn group_accounts(&self, gid: &str) -> BTreeSet<String> {
        self.load_group(gid).map(|g| mls::members_of(&g).into_iter().map(|(_, c)| c.account).collect()).unwrap_or_default()
    }
    #[doc(hidden)]
    pub fn group_devices(&self, gid: &str) -> BTreeSet<String> {
        self.load_group(gid).map(|g| mls::members_of(&g).into_iter().map(|(_, c)| c.device).collect()).unwrap_or_default()
    }
    /// Try to open a raw log entry with this device's current state (used to prove a removed member cannot read).
    #[doc(hidden)]
    pub fn debug_try_decrypt(&mut self, gid: &str, entry: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let mut group = self.load_group(gid).ok_or("no group state")?;
        if entry.is_empty() || (entry[0] != TAG_MLS && entry[0] != TAG_COMMIT) { return Err("not an mls entry".into()); }
        let m = MlsMessageIn::tls_deserialize_exact(&entry[1..]).map_err(|e| e.to_string())?;
        let pm = m.try_into_protocol_message().map_err(|e| e.to_string())?;
        match group.process_message(&self.prov, pm).map_err(|e| format!("{e:?}"))?.into_content() {
            ProcessedMessageContent::ApplicationMessage(a) => Ok(a.into_bytes()),
            _ => Err("not an application message".into()),
        }
    }
    /// A hostile member trying to remove somebody without having the right (receivers must reject).
    #[doc(hidden)]
    pub fn debug_commit_remove(&mut self, gid: &str, account: &str) -> Result<()> {
        let g = self.load_group(gid).ok_or_else(|| anyhow!("no group"))?;
        let mut plan = Plan::empty("hostile removal");
        for (i, c) in mls::members_of(&g) { if c.account == account { plan.removes.push(i); } }
        self.run_commit(gid, plan).map(|_| ())
    }
    /// Post arbitrary bytes into a group log as this device (a misbehaving member / relay-injected garbage).
    #[doc(hidden)]
    pub fn debug_post_raw(&mut self, gid: &str, data: &[u8]) -> Result<()> {
        let k = self.dev_key()?; self.api.post_entry(&k, gid, &rand_hex(16), data).map(|_| ())
    }
    #[doc(hidden)]
    pub fn debug_expire_acl_jobs(&mut self) { for j in self.st.acl_jobs.iter_mut() { j.after = 0; } }
    #[doc(hidden)]
    pub fn outbox_len(&self) -> usize { self.st.outbox.len() }

    #[allow(dead_code)]
    fn _batch() -> usize { BATCH }
}
