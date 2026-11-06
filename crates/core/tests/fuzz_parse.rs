//! Cheap fuzzing of every parser that touches bytes from the network or from other members.
//! Deterministic (seeded); run `FUZZ_ITERS=2000000 cargo test --test fuzz_parse --release` for a longer soak.
use commons_core::envelope::*;
use commons_core::vault::Vault;
use commons_proto::*;
use rand::{Rng, RngCore, SeedableRng};

fn mutate(rng: &mut rand::rngs::StdRng, base: &[u8]) -> Vec<u8> {
    let mut v = base.to_vec();
    match rng.gen_range(0..5) {
        0 if !v.is_empty() => { let i = rng.gen_range(0..v.len()); v[i] ^= 1 << rng.gen_range(0..8); }
        1 if !v.is_empty() => { let i = rng.gen_range(0..v.len()); v.truncate(i); }
        2 => { let i = rng.gen_range(0..=v.len()); let n = rng.gen_range(1..8); for _ in 0..n { v.insert(i, rng.gen()); } }
        3 if !v.is_empty() => { let i = rng.gen_range(0..v.len()); v.remove(i); }
        _ => { let n = rng.gen_range(0..300); v = (0..n).map(|_| rng.gen()).collect(); }
    }
    v
}

#[test]
fn parsers_never_panic() {
    let iters: usize = std::env::var("FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(30_000);
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xC0FFEE);
    let id = "0123456789abcdef0123456789abcdef".to_string();
    let seeds: Vec<Vec<u8>> = vec![
        Envelope::Text { id: id.clone(), text: "hi".into(), reply: Some(id.clone()), ts: 1 }.to_bytes(),
        Envelope::File { id: id.clone(), name: "a".into(), mime: "b".into(), size: 3, blob: id.clone(), key: "00".repeat(32), nonce: "00".repeat(12), caption: "".into(), reply: None, ts: 1 }.to_bytes(),
        Envelope::React { target: id.clone(), emoji: "👍".into(), on: true }.to_bytes(),
        Envelope::Revoked { account: "a".repeat(64), device: "b".repeat(64), sig: "c".repeat(128) }.to_bytes(),
        GroupExt::Channel { community: "abcdef0123".into(), name: "x".into(), owner: "a".repeat(64), nonce: id.clone(), admins: vec![AdminGrant { account: "b".repeat(64), sig: "c".repeat(128) }] }.to_bytes(),
        GroupExt::Control { name: "n".into(), owner: "a".repeat(64), nonce: id.clone(), admins: vec![], channels: vec![ChanMeta { id: "abcdef0123".into(), name: "c".into() }], invites: vec![], banned: vec![] }.to_bytes(),
        br#"{"t":"Text","id":"x","text":"\u0000","ts":-1}"#.to_vec(),
        InviteLink { owner: "a".repeat(64), relay: "http://127.0.0.1:1".into(), community: "abcdef0123".into(), invite: id.clone(), sk: "b".repeat(64), name: "n".into() }.encode().into_bytes(),
    ];
    let mut ok = 0u64;
    for i in 0..iters {
        let data = if i % 4 == 0 { let mut b = vec![0u8; rng.gen_range(0..2000)]; rng.fill_bytes(&mut b); b } else { mutate(&mut rng, &seeds[i % seeds.len()]) };
        if Envelope::parse(&data).is_ok() { ok += 1; }
        let _ = GroupExt::parse(&data);
        let _ = JoinRequest::parse(&data);
        let _ = DeviceCert::from_bytes(&data);
        let s = String::from_utf8_lossy(&data).to_string();
        let _ = InviteLink::decode(&s);
        let _ = verify_request(&s, "POST", "/v1/sync", &data);
        let _ = unb64(&s);
        let _ = unhex32(&s);
    }
    assert!(ok > 0, "sanity: some mutations still parse");
}

#[test]
fn mls_wire_and_state_parsers_never_panic() {
    use openmls::prelude::{tls_codec::Deserialize, *};
    let iters: usize = std::env::var("FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(30_000) / 3;
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    for i in 0..iters {
        let mut b = vec![0u8; rng.gen_range(0..1500)];
        rng.fill_bytes(&mut b);
        if i % 3 == 0 && b.len() > 4 { b[0] = 0; b[1] = 1; } // plausible version prefix
        if let Ok(m) = MlsMessageIn::tls_deserialize_exact(&b) { let _ = m.try_into_protocol_message(); }
        let _ = KeyPackageIn::tls_deserialize_exact(&b);
        let _ = commons_core::mls::Prov::load(&b);
    }
    // a damaged or foreign state file is reported as an error, never a panic
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("state.cmns");
    for n in [0usize, 5, 40, 500] { let mut b = vec![0u8; n]; rng.fill_bytes(&mut b); std::fs::write(&p, b).unwrap(); assert!(Vault::open(&p, "x").is_err()); }
}
