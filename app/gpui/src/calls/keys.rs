//! Frame keys from the room's MLS group, applied the way call.js applies them (CALLS-PLAN.md section 2).
//!
//! The core's `call_state` gives one 32-byte key per device for the current epoch. Each is set at index `epoch % 16`.
//! Two settings the SDK does not default to are what make a native client decode the web's frames and the other way
//! round: HKDF as the derivation (the SDK's default is PBKDF2) and the sending index, which the SDK leaves at 0
//! unless told (patch L2 and [`KeyBook::point_senders`]).
use base64::Engine;
use livekit::e2ee::key_provider::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use livekit::e2ee::manager::E2eeManager;
use livekit::prelude::ParticipantIdentity;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// LiveKit's key ring size; the key index of an epoch is `epoch % 16`.
pub const KEY_RING: u64 = 16;
/// How long the previous epoch's keys stay usable for frames in flight.
pub const OLD_KEY_GRACE: Duration = Duration::from_millis(3000);

pub fn index_of(epoch: u64) -> i32 { (epoch % KEY_RING) as i32 }

/// The web client's `BaseKeyProvider({ sharedKey: false, ratchetWindowSize: 0, failureTolerance: -1 })`, with the key
/// imported for HKDF as `createKeyMaterialFromBuffer` does.
pub fn options(kdf: KeyDerivationAlgorithm) -> KeyProviderOptions {
    KeyProviderOptions { ratchet_window_size: 0, failure_tolerance: -1, key_ring_size: KEY_RING as i32, key_derivation_algorithm: kdf, ..Default::default() }
}
pub fn provider() -> KeyProvider { KeyProvider::new(options(KeyDerivationAlgorithm::HKDF)) }

