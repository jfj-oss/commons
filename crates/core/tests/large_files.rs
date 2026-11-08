//! Large files: chunked encryption, resume, quotas and expiry, all through a real in-process relay.
mod common;
use common::*;
use commons_core::client::rand_hex;
use commons_core::envelope::LargeFile;
use commons_core::state::MsgBody;
use commons_core::transport::relay_status;
use commons_proto::*;
use std::path::{Path, PathBuf};

fn content(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    let mut x = 0x9e3779b97f4a7c15u64;
    for b in v.iter_mut() {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        *b = (x >> 24) as u8;
    }
    v
}

fn begin(p: &mut Peer, gid: &str, name: &str, size: u64, lm: u64) -> (String, u64, u64, Vec<u64>) {
    let u = p.c.large_upload_begin(gid, name, "application/octet-stream", size, lm, "").unwrap();
    let (api, k) = p.c.net_handle().unwrap();
    let r = api.large_begin(&k, &u.id, &LargeBeginReq { gid: gid.into(), size, chunk_size: u.chunk_size }).unwrap();
    let u = p.c.large_upload_started(&u.id, &r).unwrap();
    (u.id, u.chunk_size, u.chunks, u.uploaded)
}

/// Send the named chunks, skipping the ones the relay says it has. Returns how many actually went over the wire.
fn send(p: &mut Peer, id: &str, data: &[u8], chunk_size: u64, chunks: u64) -> u64 {
    let (api, k) = p.c.net_handle().unwrap();
    let mut puts = 0;
    for i in 0..chunks {
        let at = (i * chunk_size) as usize;
        let end = ((i + 1) * chunk_size).min(data.len() as u64) as usize;
        let prep = p.c.large_chunk_prepare(id, i, &data[at..end]).unwrap();
        if prep.needed {
            api.large_put_chunk(&k, id, i, &prep.ct).unwrap(); puts += 1;
        }
        p.c.large_chunk_done(id, i);
    }
    puts
}

fn finish(p: &mut Peer, id: &str) -> String {
    let (blob, root) = p.c.large_finish_prepare(id).unwrap();
    let (api, k) = p.c.net_handle().unwrap();
    api.large_finish(&k, &blob, &root).unwrap();
    p.c.large_finish_done(id).unwrap()
}

fn large_message(p: &Peer, gid: &str) -> (String, LargeFile) {
    for m in p.c.messages(gid).into_iter().rev() {
        if let MsgBody::File { large: Some(l), .. } = &m.body { return (m.id.clone(), l.clone()); }
    }
    panic!("no large-file message from {}", p.name)
}

fn download(p: &mut Peer, gid: &str, mid: &str, dir: &Path) -> PathBuf {
    p.c.set_download_dir(dir.to_str().unwrap()).unwrap();
    let mut d = p.c.large_download_begin(gid, mid).unwrap();
    assert!(!d.finished, "a fresh download cannot already be finished");
    let (api, k) = p.c.net_handle().unwrap();
    while d.done < d.chunks {
        let ct = api.large_get_chunk(&k, &d.blob, d.done).unwrap();
        let step = p.c.large_download_write(gid, mid, &ct).unwrap();
        d.done = step.done;
        if step.finished { return PathBuf::from(step.path); }
    }
    panic!("download ended without the whole-file check")
}

