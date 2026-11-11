//! Sending side: communities, channels, invites, membership changes, messages, attachments, reconcile loop.
use crate::client::*;
use crate::envelope::*;
use crate::mls;
use crate::state::*;
use crate::sync::BATCH;
use anyhow::{anyhow, bail, Result};
use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
use crate::transport::{relay_says, relay_status, Api};
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
    acl_demote: Vec<String>,
    force_update: bool,
    /// Queued requests this commit answers.
    handled: Vec<u64>,
}
impl Plan {
    fn empty(desc: &str) -> Plan { Plan { adds: vec![], removes: vec![], gce: None, desc: desc.into(), acl_remove: vec![], acl_demote: vec![], force_update: false, handled: vec![] } }
}
/// How long before a member whose consent is missing from the table offers it again.
const CONSENT_RETRY_SECS: u64 = 600;
/// How often a device repeats its offer of the relay's sign-up secret to the account's other devices, or asks for it.
/// A device answers a request as soon as it sees one, so this is only the fallback.
const RELAY_SECRET_RETRY_SECS: u64 = 6 * 3600;

#[derive(Debug, PartialEq)]
pub enum Outcome { Applied, Pending, Lost }

/// An invite link, and whether it can sign up a friend who has no account on this relay yet.
pub struct PwInviteMade { pub invite: String, pub expires: u64, pub signup_needs_secret: bool, pub link: String }
pub struct InviteMade { pub link: String, pub signup_needs_secret: bool }

impl Client {
    fn ctl_ext(&self, cid: &str) -> Result<GroupExt> {
        let g = self.load_group(cid).ok_or_else(|| anyhow!("unknown community"))?;
        mls::group_ext(g.extensions()).ok_or_else(|| anyhow!("community has no metadata"))
    }
    fn my_role(&self, cid: &str) -> Role {
        self.st.communities.get(cid).and_then(|c| c.role_of(&self.my_account())).unwrap_or(Role::Member)
    }
    fn short(g: &str) -> &str { &g[..8.min(g.len())] }

    /// This account's owner-signed grant, which is what proves its authority to the relay.
    pub(crate) fn my_grant(&self, cid: &str) -> Option<String> {
        let me = self.my_account();
        let Ok(GroupExt::Control { admins, .. }) = self.ctl_ext(cid) else { return None };
        admins.into_iter().find(|a| a.account == me).map(|a| a.sig)
    }
    fn delete_group(&mut self, gid: &str) {
        if let Some(mut g) = self.load_group(gid) { let _ = g.delete(self.prov.storage()); }
        self.st.groups.remove(gid);
    }

    // ------------------------------------------------------------------ communities and channels
    pub fn create_community(&mut self, name: &str) -> Result<String> {
        let name = name.trim();
        check_name("a community name", name).map_err(|e| anyhow!(e))?;
        self.create_community_with(name, rand_hex(16), "general", "")
    }

    /// The same, with the nonce (a one-to-one community's carries its marker, see `direct.rs`) and the first room chosen.
    pub(crate) fn create_community_with(&mut self, name: &str, nonce: String, room: &str, room_kind: &str) -> Result<String> {
        let owner = self.my_account();
        let cid = community_id(&owner, &nonce);
        let consents = vec![sign_consent(&self.acct_key()?, &cid, now_secs())];
        let ext = GroupExt::Control { name: name.into(), owner: owner.clone(), nonce: nonce.clone(), admins: vec![], channels: vec![], invites: vec![], banned: vec![], transfers: vec![], consents, style: None };
        let signer = self.signer()?;
        let cfg = mls::create_cfg(&ext)?;
        let mut group = MlsGroup::new_with_group_id(&self.prov, &signer, &cfg, GroupId::from_slice(&hex::decode(&cid)?), mls::cred_with_key(&self.cert()?)?)?;
        let k = self.dev_key()?;
        if let Err(e) = self.api.acl(&k, &cid, &AclReq { add: vec![owner.clone()], ..Default::default() }) {
            let _ = group.delete(self.prov.storage());
            self.note_failure(&e);
            return Err(e);
        }
        drop(group);
        let mut gs = GroupState::new(&cid, &cid, Kind::Control);
        gs.join_epoch = 0;
        self.st.groups.insert(cid.clone(), gs);
        self.st.communities.insert(cid.clone(), Community { id: cid.clone(), ..Default::default() });
        self.post_merge(&cid, 0, &[]);
        self.synced.insert(cid.clone());
        self.st.profile_dirty = true;
        self.save()?;
        if let Err(e) = self.create_channel_kind(&cid, room, room_kind) {
            // Without its first room a new community is only a thing to explain. Nobody else knows it yet.
            self.delete_group(&cid);
            self.st.communities.remove(&cid);
            self.synced.remove(&cid);
            self.save()?;
            return Err(e);
        }
        Ok(cid)
    }

    pub fn create_channel(&mut self, cid: &str, name: &str) -> Result<String> {
        self.create_channel_kind(cid, name, "")
    }