/// The core sends standard base64 with padding; anything that does not decode to 32 bytes is refused.
pub fn decode(b64: &str) -> Option<Vec<u8>> {
    let t = b64.trim().trim_end_matches('=').replace('+', "-").replace('/', "_");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t).ok().filter(|k| k.len() == 32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member { pub device: String, pub account: String, pub name: String, pub me: bool }

/// `call_state` as the core returns it: `{epoch, me, keys: {device: base64}, members: [{device, account, name, me}]}`.
#[derive(Debug, Clone)]
pub struct CallState { pub epoch: u64, pub me: String, pub keys: BTreeMap<String, Vec<u8>>, pub members: Vec<Member> }

impl CallState {
    pub fn parse(v: &serde_json::Value) -> Result<CallState, String> {
        let epoch = v["epoch"].as_u64().ok_or("call_state: no epoch")?;
        let me = v["me"].as_str().ok_or("call_state: no device")?.to_string();
        let mut keys = BTreeMap::new();
        for (d, k) in v["keys"].as_object().ok_or("call_state: no keys")? {
            keys.insert(d.clone(), k.as_str().and_then(decode).ok_or("call_state: a key that is not 32 bytes")?);
        }
        let members = v["members"].as_array().map(|a| a.iter().map(|m| Member {
            device: m["device"].as_str().unwrap_or_default().into(), account: m["account"].as_str().unwrap_or_default().into(),
            name: m["name"].as_str().unwrap_or_default().into(), me: m["me"].as_bool().unwrap_or(false),
        }).collect()).unwrap_or_default();
        if !keys.contains_key(&me) { return Err("call_state: no key for this device".into()); }
        Ok(CallState { epoch, me, keys, members })
    }
}

/// What `apply` did.
#[derive(Debug, Default, PartialEq)]
pub struct Applied { pub rotated: bool, pub first: bool }

struct Wipe { due: Instant, index: i32, devices: Vec<String>, rotation: u32 }

/// The keys of one room session. A fresh session (join or rebuild) gets a fresh book and key provider.
pub struct KeyBook {
    pub kp: KeyProvider,
    pub me: String,
    pub epoch: Option<u64>,
    pub index: i32,
    pub rotations: u32,
    pub last_rotation: Option<Instant>,
    current: BTreeMap<String, Vec<u8>>,
    /// Every device ever keyed in this session, including ones that have since left: their old slot must die too.
    keyed: BTreeSet<String>,
    wipe: Option<Wipe>,
    pub wipes_done: u32,
}

impl KeyBook {
    pub fn new(kp: KeyProvider) -> KeyBook {
        KeyBook { kp, me: String::new(), epoch: None, index: 0, rotations: 0, last_rotation: None, current: BTreeMap::new(), keyed: BTreeSet::new(), wipe: None, wipes_done: 0 }
    }

    pub fn devices(&self) -> usize { self.current.len() }

    /// Set every member's key at the epoch's index. On a new epoch the old slot is wiped after [`OLD_KEY_GRACE`]
    /// (call [`KeyBook::tick`]). Natively, unlike the web, setting a key does not change what a sender uses, so the
    /// caller then runs [`KeyBook::point_senders`].
    pub fn apply(&mut self, st: &CallState, now: Instant) -> Applied {
        let idx = index_of(st.epoch);
        let prev = self.epoch;
        self.me = st.me.clone();
        for (d, k) in &st.keys {
            self.kp.set_key(&ParticipantIdentity(d.clone()), idx, k.clone());
            self.keyed.insert(d.clone());
        }
        self.current = st.keys.clone();
        let mut out = Applied { first: prev.is_none(), ..Default::default() };
        if let Some(p) = prev.filter(|p| *p != st.epoch) {
            self.rotations += 1;
            self.last_rotation = Some(now);
            out.rotated = true;
            let old = index_of(p);
            if old != idx {
                // Our own old slot too (rule 5): setting a key does not move a native sender, so this cannot silence us,
                // and a cryptor that wrongly stayed on the old index then sends nothing instead of using a stale key.
                self.wipe = Some(Wipe { due: now + OLD_KEY_GRACE, index: old, devices: self.keyed.iter().cloned().collect(), rotation: self.rotations });
            }
        }
        self.epoch = Some(st.epoch);
        self.index = idx;
        // `set_key` moved the provider's "latest" index to whatever it wrote; new sender cryptors (patch L2) start there.
        self.kp.set_latest_key_index(idx);
        out
    }

    /// Install a participant's current key again (when it appears, or one of its tracks is subscribed), as `_rekey` does.
    pub fn rekey(&self, identity: &str) {
        if let Some(k) = self.current.get(identity) {
            self.kp.set_key(&ParticipantIdentity(identity.to_string()), self.index, k.clone());
            self.kp.set_latest_key_index(self.index);
        }
    }

    /// Runs a wipe whose grace has passed: the old slot of every device ever keyed gets random bytes, then the current
    /// keys are set again last. True when it wiped.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(w) = self.wipe.as_ref() else { return false };
        if now < w.due { return false; }
        let w = self.wipe.take().unwrap();
        if w.rotation != self.rotations || w.index == self.index { return false; }
        for d in &w.devices {
            let mut junk = vec![0u8; 32];
            getrandom::getrandom(&mut junk).expect("random bytes");
            self.kp.set_key(&ParticipantIdentity(d.clone()), w.index, junk);
        }
        for (d, k) in &self.current { self.kp.set_key(&ParticipantIdentity(d.clone()), self.index, k.clone()); }
        self.kp.set_latest_key_index(self.index);
        self.wipes_done += 1;
        true
    }

    /// Rule 3: every sender cryptor of ours uses the current index. Returns how many were moved.
    pub fn point_senders(&self, e2ee: &E2eeManager) -> usize {
        let mut n = 0;
        for ((id, _), fc) in e2ee.frame_cryptors() {
            if id.as_str() == self.me && fc.key_index() != self.index { fc.set_key_index(self.index); n += 1; }
        }
        n
    }

    /// For checks: does the provider hold exactly `key` for `identity` at `index`? Nothing is printed.
    pub fn holds(&self, identity: &str, index: i32, key: &[u8]) -> bool {
        self.kp.get_key(&ParticipantIdentity(identity.to_string()), index).is_some_and(|k| k == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn st(epoch: u64, devs: &[(&str, u8)]) -> CallState {
        CallState { epoch, me: devs[0].0.into(), keys: devs.iter().map(|(d, b)| (d.to_string(), vec![*b; 32])).collect(), members: vec![] }
    }

    #[test]
    fn decode_accepts_standard_and_url_base64_of_32_bytes_only() {
        let k = [7u8; 32];
        assert_eq!(decode(&base64::engine::general_purpose::STANDARD.encode(k)).unwrap(), k);
        assert_eq!(decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(k)).unwrap(), k);
        assert!(decode(&base64::engine::general_purpose::STANDARD.encode([1u8; 16])).is_none());
        assert!(decode("not base64 at all!").is_none());
    }
    #[test]
    fn index_is_epoch_mod_16() { assert_eq!(index_of(1), 1); assert_eq!(index_of(17), 1); assert_eq!(index_of(16), 0); }

    #[test]
    fn parse_refuses_a_state_without_our_key() {
        let v = serde_json::json!({"epoch": 3, "me": "a", "keys": {"b": base64::engine::general_purpose::STANDARD.encode([1u8; 32])}, "members": []});
        assert!(CallState::parse(&v).is_err());
    }

    #[test]
    fn rotation_keeps_the_old_slot_for_the_grace_then_wipes_it_and_keeps_the_new() {
        let mut b = KeyBook::new(provider());
        let t0 = Instant::now();
        assert!(b.apply(&st(1, &[("me", 1), ("bob", 2), ("carol", 3)]), t0).first);
        assert_eq!(b.kp.get_latest_key_index(), 1);
        let r = b.apply(&st(2, &[("me", 4), ("bob", 5)]), t0);
        assert!(r.rotated && b.rotations == 1 && b.index == 2);
        assert!(b.holds("bob", 1, &[2; 32]), "old key still there during the grace");
        assert!(!b.tick(t0 + Duration::from_millis(2900)));
        assert!(b.tick(t0 + OLD_KEY_GRACE));
        for (d, k) in [("me", 1u8), ("bob", 2), ("carol", 3)] { assert!(!b.holds(d, 1, &[k; 32]), "{d}'s old slot wiped"); }
        assert!(b.holds("me", 2, &[4; 32]) && b.holds("bob", 2, &[5; 32]));
        assert_eq!(b.kp.get_latest_key_index(), 2, "senders still pointed at the new index");
        assert!(!b.tick(t0 + Duration::from_secs(10)), "a wipe runs once");
    }

    #[test]
    fn a_second_rotation_inside_the_grace_cancels_the_first_wipe() {
        let mut b = KeyBook::new(provider());
        let t0 = Instant::now();
        b.apply(&st(1, &[("me", 1)]), t0);
        b.apply(&st(2, &[("me", 2)]), t0);
        b.apply(&st(3, &[("me", 3)]), t0 + Duration::from_secs(1));
        assert!(b.tick(t0 + Duration::from_secs(5)));
        assert!(!b.holds("me", 2, &[2; 32]), "the slot of epoch 2 is the one wiped");
        assert!(b.holds("me", 3, &[3; 32]));
    }

    #[test]
    fn a_member_change_at_the_same_epoch_is_not_a_rotation() {
        let mut b = KeyBook::new(provider());
        let t0 = Instant::now();
        b.apply(&st(4, &[("me", 1)]), t0);
        assert_eq!(b.apply(&st(4, &[("me", 1), ("bob", 2)]), t0), Applied::default());
        assert!(!b.tick(t0 + Duration::from_secs(5)));
    }
}