#[test]
fn a_large_file_resumes_after_a_restart_and_arrives_intact() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    let size = 40 * 1024 * 1024 + 12345; // five chunks, the last one short
    let data = content(size as usize);
    let (id, cs, chunks, uploaded) = begin(&mut alice, &gid, "a big film.mkv", size as u64, 7);
    assert_eq!(chunks, LargeFile::chunk_count(size as u64, cs));
    assert!(uploaded.is_empty());
    // Two chunks go across, then the app closes.
    assert_eq!(send(&mut alice, &id, &data, cs, 2), 2);
    alice.reopen(&net);

    let (id2, cs2, chunks2, uploaded2) = begin(&mut alice, &gid, "a big film.mkv", size as u64, 7);
    assert_eq!((id2.as_str(), cs2, chunks2), (id.as_str(), cs, chunks));
    assert_eq!(uploaded2, vec![0, 1], "the relay must remember what already arrived");
    // The UI re-reads every chunk (the whole-file hash needs each one), but only the missing three are uploaded.
    assert_eq!(send(&mut alice, &id, &data, cs, chunks), chunks - 2);
    let mid = finish(&mut alice, &id);
    settle(&mut [&mut alice, &mut bob]);

    let (got_id, got) = large_message(&bob, &gid);
    assert_eq!(got_id, mid);
    assert_eq!(got.size, size as u64);
    assert_eq!(got.chunks, chunks);
    assert_eq!(got.chunk_size, cs);
    assert!(got.expires > now_secs());

    let dir = tempfile::tempdir().unwrap();
    let path = download(&mut bob, &gid, &mid, dir.path());
    assert_eq!(path.file_name().unwrap(), "a big film.mkv");
    assert_eq!(std::fs::read(&path).unwrap(), data, "the downloaded bytes must be the original bytes");

    // Pressing Download again does not fetch the file a second time.
    let again = bob.c.large_download_begin(&gid, &mid).unwrap();
    assert!(again.finished);
    assert_eq!(PathBuf::from(again.path), path);
}

#[test]
fn a_changed_file_does_not_resume_an_old_upload() {
    // Same name and size, different modification time: these are not the same bytes, so they must not share the
    // relay copy (or the whole-file hash would describe one file while the chunks held another).
    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);
    let size = LARGE_FILE_THRESHOLD + 1;
    let (first, _, _, _) = begin(&mut alice, &gid, "notes.bin", size, 7);
    let (second, _, _, uploaded) = begin(&mut alice, &gid, "notes.bin", size, 8);
    assert_ne!(first, second);
    assert!(uploaded.is_empty(), "a changed file must not inherit the relay's chunks");
}

fn direct_begin(p: &Peer, gid: &str, size: u64) -> anyhow::Result<(String, LargeInfoResp)> {
    let (api, k) = p.c.net_handle()?;
    let id = rand_hex(16);
    let r = api.large_begin(&k, &id, &LargeBeginReq { gid: gid.into(), size, chunk_size: LARGE_CHUNK_BYTES })?;
    Ok((id, r))
}

#[test]
fn the_owner_and_member_quotas_are_different_and_only_the_uploader_can_delete() {
    let net = Net::with_limits(commons_relay::Limits {
        large_file_quota_per_account: 10 << 20,
        large_file_quota_owner: 20 << 20,
        large_file_total: 1 << 30,
        ..Default::default()
    });
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    // The owner may hold 15 MB; a member with the 10 MB limit may not.
    let (id, _) = direct_begin(&alice, &gid, 15 << 20).unwrap();
    match direct_begin(&bob, &gid, 15 << 20) {
        Ok(_) => panic!("a member over the member quota was allowed"),
        Err(e) => assert_eq!(relay_status(&e), Some(507), "{e}"),
    }

    // Bob cannot remove Alice's upload; Alice can, and after that the relay has nothing.
    let (api_b, k_b) = bob.c.net_handle().unwrap();
    assert_eq!(relay_status(&api_b.large_delete(&k_b, &id).unwrap_err()), Some(403));
    let (api, k) = alice.c.net_handle().unwrap();
    api.large_delete(&k, &id).unwrap();
    assert_eq!(relay_status(&api.large_info(&k, &id).unwrap_err()), Some(404));
}

