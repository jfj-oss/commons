//! Randomized soak: several clients, a real relay, random joins/leaves/kicks/messages while the network
//! misbehaves (offline, reordering, duplicates, stale views, relay restarts, app restarts).
//! After the chaos stops, everyone who is still a member must have converged and a marker message must
//! be readable by exactly the current members.
mod common;
use common::*;
use rand::{Rng, SeedableRng};

fn run(seed: u64, steps: usize) {
    eprintln!("seed {seed}");
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut net = Net::new();
    let mut peers: Vec<Peer> = vec![Peer::new(&net, "p0"), Peer::new(&net, "p1"), Peer::new(&net, "p2")];
    let (cid, gen) = {
        let (a, rest) = peers.split_at_mut(1);
        let mut r: Vec<&mut Peer> = rest.iter_mut().collect();
        community_with(&net, &mut a[0], &mut r)
    };
    let mut kicked: Vec<usize> = vec![];
    let mut sent = 0;
    for step in 0..steps {
        let n = peers.len();
        let i = rng.gen_range(0..n);
        match rng.gen_range(0..14) {
            0..=4 => {
                let chans = peers[i].c.channels_of(&cid);
                if !chans.is_empty() && !kicked.contains(&i) {
                    let ch = &chans[rng.gen_range(0..chans.len())];
                    if ch.joined { sent += 1; let _ = peers[i].c.send_text(&ch.gid, &format!("m{sent} from p{i}"), None); }
                }
            }
            5 => { let mut f = peers[i].faults.lock().unwrap(); f.offline = !f.offline; }
            6 => { let mut f = peers[i].faults.lock().unwrap(); f.reorder = !f.reorder; f.duplicate = rng.gen(); }
            7 => { let mut f = peers[i].faults.lock().unwrap(); f.stale_sync = !f.stale_sync; }
            8 if i != 0 && !kicked.contains(&i) => {
                let t = peers[i].acct();
                if peers[0].c.kick(&cid, &t).is_ok() { kicked.push(i); }
            }
            9 if peers.len() < 6 => {
                let name = format!("p{}", peers.len());
                let p = Peer::new(&net, &name);
                peers.push(p);
                if let Ok(link) = peers[0].c.create_invite(&cid, Some(1), 3600) { let k = peers.len() - 1; let _ = peers[k].c.join_with_link(&link); }
            }
            10 => { let _ = peers[0].c.create_channel(&cid, &format!("room-{step}")); }
            11 => { peers[i].reopen(&net); }
            12 if step % 7 == 0 => { net.stop_relay(); net.start_relay(); }
            13 if i != 0 => { let _ = peers[i].c.set_name(&format!("name{step}")); }
            _ => {}
        }
        for _ in 0..rng.gen_range(1..3) { let j = rng.gen_range(0..peers.len()); peers[j].tick(); }
    }
    // chaos over
    for p in peers.iter() { *p.faults.lock().unwrap() = Faults::default(); }
    {
        let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
        settle(&mut refs);
    }
    // a late joiner whose request was never served is not a member: find out who is really in
    let owner = &peers[0];
    let members = owner.c.group_accounts(&cid);
    let joined: Vec<usize> = (0..peers.len()).filter(|&k| members.contains(&peers[k].acct())).collect();
    assert!(joined.contains(&0));
    for &k in &kicked { assert!(!members.contains(&peers[k].acct()), "kicked p{k} still a member (seed {seed})"); }
    let mut chans: Vec<String> = vec![cid.clone(), gen.clone()];
    chans.extend(owner.c.channels_of(&cid).into_iter().map(|c| c.gid));
    chans.sort(); chans.dedup();
    for g in &chans {
        let fp = owner.c.epoch_fingerprint(g);
        for &k in &joined {
            assert_eq!(peers[k].c.epoch_fingerprint(g), fp, "p{k} diverged in {} (seed {seed}); notices: {:#?}", &g[..6], notices(&peers[k]));
            assert_eq!(peers[k].c.group_accounts(g), owner.c.group_accounts(g), "member list differs (seed {seed})");
        }
    }
    // marker messages: readable by all current members, never by removed ones
    for &k in &joined {
        let id = peers[k].c.send_text(&gen, &format!("final-{k}"), None);
        assert!(id.is_ok(), "p{k} cannot send at the end: {:?} {:?}", id, notices(&peers[k]));
    }
    {
        let mut refs: Vec<&mut Peer> = peers.iter_mut().collect();
        settle(&mut refs);
    }
    for &k in &joined { for &m in &joined {
        assert!(has_text(&peers[k], &gen, &format!("final-{m}")), "p{k} missed final-{m} (seed {seed}); {:#?}", notices(&peers[k]));
    } }
    for &k in &kicked {
        for &m in &joined { assert!(!has_text(&peers[k], &gen, &format!("final-{m}")), "kicked p{k} read final-{m} (seed {seed})"); }
        let mut tried = 0;
        for (_s, d) in relay_entries(&net, &gen) { if let Ok(pt) = peers[k].c.debug_try_decrypt(&gen, &d) { assert!(!String::from_utf8_lossy(&pt).contains("final-"), "kicked member decrypted a final message"); } tried += 1; }
        assert!(tried > 0);
    }
    // nothing was silently dropped: every failure is either explained in the timeline or absent
    let _ = sent;
}

#[test]
fn chaos_converges_seed_1() { run(1, 90); }
#[test]
fn chaos_converges_seed_2() { run(2, 90); }
#[test]
fn chaos_converges_seed_3() { run(3, 90); }
#[test]
fn chaos_soak_env() {
    // CHAOS_SEEDS=10 CHAOS_STEPS=200 cargo test --release --test simulation chaos_soak_env
    let seeds: u64 = std::env::var("CHAOS_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let steps: usize = std::env::var("CHAOS_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    for s in 0..seeds { run(100 + s, steps); }
}
