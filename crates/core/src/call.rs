//! Voice and screen-share support: the media keys for a room's call come out of the room's MLS group.
//!
//! For every device in the group, the key for frames *sent by that device* is
//! `MLS-Exporter("commons-call-e2ee-v1", "<group id>|<device id>", 32)` of the current epoch (RFC 9420 section 8.5).
//! Every member derives the same keys; anybody who is not in the group at this epoch cannot. Each commit (any
//! membership change, any self-update) starts a new epoch, so the keys rotate exactly when text keys do.
//! Nothing is derived by hand: the exporter is OpenMLS, the frame cipher (AES-GCM) and its HKDF are the LiveKit
//! client's WebCrypto. See docs/DESIGN.md section 11.
use crate::client::Client;
use crate::mls;
use anyhow::{anyhow, bail, Result};
use commons_proto::CallTokenResp;
use openmls_traits::OpenMlsProvider;
use std::collections::BTreeMap;

pub const CALL_LABEL: &str = "commons-call-e2ee-v1";

#[derive(Debug, Clone)]
pub struct CallMember { pub device: String, pub account: String, pub name: String, pub is_me: bool }
#[derive(Debug, Clone)]
pub struct CallState {
    pub epoch: u64,
    pub my_device: String,
    pub members: Vec<CallMember>,
    /// per sending device: the 32-byte frame key for this epoch
    pub keys: BTreeMap<String, [u8; 32]>,
}

impl Client {
    /// The call keys of this room at the epoch this device is at. Fails if this device is no longer in the room.
    pub fn call_state(&self, gid: &str) -> Result<CallState> {
        match self.st.groups.get(gid) {
            None => bail!("this room has not finished joining yet"),
            Some(g) if g.removed => bail!("you are no longer a member of this room"),
            Some(g) if g.broken => bail!("this room's keys are out of sync; waiting to be re-added"),
            _ => {}
        }
        let g = self.load_group(gid).ok_or_else(|| anyhow!("no keys for this room"))?;
        if !g.is_active() { bail!("you are no longer a member of this room"); }
        let me = self.my_device();
        let mut members = vec![];
        let mut keys = BTreeMap::new();
        for (_, c) in mls::members_of(&g) {
            let ctx = format!("{gid}|{}", c.device);
            let k = g.export_secret(self.prov.crypto(), CALL_LABEL, ctx.as_bytes(), 32).map_err(|e| anyhow!("key export: {e:?}"))?;
            let k: [u8; 32] = k.try_into().map_err(|_| anyhow!("key length"))?;
            keys.insert(c.device.clone(), k);
            members.push(CallMember { is_me: c.device == me, name: self.display_name(&c.account), account: c.account, device: c.device });
        }
        if !keys.contains_key(&me) { bail!("this device is not in the room's key group"); }
        Ok(CallState { epoch: g.epoch().as_u64(), my_device: me, members, keys })
    }

    /// A join token from the relay (it checks that the account is a current member).
    pub fn call_token(&self, gid: &str) -> Result<CallTokenResp> {
        if !self.st.groups.get(gid).is_some_and(|g| !g.removed && !g.broken) { bail!("you are not in this room"); }
        let k = self.dev_key()?;
        self.api.call_token(&k, gid)
    }
}