#[test]
fn the_relay_total_is_a_ceiling() {
    let net = Net::with_limits(commons_relay::Limits {
        large_file_quota_per_account: 1 << 30,
        large_file_quota_owner: 1 << 30,
        large_file_total: 20 << 20,
        ..Default::default()
    });
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    let (id, _) = direct_begin(&alice, &gid, 15 << 20).unwrap();
    match direct_begin(&bob, &gid, 10 << 20) {
        Ok(_) => panic!("the relay went over its total"),
        Err(e) => assert_eq!(relay_status(&e), Some(507), "{e}"),
    }
    let (api, k) = alice.c.net_handle().unwrap();
    api.large_delete(&k, &id).unwrap();
}

#[test]
fn a_large_file_expires_and_its_bytes_leave_the_disk() {
    let net = Net::with_limits(commons_relay::Limits { large_file_expire_secs: 1, ..Default::default() });
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    let (id, _) = direct_begin(&alice, &gid, 25 << 20).unwrap();
    let dir = net.data.path().join("relay").join("large").join(&id);
    assert!(dir.is_dir(), "the upload has its own directory");
    std::thread::sleep(std::time::Duration::from_millis(1200));

    let (api, k) = alice.c.net_handle().unwrap();
    assert_eq!(relay_status(&api.large_info(&k, &id).unwrap_err()), Some(404), "an expired file is gone");
    // A new reservation sweeps expired rows and their bytes before it counts storage.
    let (_new, _) = direct_begin(&alice, &gid, 25 << 20).unwrap();
    assert!(!dir.exists(), "the expired file's chunks must be removed");
}

#[test]
fn a_short_chunk_is_refused_by_the_relay() {
    // The relay knows the exact ciphertext size of every chunk from the announcement, so junk never lands.
    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);
    let size = (LARGE_FILE_THRESHOLD + 1) as u64;
    let (id, _) = direct_begin(&alice, &gid, size).unwrap();
    let (api, k) = alice.c.net_handle().unwrap();
    let e = api.large_put_chunk(&k, &id, 0, &[0u8; 100]).unwrap_err();
    assert_eq!(relay_status(&e), Some(400), "{e}");
    let (api2, k2) = alice.c.net_handle().unwrap();
    api2.large_delete(&k2, &id).unwrap();
}

#[test]
fn a_download_restarts_at_the_last_verified_chunk_after_a_restart() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    let size = (LARGE_FILE_THRESHOLD + 2 * (1 << 20)) as usize;
    let data = content(size);
    let (id, cs, chunks, _) = begin(&mut alice, &gid, "part.bin", size as u64, 1);
    send(&mut alice, &id, &data, cs, chunks);
    let mid = finish(&mut alice, &id);
    settle(&mut [&mut alice, &mut bob]);

    let dir = tempfile::tempdir().unwrap();
    bob.c.set_download_dir(dir.path().to_str().unwrap()).unwrap();
    let mut d = bob.c.large_download_begin(&gid, &mid).unwrap();
    let (api, k) = bob.c.net_handle().unwrap();
    // Two chunks, then the app closes: the partial file and its verified count survive.
    for _ in 0..2 {
        let ct = api.large_get_chunk(&k, &d.blob, d.done).unwrap();
        let s = bob.c.large_download_write(&gid, &mid, &ct).unwrap();
        d.done = s.done;
    }
    assert_eq!(d.done, 2);
    bob.reopen(&net);
    let d = bob.c.large_download_begin(&gid, &mid).unwrap();
    assert_eq!((d.done, d.chunks), (2, chunks), "the saved position is where it stopped");
    let path = download(&mut bob, &gid, &mid, dir.path());
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[test]
fn cancelling_an_upload_removes_the_relay_copy() {
    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);
    let size = (LARGE_FILE_THRESHOLD + 1000) as u64;
    let (id, _, _, _) = begin(&mut alice, &gid, "cancel.bin", size, 3);
    let blob = alice.c.large_upload_cancel(&id).unwrap().unwrap();
    assert_eq!(blob, id);
    let (api, k) = alice.c.net_handle().unwrap();
    api.large_delete(&k, &blob).unwrap();
    assert_eq!(relay_status(&api.large_info(&k, &blob).unwrap_err()), Some(404));
}