    /// `kind` is empty for a text room or [`ROOM_VOICE`] for a voice room. Only those two are accepted here:
    /// anything else would be a new kind this version does not know how to run.
    pub fn create_channel_kind(&mut self, cid: &str, name: &str, kind: &str) -> Result<String> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can create channels"); }
        let kind = match kind { "" | "text" => String::new(), ROOM_VOICE => ROOM_VOICE.to_string(), _ => bail!("unknown room kind {kind}") };
        let name = name.trim().to_lowercase().replace(' ', "-");
        check_name("a room name", &name).map_err(|e| anyhow!(e))?;
        if self.st.communities[cid].channels.iter().any(|c| name_key(&c.name) == name_key(&name)) { bail!("there is already a room called {name}"); }
        let gid = rand_hex(16);
        let GroupExt::Control { owner: c_owner, nonce: c_nonce, admins: c_admins, transfers: c_transfers, .. } = self.ctl_ext(cid)? else { bail!("bad table") };
        let ext = GroupExt::Channel { community: cid.into(), name: name.clone(), owner: c_owner, nonce: c_nonce, admins: c_admins, transfers: c_transfers };
        let signer = self.signer()?;
        let cfg = mls::create_cfg(&ext)?;
        let mut group = MlsGroup::new_with_group_id(&self.prov, &signer, &cfg, GroupId::from_slice(&hex::decode(&gid)?), mls::cred_with_key(&self.cert()?)?)?;
        let epoch = group.epoch().as_u64();
        let k = self.dev_key()?;
        let mut accounts = vec![self.my_account()];
        if let Some(g) = self.st.groups.get(cid) { accounts.extend(g.accounts.iter().cloned()); }
        accounts.sort(); accounts.dedup();
        let grant = self.my_grant(cid);
        let mut claim = AclReq { root: Some(cid.to_string()), grant: grant.clone(), ..Default::default() };
        for part in accounts.chunks(MAX_ACL_BATCH) {
            claim.add = part.to_vec();
            if let Err(e) = self.api.acl(&k, &gid, &claim) {
                let _ = group.delete(self.prov.storage());
                self.note_failure(&e);
                return Err(e);
            }
        }
        drop(group);
        let mut gs = GroupState::new(&gid, cid, Kind::Channel);
        gs.join_epoch = epoch;
        self.st.groups.insert(gid.clone(), gs);
        self.post_merge(&gid, 0, &[]);
        self.synced.insert(gid.clone());
        self.save()?;
        // Until the community's table lists it, a room is nothing. A commit lost to another admin's is redone on the
        // new table; if it cannot be done at all, the room goes again instead of lingering unlisted.
        let mut listed = Err(anyhow!("another admin changed the community at the same time; try again"));
        for _ in 0..3 {
            let mut t = self.ctl_ext(cid)?;
            if let GroupExt::Control { channels, .. } = &mut t { channels.push(ChanMeta { id: gid.clone(), name: name.clone(), kind: kind.clone() }); }
            let mut plan = Plan::empty("create channel");
            plan.gce = Some(t);
            match self.run_commit(cid, plan) {
                Ok(Outcome::Lost) => continue,
                Ok(_) => { listed = Ok(()); break }
                Err(e) => { listed = Err(e); break }
            }
        }
        if let Err(e) = listed {
            self.delete_group(&gid);
            self.synced.remove(&gid);
            self.save()?;
            return Err(e);
        }
        self.reconcile();
        Ok(gid)
    }

    pub fn create_invite(&mut self, cid: &str, uses: Option<u32>, ttl_secs: u64) -> Result<String> {
        self.create_invite_checked(cid, uses, ttl_secs).map(|m| m.link)
    }

    /// As `create_invite`, but says whether the link can sign up a friend who has no account. A device that does not
    /// hold the relay's sign-up secret asks the relay whether it is gated before building the link, so the UI can tell
    /// the person plainly instead of their friend meeting a refusal.
    pub fn create_invite_checked(&mut self, cid: &str, uses: Option<u32>, ttl_secs: u64) -> Result<InviteMade> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can create invites"); }
        if uses == Some(0) { bail!("an invite needs at least one use"); }
        // Asked first, as the person is about to share a link: with no secret here, only already-registered people
        // can use it. A relay that cannot be asked is not assumed to be gated.
        let signup_needs_secret = self.st.relay_invite.is_empty() && self.relay_gated().unwrap_or(false);
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let id = rand_hex(16);
        let mut t = self.ctl_ext(cid)?;
        let GroupExt::Control { name, owner, invites, .. } = &mut t else { bail!("bad table") };
        let (cname, founder) = (name.clone(), owner.clone());
        let now = now_secs();
        invites.retain(|i| i.expires > now);
        invites.push(InviteMeta { id: id.clone(), pk: hex::encode(sk.verifying_key().to_bytes()), expires: now + ttl_secs.min(365 * 86400), uses_left: uses });
        let mut plan = Plan::empty("create invite");
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("another admin changed the community at the same time; try again"); }
        let link = InviteLink { owner: founder, relay: self.st.relay.clone(), community: cid.into(), invite: id.clone(), sk: hex::encode(sk.to_bytes()), name: cname, secret: self.st.relay_invite.clone() };
        self.st.communities.get_mut(cid).unwrap().invite_sks.insert(id, link.sk.clone());
        self.save()?;
        Ok(InviteMade { link: link.encode(), signup_needs_secret })
    }

    /// An invite that a password opens. Makes the ordinary invite, then files a copy of its link, sealed by the password's
    /// keys, on the relay. If the relay refuses the copy (a password already in use, an old relay), the invite is taken back.
    pub fn create_invite_password(&mut self, cid: &str, d: &crate::pwinvite::Derived, uses: Option<u32>, ttl_secs: u64) -> Result<PwInviteMade> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can create invites"); }
        let ttl = ttl_secs.clamp(3600, 30 * 86400);
        let info = self.api.info(None).map_err(|e| anyhow!("could not reach the server: {e}"))?;
        if !info.pwinvite { bail!("This server is too old for password invites. Make an invite link instead."); }
        let made = self.create_invite_checked(cid, uses, ttl)?;
        let link = InviteLink::decode(&made.link)?;
        let filed = crate::pwinvite::put_body(d, &made.link, ttl, uses).and_then(|b| self.api.pw_put(&self.dev_key()?, &d.lid, &b));
        if let Err(e) = filed {
            let _ = self.revoke_invite(cid, &link.invite);
            if relay_says(&e, 409, "") { bail!("Someone already uses that password for an invite. Choose another."); }
            if relay_status(&e) == Some(429) { bail!("The server is holding as many password invites as it allows. Revoke one, or try again later."); }
            return Err(e);
        }
        let expires = now_secs() + ttl;
        self.st.pw_invites.insert(link.invite.clone(), PwInviteRec { lid: d.lid.clone(), expires });
        self.save()?;
        Ok(PwInviteMade { invite: link.invite, expires, signup_needs_secret: made.signup_needs_secret, link: made.link })
    }

    // ------------------------------------------------------------------ the relay's sign-up secret, across one account's devices
    /// Pass this device's relay sign-up secret on to the same account's other devices, sealed under a key only that
    /// account's devices hold. The carrier is an ordinary consent, so an older version parses and verifies it without
    /// noticing the extra field and without showing an "unknown kind" notice.
    pub(crate) fn queue_relay_secret(&mut self, cid: &str) -> Result<()> {
        let secret = self.st.relay_invite.clone();
        if secret.is_empty() { bail!("this device has no sign-up secret to pass on"); }
        let sealed = self.seal_relay_secret(&secret)?;
        let con = sign_consent(&self.acct_key()?, cid, now_secs());
        self.st.relay_secret_shared.insert(cid.into(), now_secs());
        self.queue_env(cid, Envelope::Consent { ts: con.ts, sig: con.sig, relay_secret: Some(sealed), relay_secret_want: false });
        Ok(())
    }

    /// Ask the same account's other devices for the relay's sign-up secret.
    pub(crate) fn queue_relay_secret_request(&mut self, cid: &str) -> Result<()> {
        let con = sign_consent(&self.acct_key()?, cid, now_secs());
        self.st.relay_secret_asked.insert(cid.into(), now_secs());
        self.queue_env(cid, Envelope::Consent { ts: con.ts, sig: con.sig, relay_secret: None, relay_secret_want: true });
        Ok(())
    }

    /// Planning only, no network: offer the sign-up secret on every community this device can encrypt in, or ask for
    /// it when this device has none. Runs after `reconcile` and before the outbox is encrypted.
    pub(crate) fn plan_relay_secret(&mut self) {
        let now = now_secs();
        // Nothing to pass on to, or ask of, unless this account has another device we know of.
        let me = self.my_device();
        if !self.st.known_devices.iter().any(|d| *d != me) { return; }
        let cids: Vec<String> = self.st.communities.values().filter(|c| !c.removed).map(|c| c.id.clone()).collect();
        for cid in cids {
            let ready = self.st.groups.get(&cid).is_some_and(|g| !g.removed && !g.broken && g.pending.is_none());
            if !ready || !self.synced.contains(&cid) { continue; }
            if self.st.relay_invite.is_empty() {
                if self.st.relay_secret_asked.get(&cid).is_none_or(|t| now >= *t + RELAY_SECRET_RETRY_SECS) {
                    if let Err(e) = self.queue_relay_secret_request(&cid) { log::warn!("ask for the relay secret: {e}"); }
                }
            } else if self.st.relay_secret_shared.get(&cid).is_none_or(|t| now >= *t + RELAY_SECRET_RETRY_SECS) {
                if let Err(e) = self.queue_relay_secret(&cid) { log::warn!("pass on the relay secret: {e}"); }
            }
        }
    }

    pub fn revoke_invite(&mut self, cid: &str, invite: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can revoke invites"); }
        let mut t = self.ctl_ext(cid)?;
        let listed = matches!(&t, GroupExt::Control { invites, .. } if invites.iter().any(|i| i.id == invite));
        if let GroupExt::Control { invites, .. } = &mut t { invites.retain(|i| i.id != invite); }
        let mut plan = Plan::empty("revoke invite");
        plan.gce = Some(t);
        if listed { self.run_commit(cid, plan)?; }
        // The sealed copy goes too. The invite is already dead (admins refuse it), so a relay that cannot be reached
        // now only means the copy lingers until it expires; the record stays so that revoking again retries.
        if let Some(rec) = self.st.pw_invites.get(invite).cloned() {
            self.api.pw_delete(&self.dev_key()?, &rec.lid).map_err(|e| anyhow!("The invite is revoked, but its copy could not be removed from the server just now ({e}). Revoke it again in a moment."))?;
            self.st.pw_invites.remove(invite);
            self.save()?;
        }
        Ok(())
    }

    pub fn set_recovery_unsaved(&mut self, on: bool) -> Result<()> { self.st.recovery_unsaved = on; self.save() }

    /// Take the sealed copies of password invites that are over (expired, or no longer listed) off the relay.
    pub fn prune_pw_invites(&mut self) {
        let now = now_secs();
        let live: BTreeSet<String> = self.st.communities.keys().filter_map(|c| self.ctl_ext(c).ok()).flat_map(|t| match t { GroupExt::Control { invites, .. } => invites.into_iter().map(|i| i.id).collect::<Vec<_>>(), _ => vec![] }).collect();
        let dead: Vec<String> = self.st.pw_invites.iter().filter(|(id, r)| r.expires <= now || !live.contains(*id)).map(|(id, _)| id.clone()).collect();
        if dead.is_empty() { return; }
        let Ok(k) = self.dev_key() else { return };
        for id in dead {
            let lid = self.st.pw_invites[&id].lid.clone();
            if self.api.pw_delete(&k, &lid).is_ok() { self.st.pw_invites.remove(&id); }
        }
        let _ = self.save();
    }

    /// Joining only records the intent; the request goes out with the next round and an admin must be online
    /// (now or later) to add us.
    pub fn join_with_link(&mut self, link: &str) -> Result<String> {
        let l = InviteLink::decode(link)?;
        if l.relay.trim_end_matches('/') != self.st.relay.trim_end_matches('/') { bail!("this invite is for a different relay ({})", l.relay); }
        if self.st.communities.get(&l.community).is_some_and(|c| !c.removed) { bail!("you are already in this community"); }
        self.st.join_refused.remove(&l.community);
        self.st.join_retry.remove(&l.community);
        self.st.joining.insert(l.community.clone(), l.clone());
        self.st.join_sent.remove(&l.community);
        // Fresh each time: after leaving, an old one is withdrawn on purpose.
        let consent = sign_consent(&self.acct_key()?, &l.community, now_secs());
        self.st.join_consent.insert(l.community.clone(), consent);
        self.save()?;
        Ok(l.community)
    }

    /// Stop waiting for a join, or put away the news that one was refused.
    pub fn cancel_join(&mut self, cid: &str) -> Result<()> {
        if let Some(c) = self.st.join_consent.remove(cid) { self.withdraw_consent(c.sig); }
        self.st.joining.remove(cid);
        self.st.join_sent.remove(cid);
        self.st.join_asked.remove(cid);
        self.st.join_retry.remove(cid);
        self.st.join_refused.remove(cid);
        self.save()
    }

    /// The bytes of a join request to post.
    pub(crate) fn join_request_data(&self, l: &InviteLink) -> Result<Vec<u8>> {
        let isk = SigningKey::from_bytes(&hex32(&l.sk)?);
        let cert = self.cert()?;
        let sig = isk.sign(&join_msg(&l.community, &cert.device, &l.invite));
        let jr = JoinRequest { invite: l.invite.clone(), cert, sig: hex::encode(sig.to_bytes()), consent: self.st.join_consent.get(&l.community).cloned() };
        let mut data = vec![TAG_JOIN_REQUEST]; data.extend(serde_json::to_vec(&jr)?);
        Ok(data)
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
        if role == Role::Owner { bail!("use transfer ownership to make someone the owner"); }
        if account == self.my_account() { bail!("you are the owner; transfer ownership first"); }
        let ak = self.acct_key()?;
        let roster = self.st.groups.get(cid).map(|g| g.accounts.clone()).unwrap_or_default();
        let mut t = self.ctl_ext(cid)?;
        let GroupExt::Control { admins, .. } = &mut t else { bail!("bad table") };
        // Nobody who has left keeps a grant in the table, whatever brought us here.
        admins.retain(|a| a.account != account && roster.contains(&a.account));
        if role == Role::Admin {
            if !roster.contains(account) { bail!("that person is not in the community"); }
            admins.push(sign_grant(&ak, cid, account));
        }
        let mut plan = Plan::empty(&format!("set role {}", role.name()));
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        // A grant cannot be recalled, only stopped from counting: the relay is told, and each room's copy of the list is
        // rewritten right away, not whenever the next change happens to come along.
        let job = if role == Role::Admin { AclJob { gid: cid.into(), promote: vec![account.into()], ..Default::default() } } else { AclJob { gid: cid.into(), demote: vec![account.into()], ..Default::default() } };
        self.st.acl_jobs.push(job);
        self.reconcile();
        self.save()
    }

    /// Hand the community to another member. The previous owner stays on as an admin, and can then leave like anyone.
    /// The signed step goes into the control table here; the new owner carries it into the rooms and re-signs the admins.
    pub fn transfer_ownership(&mut self, cid: &str, to: &str) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can transfer ownership"); }
        let me = self.my_account();
        if to == me { bail!("you already own this community"); }
        if !self.st.groups.get(cid).is_some_and(|g| g.accounts.contains(to)) { bail!("that person is not in the community"); }
        let ak = self.acct_key()?;
        let mut t = self.ctl_ext(cid)?;
        let GroupExt::Control { admins, transfers, .. } = &mut t else { bail!("bad table") };
        let seq = transfers.len() as u64 + 1;
        let sig = hex::encode(ak.sign(&transfer_msg(cid, seq, &me, to)).to_bytes());
        let step = OwnerStep { seq, from: me.clone(), to: to.into(), sig };
        transfers.push(step.clone());
        admins.retain(|a| a.account != to);
        admins.push(sign_grant(&ak, cid, &me));
        let mut plan = Plan::empty("transfer ownership");
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        self.st.acl_jobs.push(AclJob { gid: cid.into(), transfer: Some(step), ..Default::default() });
        self.save()
    }

    pub fn leave(&mut self, cid: &str) -> Result<()> {
        if self.my_role(cid) == Role::Owner { bail!("the owner cannot leave: transfer ownership to someone first"); }
        self.queue_env(cid, Envelope::Leave);
        self.save()
    }

    /// Change the table's list of rooms. Admins may, like creating one; the table is the only place a room's name and existence live.
    fn edit_rooms(&mut self, cid: &str, why: &str, f: impl FnOnce(&mut Vec<ChanMeta>) -> Result<()>) -> Result<()> {
        let mut t = self.ctl_ext(cid)?;
        let GroupExt::Control { channels, .. } = &mut t else { bail!("bad table") };
        f(channels)?;
        let mut plan = Plan::empty(why);
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        self.reconcile();
        Ok(())
    }

    pub fn rename_channel(&mut self, cid: &str, room: &str, new_name: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can rename rooms"); }
        let name = new_name.trim().to_lowercase().replace(' ', "-");
        check_name("a room name", &name).map_err(|e| anyhow!(e))?;
        self.edit_rooms(cid, "rename channel", |chs| {
            if chs.iter().any(|c| c.id != room && name_key(&c.name) == name_key(&name)) { bail!("there is already a room called {name}"); }
            let ch = chs.iter_mut().find(|c| c.id == room).ok_or_else(|| anyhow!("that room does not exist"))?;
            ch.name = name.clone();
            Ok(())
        })
    }

    /// Takes the room out of the table: nobody lists it any more. Never the last room, as a community without one is only a thing to explain.
    pub fn delete_channel(&mut self, cid: &str, room: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can delete rooms"); }
        self.edit_rooms(cid, "delete channel", |chs| {
            if !chs.iter().any(|c| c.id == room) { bail!("that room does not exist"); }
            if chs.len() <= 1 { bail!("a community needs at least one room"); }
            chs.retain(|c| c.id != room);
            Ok(())
        })
    }

    /// Lets a removed person be invited again. They are not added back; they need a new invite.
    pub fn unban(&mut self, cid: &str, account: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can lift a ban"); }
        let mut t = self.ctl_ext(cid)?;
        let GroupExt::Control { banned, .. } = &mut t else { bail!("bad table") };
        if !banned.iter().any(|a| a == account) { bail!("that person is not banned"); }
        banned.retain(|a| a != account);
        let mut plan = Plan::empty("unban");
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        Ok(())
    }

    pub fn rename_community(&mut self, cid: &str, new_name: &str) -> Result<()> {
        if self.my_role(cid) < Role::Admin { bail!("only admins can rename"); }
        let new_name = new_name.trim();
        check_name("a community name", new_name).map_err(|e| anyhow!(e))?;
        let mut t = self.ctl_ext(cid)?;
        if let GroupExt::Control { name, .. } = &mut t { *name = new_name.into(); }
        let mut plan = Plan::empty("rename");
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        Ok(())
    }

    /// The style document as it stands, with entries for rooms that no longer exist dropped (so it stays small). The community's own id is a
    /// valid key too: its card has a colour pair, a look, a lock and a wall of its own.
    fn style_base(&self, cid: &str) -> Result<StyleDoc> {
        let c = self.st.communities.get(cid).ok_or_else(|| anyhow!("unknown community"))?;
        let mut doc = c.style.clone().unwrap_or_default();
        let mut live: BTreeSet<&str> = c.channels.iter().map(|ch| ch.id.as_str()).collect();
        live.insert(cid);
        doc.rooms.retain(|g, _| live.contains(g.as_str()));
        doc.locked.retain(|g| live.contains(g.as_str()));
        doc.wall.retain(|g, _| live.contains(g.as_str()));
        doc.looks.retain(|g, _| live.contains(g.as_str()));
        Ok(doc)
    }
    fn has_room(&self, cid: &str, room: &str) -> bool { self.st.communities.get(cid).is_some_and(|c| c.channels.iter().any(|ch| ch.id == room)) }

    /// The owner sets the colours of one room (`room` = its id) or of the community's own card (`room` = None). `None` for
    /// `duo` puts the default back. Anybody else is refused here, and their device would refuse to believe it anyway: the
    /// table only carries colours the owner's key signed.
    pub fn set_style(&mut self, cid: &str, room: Option<&str>, duo: Option<Duo>) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can change colours"); }
        let duo = duo.map(|d| d.lower());
        if duo.as_ref().is_some_and(|d| !d.valid()) { bail!("colours must look like #rrggbb"); }
        if let Some(r) = room { if !self.has_room(cid, r) { bail!("unknown room"); } }
        let mut doc = self.style_base(cid)?;
        match (room, duo) {
            (Some(r), Some(d)) => { doc.rooms.insert(r.to_string(), d); }
            (Some(r), None) => { doc.rooms.remove(r); }
            (None, d) => doc.community = d,
        }
        self.publish_style(cid, doc, "colours")
    }

    /// Owner only: the look of one room (`room` = its id) or of the community's own card (`room` = None): the card pattern, the chat pattern and the
    /// backdrop strength. `None` for `look` (or one with no part set) puts everything back to automatic. Nothing is published when nothing changes.
    pub fn set_look(&mut self, cid: &str, room: Option<&str>, look: Option<Look>) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can change the look"); }
        let look = look.map(|mut l| { l.oracle_quiet = l.oracle_quiet.filter(|q| *q); l.oracle_draw = l.oracle_draw.filter(|q| *q); l.back = l.back.map(|d| d.lower()); l }).filter(|l| !l.is_empty());
        if look.as_ref().is_some_and(|l| !l.valid()) { bail!("a look has patterns 0 to {MAX_LOOK_PATTERN}, a strength of 0 to {MAX_LOOK_DIM} percent and backdrop colours like #rrggbb"); }
        if let Some(r) = room { if !self.has_room(cid, r) { bail!("unknown room"); } }
        if room.is_none() && look.as_ref().is_some_and(|l| l.chat.is_some() || l.dim.is_some() || l.oracle_quiet.is_some() || l.oracle_draw.is_some() || l.back.is_some()) { bail!("the community card has a card pattern only"); }
        let mut doc = self.style_base(cid)?;
        let key = room.unwrap_or(cid).to_string();
        let cur = self.st.communities.get(cid).and_then(|c| c.style.as_ref()).and_then(|d| d.looks.get(&key)).cloned();
        if cur == look { return Ok(()); }
        match look { Some(l) => { doc.looks.insert(key, l); } None => { doc.looks.remove(&key); } }
        self.publish_style(cid, doc, "look")
    }

    /// The look the owner has set for a room (or the community card when `room` is None), if any.
    pub fn look_of(&self, cid: &str, room: Option<&str>) -> Option<Look> {
        self.st.communities.get(cid)?.style.as_ref()?.looks.get(room.unwrap_or(cid)).cloned()
    }

    /// Owner only: switch Oracle off (`on`) or back on for one room. Only that part of the room's look changes.
    pub fn set_oracle_quiet(&mut self, cid: &str, room: &str, on: bool) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can change the look"); }
        let mut look = self.look_of(cid, Some(room)).unwrap_or_default();
        look.oracle_quiet = Some(on);
        self.set_look(cid, Some(room), Some(look))
    }

    /// Owner only: let Oracle doodle on this room's backdrop now and then (`on`), or not. Only that part of the room's look changes.
    pub fn set_oracle_draw(&mut self, cid: &str, room: &str, on: bool) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can change the look"); }
        let mut look = self.look_of(cid, Some(room)).unwrap_or_default();
        look.oracle_draw = Some(on);
        self.set_look(cid, Some(room), Some(look))
    }

    /// Owner only: lock a wall (`lock` = Some(true)) or open it again, and/or clear it (`clear`: its generation goes up by one, so every
    /// drawing and snapshot of the old generation is ignored from now on, by devices that were offline too). Both live in the signed style document.
    /// `room` is a room's id, or the community's own id for the community card's wall.
    pub fn set_wall(&mut self, cid: &str, room: &str, lock: Option<bool>, clear: bool) -> Result<()> {
        if self.my_role(cid) != Role::Owner { bail!("only the owner can lock or clear a wall"); }
        if room != cid && !self.has_room(cid, room) { bail!("unknown room"); }
        let mut doc = self.style_base(cid)?;
        match lock { Some(true) => { doc.locked.insert(room.to_string()); } Some(false) => { doc.locked.remove(room); } None => {} }
        if clear { let n = doc.wall.entry(room.to_string()).or_insert(0); *n += 1; }
        if lock.is_none() && !clear { return Ok(()); }
        self.publish_style(cid, doc, "wall")
    }

    fn publish_style(&mut self, cid: &str, mut doc: StyleDoc, label: &str) -> Result<()> {
        doc.ver = self.st.communities.get(cid).and_then(|c| c.style.as_ref()).map_or(0, |s| s.ver) + 1;
        doc.ts = now_secs();
        let signed = sign_style(&self.acct_key()?, cid, &doc).map_err(|e| anyhow!(e))?;
        let mut t = self.ctl_ext(cid)?;
        if let GroupExt::Control { style, .. } = &mut t { *style = Some(signed); }
        let mut plan = Plan::empty(label);
        plan.gce = Some(t);
        if self.run_commit(cid, plan)? == Outcome::Lost { bail!("the community changed at the same time; try again"); }
        Ok(())
    }

    // ------------------------------------------------------------------ messages
    pub(crate) fn queue_env(&mut self, gid: &str, env: Envelope) {
        let id = match &env { Envelope::Text { id, .. } | Envelope::File { id, .. } => id.clone(), _ => rand_hex(16) };
        if let Ok(cert) = self.cert() {
            match &env {
                Envelope::Profile { name, ts, avatar, avatar_cleared, .. } => {
                    let p = self.st.profiles.entry(cert.account.clone()).or_default();
                    if *ts >= p.ts {
                        p.name = name.clone();
                        p.ts = *ts;
                        if *avatar_cleared { p.avatar = None; }
                        else if avatar.is_some() { p.avatar = avatar.clone(); }
                    }
                }
                Envelope::Leave | Envelope::Revoked { .. } => {}
                _ => self.apply_chat(gid, 0, &cert, env.clone(), true),
            }
        }
        self.st.outbox.push(OutItem { id: if matches!(env, Envelope::Text { .. } | Envelope::File { .. }) { rand_hex(16) } else { id }, gid: gid.into(), env, cipher: None, tries: 0 });
        self.activity += 1;
    }
    pub(crate) fn check_chan(&self, gid: &str) -> Result<()> {
        if self.st.self_revoked { bail!("this device has been revoked, so it can no longer send"); }
        match self.st.groups.get(gid) { Some(g) if g.kind == Kind::Channel && !g.removed => Ok(()), _ => bail!("not a channel you are in") }
    }
    pub fn send_text(&mut self, gid: &str, text: &str, reply: Option<String>) -> Result<String> {
        self.check_chan(gid)?;
        if text.trim().is_empty() || text.len() > MAX_TEXT { bail!("message must be 1-{MAX_TEXT} bytes"); }
        let id = rand_hex(16);
        let env = Envelope::Text { id: id.clone(), text: text.into(), reply, ts: now_secs() };
        env.validate().map_err(|e| anyhow!("{e}"))?;
        self.queue_env(gid, env);
        self.save()?;
        Ok(id)
    }
    pub fn edit_message(&mut self, gid: &str, id: &str, text: &str) -> Result<()> {
        self.check_chan(gid)?;
        let mine = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == id)).is_some_and(|m| m.sender == self.my_account() && !m.deleted);
        if !mine { bail!("you can only edit your own messages"); }
        if text.trim().is_empty() || text.len() > MAX_TEXT { bail!("message must be 1-{MAX_TEXT} bytes"); }
        let env = Envelope::Edit { target: id.into(), text: text.into(), ts: now_secs() };
        env.validate().map_err(|e| anyhow!("{e}"))?;
        self.queue_env(gid, env); self.save()
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
        let env = Envelope::React { target: id.into(), emoji: emoji.into(), on };
        env.validate().map_err(|_| anyhow!("that is not a reaction"))?;
        if !self.st.chans.get(gid).is_some_and(|l| l.msgs.iter().any(|m| m.id == id && !m.deleted)) { bail!("no such message"); }
        self.queue_env(gid, env); self.save()
    }
    /// Everything about a file message that can go wrong before any bytes move. Encrypts the file (no network).
    pub fn file_prepare(&self, gid: &str, name: &str, mime: &str, data: &[u8], caption: &str) -> Result<OutFile> {
        self.file_prepare_voice(gid, name, mime, data, caption, None)
    }
    pub fn file_prepare_voice(&self, gid: &str, name: &str, mime: &str, data: &[u8], caption: &str, voice: Option<crate::envelope::Voice>) -> Result<OutFile> {
        self.check_chan(gid)?;
        if data.len() > MAX_BLOB_BYTES - 64 { bail!("file is larger than the {} MB limit", MAX_BLOB_BYTES / 1024 / 1024); }
        if caption.len() > MAX_TEXT { bail!("the caption is longer than {MAX_TEXT} bytes"); }
        let mut key = [0u8; 32]; let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut key); rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = ChaCha20Poly1305::new((&key).into()).encrypt(Nonce::from_slice(&nonce), data).map_err(|_| anyhow!("encrypt"))?;
        let safe: String = name.chars().filter(|c| !c.is_control()).collect();
        let safe = cut_name(safe.trim(), 200);
        let env = Envelope::File { id: rand_hex(16), name: if safe.is_empty() { "file".into() } else { safe }, mime: clean_mime(mime), size: data.len() as u64, blob: rand_hex(16), key: hex::encode(key), nonce: hex::encode(nonce), caption: caption.into(), reply: None, ts: now_secs(), large: None, voice };
        env.validate().map_err(|e| anyhow!("{e}"))?;
        Ok(OutFile { gid: gid.into(), env, ct })
    }
    /// The bytes are on the relay: now the message may go.
    pub fn file_queue(&mut self, f: OutFile) -> Result<String> {
        let Envelope::File { id, .. } = &f.env else { bail!("not a file") };
        let id = id.clone();
        self.check_chan(&f.gid)?;
        self.queue_env(&f.gid, f.env);
        self.save()?;
        Ok(id)
    }
    pub fn send_file(&mut self, gid: &str, name: &str, mime: &str, data: &[u8], caption: &str) -> Result<String> {
        let f = self.file_prepare(gid, name, mime, data, caption)?;
        f.upload(&self.api, &self.dev_key()?)?;
        self.file_queue(f)
    }
    /// What it takes to get one attachment: from the cache if we can, otherwise from the relay.
    pub fn file_locate(&self, gid: &str, msg_id: &str) -> Result<InFile> {
        let m = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == msg_id)).ok_or_else(|| anyhow!("no such message"))?;
        let MsgBody::File { name, mime, blob, key, nonce, .. } = &m.body else { bail!("not a file") };
        // The cache is keyed by everything that identifies the plaintext. By blob id alone, anyone who knows an id could
        // send a message naming it with their own name and type, and get the cached bytes shown under them.
        let slot = hex::encode(sha256(&[b"commons/cache/v1", blob.as_bytes(), key.as_bytes(), nonce.as_bytes()]));
        let cached = std::fs::read(self.dir.join("blobs").join(&slot)).ok().and_then(|s| self.vault.unseal(&s).ok());
        Ok(InFile { name: name.clone(), mime: mime.clone(), blob: blob.clone(), key: key.clone(), nonce: nonce.clone(), slot, cached })
    }
    /// Keep a decrypted attachment for next time, sealed with the vault key.
    pub fn file_cache(&self, slot: &str, plain: &[u8]) {
        let _ = std::fs::create_dir_all(self.dir.join("blobs"));
        if let Ok(sealed) = self.vault.seal(plain) { let _ = std::fs::write(self.dir.join("blobs").join(slot), sealed); }
    }
    pub fn fetch_file(&mut self, gid: &str, msg_id: &str) -> Result<(String, String, Vec<u8>)> {
        let mut f = self.file_locate(gid, msg_id)?;
        let fresh = f.cached.is_none();
        let plain = f.bytes(&self.api, &self.dev_key()?)?;
        if fresh { self.file_cache(&f.slot, &plain); }
        Ok((f.name, f.mime, plain))
    }
    // ------------------------------------------------------------------ profile pictures
    /// Everything about a picture that can go wrong before any bytes move. Encrypts it exactly like an attachment (no
    /// network). The picture is one per account: it lives in the account-keyed profile every device already shares.
    pub fn avatar_prepare(&self, mime: &str, shape: &str, data: &[u8]) -> Result<OutAvatar> {
        if !self.has_identity() { bail!("this device has no identity yet"); }
        if data.is_empty() { bail!("the picture is empty"); }
        if data.len() as u64 > MAX_AVATAR { bail!("the picture is larger than {} KB after shrinking", MAX_AVATAR / 1024); }
        let mime = clean_mime(mime);
        if !mime.starts_with("image/") { bail!("that is not a picture"); }
        let mut key = [0u8; 32]; let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut key); rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = ChaCha20Poly1305::new((&key).into()).encrypt(Nonce::from_slice(&nonce), data).map_err(|_| anyhow!("encrypt"))?;
        let shape = if shape == "square" { "square" } else { "circle" };
        let r = AvatarRef { blob: rand_hex(16), key: hex::encode(key), nonce: hex::encode(nonce), mime, size: data.len() as u64, shape: shape.into() };
        r.validate().map_err(|e| anyhow!("{e}"))?;
        Ok(OutAvatar { r, ct })
    }
    /// The bytes are on the relay: now the next profile message may carry the reference.
    pub fn avatar_set(&mut self, r: AvatarRef) -> Result<()> {
        if !self.has_identity() { bail!("this device has no identity yet"); }
        r.validate().map_err(|e| anyhow!("{e}"))?;
        let me = self.my_account();
        // Never go backwards on this account's clock: another device of ours may have set a later time, and a picture
        // sent with an older ts would be ignored by everyone.
        let ts = now_secs().max(self.st.profiles.get(&me).map_or(0, |p| p.ts.saturating_add(1)));
        let name = self.st.name.clone();
        let p = self.st.profiles.entry(me).or_default();
        if p.name.is_empty() { p.name = name; }
        p.avatar = Some(r);
        p.ts = ts;
        self.st.avatar_removed = false;
        self.st.profile_dirty = true;
        self.save()
    }
    /// Back to the generated seal, on every device of this account and in every community.
    pub fn avatar_clear(&mut self) -> Result<()> {
        if !self.has_identity() { bail!("this device has no identity yet"); }
        let me = self.my_account();
        let ts = now_secs().max(self.st.profiles.get(&me).map_or(0, |p| p.ts.saturating_add(1)));
        let name = self.st.name.clone();
        let p = self.st.profiles.entry(me).or_default();
        if p.name.is_empty() { p.name = name; }
        p.avatar = None;
        p.ts = ts;
        self.st.avatar_removed = true;
        self.st.profile_dirty = true;
        self.save()
    }
    /// For tests and simple callers: prepare, upload, and set in one go.
    pub fn set_avatar(&mut self, mime: &str, data: &[u8]) -> Result<AvatarRef> {
        self.set_avatar_shaped(mime, "circle", data)
    }
    pub fn set_avatar_shaped(&mut self, mime: &str, shape: &str, data: &[u8]) -> Result<AvatarRef> {
        let f = self.avatar_prepare(mime, shape, data)?;
        f.upload(&self.api, &self.dev_key()?)?;
        let r = f.r.clone();
        self.avatar_set(r.clone())?;
        Ok(r)
    }
    pub fn avatar_of(&self, account: &str) -> Option<AvatarRef> {
        self.st.profiles.get(account).and_then(|p| p.avatar.clone())
    }
    /// Account -> blob id, the cheap check the UI polls: download only the pictures that changed.
    pub fn avatar_versions(&self) -> BTreeMap<String, String> {
        self.st.profiles.iter().filter_map(|(a, p)| p.avatar.as_ref().map(|v| (a.clone(), v.blob.clone()))).collect()
    }
    /// What it takes to show one person's picture: from the cache if we can, otherwise from the relay.
    pub fn avatar_locate(&self, account: &str) -> Result<InAvatar> {
        let avatar = self.avatar_of(account).ok_or_else(|| anyhow!("no profile picture"))?;
        avatar.validate().map_err(|e| anyhow!("{e}"))?;
        let slot = hex::encode(sha256(&[b"commons/avatar-cache/v1", avatar.blob.as_bytes(), avatar.key.as_bytes(), avatar.nonce.as_bytes()]));
        let cached = std::fs::read(self.dir.join("blobs").join(&slot)).ok().and_then(|s| self.vault.unseal(&s).ok());
        Ok(InAvatar { avatar, slot, cached })
    }
    /// One person's picture, from the cache when it is there.
    pub fn fetch_avatar(&mut self, account: &str) -> Result<(String, Vec<u8>)> {
        let mut f = self.avatar_locate(account)?;
        let fresh = f.cached.is_none();
        let plain = f.bytes(&self.api, &self.dev_key()?)?;
        if fresh { self.file_cache(&f.slot, &plain); }
        Ok((f.avatar.mime.clone(), plain))
    }
    /// Best-effort typing hint. On by default for new accounts; an existing account keeps its choice (see THREAT-MODEL.md).
    pub fn send_typing(&mut self, gid: &str) {
        if !self.st.typing_enabled || self.check_chan(gid).is_err() || !self.ready_to_encrypt(gid) { return; }
        let key = format!("{gid}|me");
        let now = now_secs();
        if self.typing.get(&key).is_some_and(|t| now < t + 4) { return; }
        self.typing.insert(key, now);
        self.queue_env(gid, Envelope::Typing);
    }
    /// Offer live data to a room. Best effort: it is only sent when the room can encrypt right now, at most four a second
    /// per room, and when several of one kind pile up only the newest goes. Returns whether it was taken (false = dropped).
    pub fn live_send(&mut self, gid: &str, kind: &str, data: &str) -> Result<bool> {
        if !valid_live_kind(kind) { bail!("bad live kind"); }
        if data.len() > MAX_LIVE { bail!("live data is too large (4096 bytes at most)"); }
        self.check_chan(gid)?;
        if !self.live_sendable(gid) { return Ok(false); }
        let q = self.live_out.entry(gid.to_string()).or_default();
        if !q.contains_key(kind) && q.len() >= LIVE_KINDS { return Ok(false); }
        q.insert(kind.to_string(), data.to_string());
        self.flush_live();
        Ok(true)
    }
    /// Whether a live event of this kind would have to wait (one is already waiting, or the room has sent its four this second), and so
    /// would be replaced by the next: a sender that must not lose events asks first and tries again later.
    pub(crate) fn live_would_wait(&self, gid: &str, kind: &str) -> bool {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        self.live_out.get(gid).is_some_and(|q| q.contains_key(kind))
            || self.live_sent.get(gid).is_some_and(|w| w.iter().filter(|t| now_ms < **t + 1000).count() >= self.live_send_cap())
    }
    /// Events a room may send per second: more on the unstored channel (live.rs), the old four through the log.
    fn live_send_cap(&self) -> usize { if self.live_chan_on() { crate::live::LIVE_CHAN_PER_SEC } else { LIVE_SEND_PER_SEC } }
    fn live_sendable(&self, gid: &str) -> bool { if self.live_chan_on() { self.live_chan_ready(gid) } else { self.ready_to_encrypt(gid) } }
    /// Send what the rate allows. Anything not ready to go out now is dropped, not kept for later.
    pub(crate) fn flush_live(&mut self) {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let gids: Vec<String> = self.live_out.keys().cloned().collect();
        for gid in gids {
            if !self.live_sendable(&gid) { self.live_out.remove(&gid); continue; }
            let (chan, cap) = (self.live_chan_on(), self.live_send_cap());
            let win = self.live_sent.entry(gid.clone()).or_default();
            while win.front().is_some_and(|t| now_ms >= *t + 1000) { win.pop_front(); }
            let mut send = vec![];
            while win.len() < cap {
                let Some(kind) = self.live_out.get(&gid).and_then(|q| q.keys().next().cloned()) else { break };
                let data = self.live_out.get_mut(&gid).and_then(|q| q.remove(&kind)).unwrap_or_default();
                win.push_back(now_ms);
                send.push((kind, data));
            }
            if self.live_out.get(&gid).is_some_and(|q| q.is_empty()) { self.live_out.remove(&gid); }
            for (kind, data) in send {
                // The unstored channel when the relay has it (nothing in the log, no MLS state touched); else the log, as before.
                if chan { self.live_chan_seal(&gid, &kind, &data); } else { self.queue_env(&gid, Envelope::Live { kind, data, ts: now_secs() }); }
            }
        }
        self.live_sent.retain(|_, w| !w.is_empty());
    }
    /// Live events newer than `since` (0 for all still held), oldest first, from rooms we are still in.
    pub fn live_poll(&self, since: u64, gid: Option<&str>) -> Vec<LiveEvent> {
        self.live_in.iter().filter(|e| e.id > since && gid.is_none_or(|g| g == e.gid) && self.st.groups.get(&e.gid).is_some_and(|g| !g.removed)).cloned().collect()
    }
    /// Ring a voice room: everyone in it who is not already in the call, or one member (`to`, an account). Best effort like live
    /// data: it goes only if the room can encrypt right now, is not retried, and is dropped if it waits more than a few seconds.
    /// Returns the ring's id, which `ring_end` uses to take it back. At most one ring per room every 5 seconds from here.
    pub fn ring_send(&mut self, gid: &str, to: Option<&str>) -> Result<String> {
        self.check_chan(gid)?;
        let comm = self.st.groups.get(gid).map(|g| g.community.clone()).unwrap_or_default();
        if !self.st.communities.get(&comm).is_some_and(|c| c.channels.iter().any(|ch| ch.id == gid && ch.kind == ROOM_VOICE)) { bail!("only a voice room can be rung"); }
        if let Some(t) = to {
            if t == self.my_account() { bail!("you cannot ring yourself"); }
            if !self.st.groups.get(gid).is_some_and(|g| g.accounts.contains(t)) { bail!("that person is not in this room"); }
        }
        if !self.ready_to_encrypt(gid) { bail!("this room is not ready yet (its keys are still being exchanged); try again in a moment"); }
        let now = now_secs();
        let key = format!("{gid}|{}", to.unwrap_or(""));
        if self.ring_sent.get(&key).is_some_and(|t| now < *t + 5) { bail!("you rang a moment ago"); }
        self.ring_sent.insert(key, now);
        let id = rand_hex(16);
        self.queue_env(gid, Envelope::Ring { id: id.clone(), ts: now, to: to.map(String::from), end: false });
        self.save()?;
        Ok(id)
    }
    /// Take a ring back (the caller joined the answer or gave up). Same best-effort delivery.
    pub fn ring_end(&mut self, gid: &str, ring: &str, to: Option<&str>) -> Result<()> {
        self.check_chan(gid)?;
        if !is_id(ring) { bail!("bad ring id"); }
        if !self.ready_to_encrypt(gid) { return Ok(()); }
        self.queue_env(gid, Envelope::Ring { id: ring.to_string(), ts: now_secs(), to: to.map(String::from), end: true });
        self.save()?;
        Ok(())
    }
    /// Test bridge only: a ring stamped `age` seconds ago, to show that a stale one rings nobody.
    #[cfg(feature = "devbridge")]
    #[doc(hidden)]
    pub fn debug_ring_aged(&mut self, gid: &str, age: u64) -> Result<String> {
        let id = rand_hex(16);
        self.queue_env(gid, Envelope::Ring { id: id.clone(), ts: now_secs().saturating_sub(age), to: None, end: false });
        self.save()?;
        Ok(id)
    }
    /// Rings newer than `since` (0 for all still held), oldest first, from rooms we are still in.
    pub fn ring_poll(&self, since: u64) -> Vec<RingEvent> {
        self.ring_in.iter().filter(|e| e.id > since && self.st.groups.get(&e.gid).is_some_and(|g| !g.removed)).cloned().collect()
    }
    pub(crate) fn ready_to_encrypt(&self, gid: &str) -> bool {
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

    /// Encrypt everything that can be, with one save for the whole batch. The ratchet and the ciphertext reach the disk
    /// before anything reaches the network: never encrypt twice with one key.
    pub(crate) fn encrypt_outbox(&mut self) -> Result<()> {
        let mut changed = false;
        let mut gone = BTreeSet::new();
        for i in 0..self.st.outbox.len() {
            let (id, gid, ready) = { let it = &self.st.outbox[i]; (it.id.clone(), it.gid.clone(), it.cipher.is_some()) };
            let Some(gs) = self.st.groups.get(&gid).cloned() else { gone.insert(id); continue }; // group gone
            let live_ts = match &self.st.outbox[i].env { Envelope::Live { ts, .. } | Envelope::Ring { ts, .. } | Envelope::Direct { ts, .. } => Some(*ts), _ => None };
            // Live data is for now: not kept past a few seconds, never reported when it goes, and never held back by a bad moment.
            if let Some(ts) = live_ts {
                if gs.removed || ts + 10 < now_secs() || (!ready && !self.ready_to_encrypt(&gid)) { gone.insert(id); continue; }
            }
            if gs.removed { self.notice("outbox", "dropped an unsent message: you are no longer in that group"); gone.insert(id); continue; }
            if ready { continue; }
            if gs.tainted && gs.pending.is_none() && self.synced.contains(&gid) && !gs.broken {
                let mut p = Plan::empty("refresh keys after restore"); p.force_update = true;
                if let Err(e) = self.build_commit(&gid, p) { log::warn!("refresh after restore: {e}"); }
            }
            if !self.ready_to_encrypt(&gid) { continue; }
            let env = self.st.outbox[i].env.clone();
            match self.encrypt_env(&gid, &env) {
                Ok((data, nid)) => {
                    let it = &mut self.st.outbox[i];
                    it.cipher = Some(b64(&data)); it.id = nid;
                    changed = true;
                }
                Err(e) => {
                    let it = &mut self.st.outbox[i];
                    it.tries += 1;
                    if matches!(it.env, Envelope::Live { .. } | Envelope::Ring { .. } | Envelope::Direct { .. }) { gone.insert(id); continue; }
                    if it.tries > 20 { gone.insert(id); self.notice("outbox", format!("could not encrypt a message, giving up: {e}")); }
                }
            }
        }
        self.st.outbox.retain(|i| !gone.contains(&i.id));
        if changed { self.save()?; }
        Ok(())
    }

    // ------------------------------------------------------------------ commit machinery
    /// Create and persist one commit. No network: it goes out with the next round, or from `run_commit`.
    pub(crate) fn build_commit(&mut self, gid: &str, plan: Plan) -> Result<String> {
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
        // A member adding a device of their own needs nothing from the relay: their account is in already.
        let acl_add: Vec<String> = { let mut v: Vec<String> = plan.adds.iter().map(|(c, _)| c.account.clone()).filter(|a| !gs.accounts.contains(a)).collect(); v.sort(); v.dedup(); v };
        self.st.groups.get_mut(gid).unwrap().pending = Some(PendingCommit { msg_id: msg_id.clone(), data: b64(&data), welcome, recipients, acl_remove: plan.acl_remove.clone(), posted: false, desc: plan.desc.clone(), acl_add, handled: plan.handled.clone(), acl_demote: plan.acl_demote.clone() });
        self.save()?; // the staged commit and advanced key schedule hit disk before the network
        Ok(msg_id)
    }

    /// Give up on a commit the relay will not take.
    pub(crate) fn abandon_pending(&mut self, gid: &str, why: &str) {
        if let Some(mut g) = self.load_group(gid) { let _ = g.clear_pending_commit(self.prov.storage()); }
        self.drop_pending(gid, why);
    }

    /// Create, persist, post and (try to) confirm one commit, for actions that need the answer now. Never merges before
    /// the relay log shows it won.
    pub(crate) fn run_commit(&mut self, gid: &str, plan: Plan) -> Result<Outcome> {
        if self.st.groups.get(gid).is_some_and(|g| g.pending.is_some()) {
            self.try_post_pending(gid);
            let _ = self.sync_once(0);
            if self.st.groups[gid].pending.is_some() { return Ok(Outcome::Pending); }
        }
        self.catch_up()?;
        if !self.synced.contains(gid) { bail!("not caught up with the group yet"); }
        let msg_id = self.build_commit(gid, plan)?;
        let k = self.dev_key()?;
        let p = self.st.groups[gid].pending.clone().unwrap();
        if !p.acl_add.is_empty() {
            let req = AclReq { add: p.acl_add.clone(), grant: self.my_grant(&self.st.groups[gid].community.clone()), ..Default::default() };
            if let Err(e) = self.api.acl(&k, gid, &req) {
                self.abandon_pending(gid, "the relay would not let the new member in");
                self.save()?;
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

    pub(crate) fn try_post_pending(&mut self, gid: &str) {
        let Some(p) = self.st.groups.get(gid).and_then(|g| g.pending.clone()) else { return };
        let Ok(k) = self.dev_key() else { return };
        match self.api.post_entry(&k, gid, &p.msg_id, &unb64(&p.data).unwrap_or_default()) {
            Ok(_) => { if let Some(g) = self.st.groups.get_mut(gid) { if let Some(pp) = g.pending.as_mut() { pp.posted = true; } } }
            Err(e) => log::warn!("commit post failed, will retry: {e}"),
        }
    }

    // ------------------------------------------------------------------ reconcile
    /// Bring group membership in line with the roster (control group) and the device directory.
    /// Admins/owner reconcile everyone; plain members only reconcile their own devices.
    /// This is planning, with no network: what it needs from the relay it asks for, and it finds the answer on a later
    /// round. The commits it makes go out with the next round.
    pub fn reconcile(&mut self) {
        let cids: Vec<String> = self.st.communities.values().filter(|c| !c.removed).map(|c| c.id.clone()).collect();
        for cid in cids {
            if let Err(e) = self.reconcile_community(&cid) { log::warn!("reconcile {}: {e}", Self::short(&cid)); }
        }
    }

    fn dir_for(&mut self, acct: &str) -> Vec<(DeviceCert, bool)> {
        let now = now_secs();
        match self.dir_cache.get(acct) {
            Some((t, d)) => { let d = d.clone(); if now >= t + self.dir_ttl_secs { self.dir_wanted.insert(acct.to_string()); } d }
            None => { self.dir_wanted.insert(acct.to_string()); vec![] }
        }
    }

    fn reconcile_community(&mut self, cid: &str) -> Result<()> {
        let c = self.st.communities[cid].clone();
        let me = self.my_account();
        let role = self.my_role(cid);
        let Some(ctl) = self.st.groups.get(cid).cloned() else { return Ok(()) };
        if ctl.removed || ctl.broken || !self.synced.contains(cid) { return Ok(()); }
        // A table from before consents existed, or one that lost ours: offer it again, or our next device could not come in.
        if c.my_consent.is_none() {
            let now = now_secs();
            if self.consent_offered.get(cid).is_none_or(|t| now >= t + CONSENT_RETRY_SECS) {
                self.consent_offered.insert(cid.to_string(), now);
                let con = sign_consent(&self.acct_key()?, cid, now);
                if role >= Role::Admin { self.st.communities.get_mut(cid).unwrap().consent_queue.insert(me.clone(), con); }
                else { self.queue_env(cid, Envelope::Consent { ts: con.ts, sig: con.sig, relay_secret: None, relay_secret_want: false }); }
            }
        }
        let c = self.st.communities[cid].clone();
        // admins turn valid join requests into joiners
        let mut joiners: Vec<Joiner> = vec![];
        if role >= Role::Admin {
            // however many people hold a link, it only admits as many as it has uses left; the others wait for the
            // next pass, when the table shows the uses are gone and they are told
            let mut left: BTreeMap<String, u32> = BTreeMap::new();
            let reqs = ctl.requests.clone();
            for r in reqs.iter().filter(|r| r.tag == TAG_JOIN_REQUEST) {
                match self.check_join(&c, r) {
                    JoinCheck::Accept(cert, invite, consent) => {
                        if joiners.iter().any(|j| j.cert.device == cert.device) { self.drop_request(cid, r.seq); continue; }
                        let uses = c.invites.iter().find(|i| i.id == invite).and_then(|i| i.uses_left);
                        if let Some(n) = uses {
                            let l = left.entry(invite.clone()).or_insert(n);
                            if *l == 0 { continue; }
                            *l -= 1;
                        }
                        joiners.push(Joiner { cert, invite, seq: r.seq, consent });
                    }
                    JoinCheck::Stale => self.drop_request(cid, r.seq),
                    JoinCheck::Junk(why) => { self.notice("join", format!("ignored a join request: {why}")); self.drop_request(cid, r.seq); }
                    JoinCheck::Refuse(device, code) => {
                        self.notice("join", format!("turned down a join request: {}", join_reason_text(code)));
                        if let Err(e) = self.send_join_reply(&c, &device, code) { log::warn!("join reply: {e}"); }
                        self.drop_request(cid, r.seq);
                    }
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
            if let Err(e) = self.reconcile_group(&c, &gid, is_ctl, role, &me, &remove_accounts, if is_ctl { &joiners } else { &[] }) {
                log::warn!("reconcile group {}: {e}", Self::short(&gid));
            }
        }
        Ok(())
    }

    pub(crate) fn drop_request(&mut self, gid: &str, seq: u64) {
        if let Some(g) = self.st.groups.get_mut(gid) { g.requests.retain(|r| r.seq != seq); }
    }

    /// Validate a join request against the current control table.
    fn check_join(&self, c: &Community, r: &ReqItem) -> JoinCheck {
        let jr = match unb64(&r.data).map_err(|e| e.to_string()).and_then(|d| JoinRequest::parse(&d).map_err(|e| e.to_string())) { Ok(j) => j, Err(e) => return JoinCheck::Junk(e) };
        // Already in: that comes first, or a request answered by another admin would look like a used-up invite.
        if self.st.groups.get(&c.id).is_some_and(|g| g.dev_seq.contains_key(&jr.cert.device)) { return JoinCheck::Stale; }
        if self.st.revoked.contains_key(&jr.cert.device) { return JoinCheck::Junk("device revoked".into()); }
        let Some(inv) = c.invites.iter().find(|i| i.id == jr.invite) else { return JoinCheck::Refuse(jr.cert.device, JOIN_INVITE_GONE) };
        if !crate::sync::join_signature_ok(inv, &c.id, &jr) { return JoinCheck::Junk("bad invite signature".into()); }
        if c.banned.contains(&jr.cert.account) { return JoinCheck::Refuse(jr.cert.device, JOIN_BANNED); }
        if inv.expires < now_secs() || inv.uses_left == Some(0) { return JoinCheck::Refuse(jr.cert.device, JOIN_INVITE_GONE); }
        // A consent that does not check out is left out, and the joiner offers one again once in.
        // A one-to-one community lets in the one person its nonce names, whoever holds the invite (the offer travels in a room
        // other people share, and so does the secret in it).
        if crate::direct::admits(&c.name, c.direct.is_some(), if c.founder.is_empty() { &c.owner } else { &c.founder }, &c.nonce, &jr.cert.account) == Some(false) { return JoinCheck::Refuse(jr.cert.device, JOIN_INVITE_GONE); }
        let consent = jr.consent.filter(|x| x.account == jr.cert.account && verify_consent(&c.id, x).is_ok());
        JoinCheck::Accept(jr.cert, jr.invite, consent)
    }

    /// The joiner is in no group yet, so the answer goes to their inbox, the one place an outsider can be reached.
    fn send_join_reply(&mut self, c: &Community, device: &str, code: &str) -> Result<()> {
        let k = self.dev_key()?;
        let sig = k.sign(&join_reply_msg(&c.id, device, code));
        let reply = JoinReply { community: c.id.clone(), reason: code.into(), cert: self.cert()?, sig: hex::encode(sig.to_bytes()), grant: self.my_grant(&c.id), transfers: c.transfers.clone() };
        self.st.welcome_out.push(WelcomeOut { id: rand_hex(16), device: device.into(), data: b64(&serde_json::to_vec(&reply)?), gid: c.id.clone() });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn reconcile_group(&mut self, c: &Community, gid: &str, is_ctl: bool, role: Role, me: &str, remove_accounts: &BTreeSet<String>, joiners: &[Joiner]) -> Result<()> {
        let group = self.load_group(gid).ok_or_else(|| anyhow!("no group"))?;
        let members = mls::members_of(&group);
        let ctl_accounts = self.st.groups[&c.id].accounts.clone();
        let mut desired: BTreeSet<String> = ctl_accounts.difference(remove_accounts).cloned().collect();
        let gs = self.st.groups[gid].clone();
        let may = |a: &str| role >= Role::Admin || a == me;
        let mut plan = Plan::empty("");
        let mut descs: Vec<String> = vec![];
        // A room's table may still name the previous owner, and every other change here would be judged by that.
        // So the signed chain goes in first, on its own; the new owner is who can do that, with the same re-check as any commit.
        if !is_ctl && role == Role::Owner {
            if let (Some(GroupExt::Channel { community, name, owner, nonce, admins, transfers }), Ok(GroupExt::Control { admins: ctl_admins, transfers: ctl_transfers, .. })) = (mls::group_ext(group.extensions()), self.ctl_ext(&c.id)) {
                if transfers != ctl_transfers {
                    plan.gce = Some(GroupExt::Channel { community, name, owner, nonce, admins: ctl_admins, transfers: ctl_transfers });
                    plan.desc = "update owner".into();
                    self.build_commit(gid, plan)?;
                    return Ok(());
                }
                // keep each channel's own copy of the admin list current (only the owner may change it)
                if admins != ctl_admins { plan.gce = Some(GroupExt::Channel { community, name, owner, nonce, admins: ctl_admins, transfers }); descs.push("update channel admins".into()); }
            }
        }
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
            // Each request carries a fresh nonce: one that was already acted on is a replay.
            let used = format!("{}|{}", cert.device, nonce);
            if gs.rejoin_used.contains(&used) { handled_reqs.push(r.seq); continue; }
            // Remove and re-add are one commit, or the person drops off the roster and every room follows. So the old
            // leaf only goes once the key package for the new one is in hand; the request waits until then.
            if self.kp_cache.get(&cert.device).is_none_or(|v| v.is_empty()) { *self.kp_wanted.entry(cert.device.clone()).or_insert(0) += 1; continue; }
            if let Some(g) = self.st.groups.get_mut(gid) { g.rejoin_used.push(used); if g.rejoin_used.len() > 256 { g.rejoin_used.remove(0); } }
            rejoin_devices.push(cert); handled_reqs.push(r.seq);
        }
        if is_ctl { for j in joiners { desired.insert(j.cert.account.clone()); } }
        // removals
        for (idx, cert) in &members {
            let acct_removed = !desired.contains(&cert.account) && role >= Role::Admin && cert.account != c.owner
                && (is_ctl || ctl_accounts.contains(&cert.account) || true)
                && (role == Role::Owner || c.role_of(&cert.account) == Some(Role::Member));
            let revoked = self.st.revoked.contains_key(&cert.device) && may(&cert.account);
            let rejoin = rejoin_devices.iter().any(|d| d.device == cert.device);
            if acct_removed || revoked || rejoin {
                plan.removes.push(*idx);
                if acct_removed && !members.iter().any(|(i, m)| m.account == cert.account && !plan.removes.contains(i)) { plan.acl_remove.push(cert.account.clone()); }
            }
        }
        if !plan.removes.is_empty() { descs.push(format!("remove {} device(s)", plan.removes.len())); }
        // additions
        let have: BTreeSet<String> = members.iter().filter(|(i, _)| !plan.removes.contains(i)).map(|(_, c)| c.device.clone()).collect();
        let mut want: Vec<DeviceCert> = vec![];
        for acct in desired.iter().filter(|a| may(a)) {
            if is_ctl && !ctl_accounts.contains(acct) {
                for j in joiners.iter().filter(|j| &j.cert.account == acct) { want.push(j.cert.clone()); }
                continue;
            }
            if c.banned.contains(acct) { continue; }
            for (cert, revoked) in self.dir_for(acct) { if !revoked && !have.contains(&cert.device) { want.push(cert); } }
        }
        for j in &rejoin_devices { if !want.iter().any(|w| w.device == j.device) { want.push(j.clone()); } }
        if is_ctl {
            for j in joiners { if ctl_accounts.contains(&j.cert.account) && !have.contains(&j.cert.device) && !want.iter().any(|w| w.device == j.cert.device) && role >= Role::Admin { want.push(j.cert.clone()); } }
        }
        let mut seen = BTreeSet::new();
        want.retain(|w| seen.insert(w.device.clone()) && !self.st.revoked.contains_key(&w.device));
        for w in want {
            let Some(b) = self.kp_cache.get_mut(&w.device).and_then(|v| v.pop()) else { *self.kp_wanted.entry(w.device.clone()).or_insert(0) += 1; continue };
            match mls::parse_kp(&self.prov, &b) {
                Ok((kp, cert)) if cert.device == w.device && cert.account == w.account => plan.adds.push((cert, kp)),
                Ok(_) => self.notice("keypackage", format!("relay returned a key package for the wrong device (device {}); ignored", &w.device[..8])),
                Err(e) => self.notice("keypackage", format!("invalid key package for device {}: {e}", &w.device[..8])),
            }
        }
        if !plan.adds.is_empty() { descs.push(format!("add {} device(s)", plan.adds.len())); }
        // control table: consume invites, record bans, keep the admin list true to the roster
        if is_ctl && role >= Role::Admin {
            let mut t = self.ctl_ext(&c.id)?;
            let GroupExt::Control { admins, invites, banned, consents, .. } = &mut t else { bail!("bad table") };
            let mut changed = false;
            for j in joiners {
                if plan.adds.iter().any(|(c2, _)| c2.device == j.cert.device) {
                    if let Some(i) = invites.iter_mut().find(|i| i.id == j.invite) { if let Some(n) = i.uses_left.as_mut() { *n = n.saturating_sub(1); changed = true; } }
                    if let Some(con) = &j.consent { changed |= merge_consent(consents, con.clone()); }
                    plan.handled.push(j.seq);
                }
            }
            for (a, con) in &c.consent_queue { if desired.contains(a) { changed |= merge_consent(consents, con.clone()); } }
            let before = consents.len();
            consents.retain(|x| desired.contains(&x.account));
            if consents.len() != before { changed = true; }
            let before = invites.len();
            invites.retain(|i| i.uses_left != Some(0));
            if invites.len() != before { changed = true; }
            for a in &c.kick_queue { if !banned.contains(a) && members.iter().any(|(_, m)| &m.account == a) { banned.push(a.clone()); changed = true; } }
            if role == Role::Owner {
                // Someone leaving takes their grant with them, in the same commit; if they ever come back they come back
                // as a member. Grants from a previous owner are re-signed, since the relay only takes the owner's word.
                let ak = self.acct_key()?;
                let gone: Vec<String> = admins.iter().filter(|a| !desired.contains(&a.account)).map(|a| a.account.clone()).collect();
                admins.retain(|a| !gone.contains(&a.account));
                if !gone.is_empty() { plan.acl_demote = gone; changed = true; }
                for a in admins.iter_mut() {
                    if verify_grant(me, &c.id, &a.account, &a.sig).is_err() { *a = sign_grant(&ak, &c.id, &a.account); changed = true; }
                }
                // The colours we know about but the table lacks or holds an older version of (an older app rewrote it, a member put an
                // old document back, or ownership just changed hands): put them back under our own signature. The first repair is
                // immediate; the next waits 30 s, then twice as long while repairs keep being needed, up to ten minutes, so a
                // hostile member cannot make the owner commit faster than that.
                if let (Some(doc), GroupExt::Control { style, .. }) = (self.st.communities.get(&c.id).and_then(|x| x.style.clone()), &mut t) {
                    let in_force = style.as_ref().and_then(|s| verify_style(me, &c.id, s).ok()).is_some_and(|d| (d.ver, d.ts) >= (doc.ver, doc.ts));
                    let (last, gap) = self.st.communities.get(&c.id).map(|x| (x.style_repub, x.style_repub_gap)).unwrap_or((0, 0));
                    let due = now_secs() >= last + gap;
                    if !in_force && due {
                        let mut doc = doc;
                        doc.ver += 1; doc.ts = now_secs();
                        if let Ok(s) = sign_style(&ak, &c.id, &doc) {
                            *style = Some(s); changed = true;
                            if let Some(x) = self.st.communities.get_mut(&c.id) {
                                x.style_repub_gap = if now_secs() < last + 600 { (gap * 2).clamp(30, 600) } else { 30 };
                                x.style_repub = now_secs();
                            }
                        }
                    }
                }
            } else if let (Some(held), GroupExt::Control { style, .. }) = (self.st.communities.get(&c.id).and_then(|x| x.held_style()), &mut t) {
                // An admin that holds a newer owner-signed document than the table shows (a member put an older one back, an older app
                // dropped the field) carries its exact copy forward, under the same backoff as the owner's repair (audit M1). Only a
                // document the current owner signed is carried, and never anything this device signed itself. Newcomers this admin
                // lets in then read the right document at once, whether or not the owner is online.
                if let Ok(mine) = verify_style(&c.owner, &c.id, &held) {
                    let in_force = style.as_ref().and_then(|s| verify_style(&c.owner, &c.id, s).ok()).is_some_and(|d| (d.ver, d.ts) >= (mine.ver, mine.ts));
                    let (last, gap) = self.st.communities.get(&c.id).map(|x| (x.style_repub, x.style_repub_gap)).unwrap_or((0, 0));
                    if !in_force && now_secs() >= last + gap {
                        *style = Some(held); changed = true;
                        if let Some(x) = self.st.communities.get_mut(&c.id) {
                            x.style_repub_gap = if now_secs() < last + 600 { (gap * 2).clamp(30, 600) } else { 30 };
                            x.style_repub = now_secs();
                        }
                    }
                }
            }
            if changed { plan.gce = Some(t); descs.push("update table".into()); }
        }
        if plan.removes.is_empty() && plan.adds.is_empty() && plan.gce.is_none() {
            for s in handled_reqs { self.drop_request(gid, s); }
            return Ok(());
        }
        plan.desc = descs.join(", ");
        plan.handled.extend(handled_reqs);
        self.build_commit(gid, plan)?;
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
            ChannelInfo { gid: ch.id.clone(), name: ch.name.clone(), kind: ch.kind.clone(), joined, unread: self.unread(&ch.id) }
        }).collect()
    }
    pub fn unread(&self, gid: &str) -> usize {
        let me = self.my_account();
        self.st.chans.get(gid).map(|l| l.msgs.iter().skip(l.read_upto).filter(|m| m.sender != me && !m.deleted && !matches!(m.body, MsgBody::Notice(_))).count()).unwrap_or(0)
    }
    pub fn mark_read(&mut self, gid: &str) { if let Some(l) = self.st.chans.get_mut(gid) { l.read_upto = l.msgs.len(); } }
    pub fn messages(&self, gid: &str) -> Vec<Msg> { self.message_log(gid).into_iter().cloned().collect() }
    /// A room's messages in reading order, by reference: a page of a long history costs a page, not a copy of it all.
    pub fn message_log(&self, gid: &str) -> Vec<&Msg> {
        let v: Vec<&Msg> = self.st.chans.get(gid).map(|l| l.msgs.iter().collect()).unwrap_or_default();
        // relay order for delivered messages; local unsent ones stay at the end in the order sent
        let (mut sent, pending): (Vec<&Msg>, Vec<&Msg>) = v.into_iter().partition(|m| m.seq > 0 || m.status == "system");
        sent.sort_by_key(|m| (if m.seq > 0 { m.seq } else { u64::MAX }, m.ts));
        sent.extend(pending);
        sent
    }
    /// Each member's role, and whether their name is shared with someone else here.
    pub fn people_of(&self, cid: &str) -> BTreeMap<String, (Role, bool)> {
        let (Some(c), Some(g)) = (self.st.communities.get(cid), self.st.groups.get(cid)) else { return BTreeMap::new() };
        let keys: Vec<(&String, String)> = g.accounts.iter().map(|a| (a, name_key(&self.display_name(a)))).collect();
        keys.iter().map(|(a, k)| ((*a).clone(), (c.role_of(a).unwrap_or(Role::Member), keys.iter().filter(|(_, x)| x == k).count() > 1))).collect()
    }
    pub fn members(&self, cid: &str) -> Vec<MemberInfo> {
        let (Some(c), Some(g)) = (self.st.communities.get(cid), self.st.groups.get(cid)) else { return vec![] };
        let Some(group) = self.load_group(cid) else { return vec![] };
        let all = mls::members_of(&group);
        let me = self.my_account();
        let keys: Vec<String> = g.accounts.iter().map(|a| name_key(&self.display_name(a))).collect();
        g.accounts.iter().enumerate().map(|(i, a)| MemberInfo {
            account: a.clone(), name: self.display_name(a), role: c.role_of(a).unwrap_or(Role::Member),
            devices: all.iter().filter(|(_, m)| &m.account == a).count(), verified: self.st.verified.contains(a), is_me: *a == me,
            tag: account_tag(a), same_name: keys.iter().filter(|k| **k == keys[i]).count() > 1,
            listening: self.listening_of(a).cloned(),
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

    // ------------------------------------------------------------------ what rune is playing
    /// Where rune answers on this device (the default until the person changes it).
    pub fn rune_url(&self) -> String { if self.st.rune_url.is_empty() { crate::rune::DEFAULT_URL.into() } else { self.st.rune_url.clone() } }

    /// Turn sharing on or off. Turning it off tells friends at once, so nothing lingers for the TTL.
    pub fn set_listening_sharing(&mut self, on: bool, url: &str) -> Result<()> {
        let url = if url.trim().is_empty() { crate::rune::DEFAULT_URL.to_string() } else { crate::rune::clean_url(url).map_err(|e| anyhow!(e))? };
        self.st.rune_url = url;
        if self.st.share_listening == on { return self.save(); }
        self.st.share_listening = on;
        if !on {
            self.st.listening.remove(&self.my_account());
            if self.st.listening_sent.is_some() { self.send_listening(None); }
        }
        // Either way, the next status is news: turning sharing back on must not look like a renewal.
        self.st.listening_sent = None;
        self.st.listening_sent_ts = 0;
        self.save()
    }

    /// rune's latest answer. Sent only when it changes, or when a status that is still playing is due for renewal;
    /// the background poll calls this every few seconds, so nothing may be sent per poll.
    pub fn update_listening(&mut self, current: Option<NowPlaying>) -> Result<()> {
        if !self.st.share_listening { return Ok(()); }
        let current = current.filter(|n| !n.title.is_empty());
        let me = self.my_account();
        match &current {
            Some(n) => { self.st.listening.insert(me, Heard { what: n.clone(), ts: now_secs() }); }
            None => { self.st.listening.remove(&me); }
        }
        let changed = current.as_ref() != self.st.listening_sent.as_ref();
        let renew = current.is_some() && now_secs() >= self.st.listening_sent_ts.saturating_add(LISTENING_REFRESH_SECS);
        if changed || renew { self.send_listening(current); }
        self.save()
    }

    /// Put the status in every community's control group, where a profile already travels. Returns true if it went
    /// into at least one community.
    fn send_listening(&mut self, value: Option<NowPlaying>) -> bool {
        let name = self.st.name.clone();
        if name.is_empty() { return false; }
        let ctls: Vec<String> = self.st.groups.values().filter(|g| g.kind == Kind::Control && !g.removed && !g.broken && !g.tainted).map(|g| g.gid.clone()).collect();
        let ts = now_secs();
        // the picture rides along too, so a status refresh never looks like "no picture" to a device that missed one
        let avatar = self.st.profiles.get(&self.my_account()).and_then(|p| p.avatar.clone());
        for gid in &ctls { self.queue_env(gid, Envelope::Profile { name: name.clone(), ts, listen: value.clone(), avatar: avatar.clone(), avatar_cleared: false, direct: true }); }
        if ctls.is_empty() { return false; }
        self.st.listening_sent = value;
        self.st.listening_sent_ts = ts;
        true
    }

    /// What this account is listening to, if it was heard recently enough to still be true.
    pub fn listening_of(&self, acct: &str) -> Option<&NowPlaying> {
        self.st.listening.get(acct).filter(|h| now_secs() < h.ts.saturating_add(LISTENING_TTL_SECS)).map(|h| &h.what)
    }
    pub fn typing_now(&self, gid: &str) -> Vec<String> {
        let now = now_secs();
        self.typing.iter().filter(|(k, t)| k.starts_with(&format!("{gid}|")) && !k.ends_with("|me") && now < **t + 6).map(|(k, _)| self.display_name(k.split('|').nth(1).unwrap_or(""))).collect()
    }
    pub fn qr_svg(data: &str) -> String {
        qrcode::QrCode::new(data.as_bytes()).map(|q| q.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build()).unwrap_or_default()
    }
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        check_name("a name", name).map_err(|e| anyhow!(e))?;
        let me = self.my_account();
        // Two people answering to one name is how someone gets impersonated. Names are ours to pick, not the community's
        // to enforce (a modified client can ignore this), so receivers also show who is who: see `tag`.
        for c in self.st.communities.values().filter(|c| !c.removed) {
            let taken = self.st.groups.get(&c.id).is_some_and(|g| g.accounts.iter().any(|a| *a != me && name_key(&self.display_name(a)) == name_key(name)));
            if taken { bail!("someone in {} is already called that; pick another name", c.name); }
        }
        self.st.name = name.into(); self.st.profile_dirty = true; self.save()
    }
    pub fn group_status(&self, gid: &str) -> String {
        match self.st.groups.get(gid) {
            None => "not joined yet".into(),
            Some(g) if g.removed => "removed".into(),
            Some(g) if g.broken => "out of sync, waiting to be re-added".into(),
            Some(g) if g.tainted => "refreshing keys".into(),
            Some(g) if g.pending.is_some() => "applying a change".into(),
            Some(g) if g.denied => "the relay is not letting this account in".into(),
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
    /// A hostile admin adding someone to a group without asking them (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_add_account_devices(&mut self, gid: &str, account: &str) -> Result<()> {
        let k = self.dev_key()?; let dir = self.directory(account)?; let mut plan = Plan::empty("hostile add");
        for (cert, rev) in dir { if rev { continue; } if let Some(b) = self.api.take_kp(&k, &cert.device)? { let (kp, c) = mls::parse_kp(&self.prov, &b)?; plan.adds.push((c, kp)); } }
        self.run_commit(gid, plan).map(|_| ())
    }
    /// A table as a client from before consents would have left it (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_clear_consents(&mut self, cid: &str) -> Result<()> {
        let mut t = self.ctl_ext(cid)?;
        if let GroupExt::Control { consents, .. } = &mut t { consents.clear(); }
        let mut plan = Plan::empty("clear consents");
        plan.gce = Some(t);
        self.run_commit(cid, plan).map(|_| ())
    }
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_consents(&self, cid: &str) -> Vec<Consent> {
        match self.ctl_ext(cid) { Ok(GroupExt::Control { consents, .. }) => consents, _ => vec![] }
    }
    /// Put a consent in this community's table, as a hostile owner would (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_plant_consent(&mut self, cid: &str, con: Consent) -> Result<()> {
        let mut t = self.ctl_ext(cid)?;
        if let GroupExt::Control { consents, .. } = &mut t { consents.push(con); }
        let mut plan = Plan::empty("plant consent");
        plan.gce = Some(t);
        self.run_commit(cid, plan).map(|_| ())
    }
    /// Put this signed style in the table whoever signed it, as a hostile admin would (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_plant_style(&mut self, cid: &str, s: Option<SignedStyle>) -> Result<()> {
        let mut t = self.ctl_ext(cid)?;
        if let GroupExt::Control { style, .. } = &mut t { *style = s; }
        let mut plan = Plan::empty("plant style");
        plan.gce = Some(t);
        self.run_commit(cid, plan).map(|_| ())
    }
    /// Send any envelope as this device, as a hostile member would (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_send_envelope(&mut self, gid: &str, env: Envelope) -> Result<()> { self.queue_env(gid, env); self.save() }
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_account_key(&self) -> SigningKey { self.acct_key().expect("identity") }
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_my_grant(&self, cid: &str) -> Option<String> { self.my_grant(cid) }
    /// The device key, so a test can talk to the relay as a misbehaving client (test builds only).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn debug_dev_key(&self) -> Result<SigningKey> { self.dev_key() }
    #[doc(hidden)]
    pub fn debug_expire_acl_jobs(&mut self) { for j in self.st.acl_jobs.iter_mut() { j.after = 0; } }
    #[doc(hidden)]
    pub fn outbox_len(&self) -> usize { self.st.outbox.len() }

    #[allow(dead_code)]
    fn _batch() -> usize { BATCH }
}

/// A file on its way out: encrypted, not yet uploaded.
pub struct OutFile { gid: String, env: Envelope, ct: Vec<u8> }
impl OutFile {
    /// The only slow part. Takes no reference to the client, so it can run with the client free for others.
    pub fn upload(&self, api: &Api, k: &SigningKey) -> Result<()> {
        let Envelope::File { blob, .. } = &self.env else { bail!("not a file") };
        api.put_blob(k, blob, &self.ct)
    }
}
/// A profile picture on its way out: encrypted, not yet uploaded.
pub struct OutAvatar { r: AvatarRef, ct: Vec<u8> }
impl OutAvatar {
    pub fn upload(&self, api: &Api, k: &SigningKey) -> Result<()> { api.put_blob(k, &self.r.blob, &self.ct) }
    pub fn into_ref(self) -> AvatarRef { self.r }
}
/// A profile picture we want to show.
pub struct InAvatar { pub avatar: AvatarRef, pub slot: String, pub cached: Option<Vec<u8>> }
impl InAvatar {
    /// Decrypted bytes, downloading them if they are not cached. No reference to the client, as above.
    pub fn bytes(&mut self, api: &Api, k: &SigningKey) -> Result<Vec<u8>> {
        let plain = if let Some(p) = self.cached.take() { p } else {
            let ct = api.get_blob(k, &self.avatar.blob)?;
            ChaCha20Poly1305::new((&hex32(&self.avatar.key)?).into()).decrypt(Nonce::from_slice(&hex::decode(&self.avatar.nonce)?), ct.as_slice())
                .map_err(|_| anyhow!("the profile picture is corrupt or was tampered with on the relay (authentication failed)"))?
        };
        // The size in the reference is checked, but the relay's copy is only trusted once it is open: a sender could
        // point at a much larger blob of their own.
        if plain.len() as u64 != self.avatar.size || plain.len() as u64 > MAX_AVATAR { bail!("the profile picture is not the size it claims"); }
        Ok(plain)
    }
}
/// An attachment we want to read.
pub struct InFile { pub name: String, pub mime: String, blob: String, key: String, nonce: String, pub slot: String, pub cached: Option<Vec<u8>> }
impl InFile {
    /// Decrypted bytes, downloading them if they are not cached. No reference to the client, as above.
    pub fn bytes(&mut self, api: &Api, k: &SigningKey) -> Result<Vec<u8>> {
        if let Some(p) = self.cached.take() { return Ok(p); }
        let ct = api.get_blob(k, &self.blob)?;
        ChaCha20Poly1305::new((&hex32(&self.key)?).into()).decrypt(Nonce::from_slice(&hex::decode(&self.nonce)?), ct.as_slice())
            .map_err(|_| anyhow!("the attachment is corrupt or was tampered with on the relay (authentication failed)"))
    }
}

struct Joiner { cert: DeviceCert, invite: String, seq: u64, consent: Option<Consent> }
enum JoinCheck { Accept(DeviceCert, String, Option<Consent>), Stale, Junk(String), Refuse(String, &'static str) }

/// Six characters of the account id: enough to tell two people apart, and the same on every screen.
pub fn account_tag(a: &str) -> String { a.chars().take(6).collect() }
