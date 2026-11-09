//! The point of the chunked design: a file far larger than the memory limit moves through the client and the relay
//! without either process growing to hold it. Its own test binary, so the peak belongs to this run alone.
mod common;
use common::*;
use commons_proto::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const SIZE: u64 = 2 << 30; // 2 GiB

fn work_dir() -> tempfile::TempDir {
    let base = std::env::var_os("COMMONS_BIG_TEST_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("commons-big-").tempdir_in(&base).unwrap()
}

fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// A sparse file: 2 GiB long, with a marker written every chunk (so a mixed-up chunk is visible), and holes
/// everywhere else. Reading it still yields 2 GiB, but it costs almost no disk.
fn sparse_source(path: &PathBuf) -> std::fs::File {
    let mut f = std::fs::File::create(path).unwrap();
    f.set_len(SIZE).unwrap();
    let mut at = 0u64;
    while at < SIZE {
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(&at.to_le_bytes()).unwrap();
        at += LARGE_CHUNK_BYTES;
    }
    f.seek(SeekFrom::Start(0)).unwrap();
    f.sync_all().unwrap();
    drop(f);
    std::fs::File::open(path).unwrap()
}

#[test]
fn a_two_gigabyte_file_streams_through_without_holding_it() {
    let work = work_dir();
    let src = work.path().join("source.bin");
    let mut source = sparse_source(&src);

    let net = Net::new();
    let mut alice = Peer::new(&net, "Alice");
    let mut bob = Peer::new(&net, "Bob");
    let (_cid, gid) = community_with(&net, &mut alice, &mut [&mut bob]);

    // ---- upload one chunk at a time, hashing as we read ----
    let u0 = alice.c.large_upload_begin(&gid, "big.bin", "application/octet-stream", SIZE, 9, "").unwrap();
    let (api, k) = alice.c.net_handle().unwrap();
    let r = api.large_begin(&k, &u0.id, &LargeBeginReq { gid: gid.clone(), size: SIZE, chunk_size: u0.chunk_size }).unwrap();
    let u = alice.c.large_upload_started(&u0.id, &r).unwrap();
    let mut buf = vec![0u8; u.chunk_size as usize];
    let mut source_hash = Sha256::new();
    for i in 0..u.chunks {
        let at = i * u.chunk_size;
        let len = (SIZE - at).min(u.chunk_size) as usize;
        source.read_exact(&mut buf[..len]).unwrap();
        source_hash.update(&buf[..len]);
        let prep = alice.c.large_chunk_prepare(&u.id, i, &buf[..len]).unwrap();
        if prep.needed { api.large_put_chunk(&k, &u.id, i, &prep.ct).unwrap(); }
        alice.c.large_chunk_done(&u.id, i);
    }
    let source_hash = hex::encode(source_hash.finalize());
    drop(buf);

    let (blob, root) = alice.c.large_finish_prepare(&u.id).unwrap();
    api.large_finish(&k, &blob, &root).unwrap();
    let mid = alice.c.large_finish_done(&u.id).unwrap();
    settle(&mut [&mut alice, &mut bob]);

    // ---- download one chunk at a time, straight to disk ----
    let downloads = work.path().join("downloads");
    bob.c.set_download_dir(downloads.to_str().unwrap()).unwrap();
    let mut d = bob.c.large_download_begin(&gid, &mid).unwrap();
    let (api2, k2) = bob.c.net_handle().unwrap();
    while d.done < d.chunks {
        let ct = api2.large_get_chunk(&k2, &d.blob, d.done).unwrap();
        let step = bob.c.large_download_write(&gid, &mid, &ct).unwrap();
        d.done = step.done;
        if step.finished { break; }
    }
    let saved = bob.c.large_download_state(&mid).unwrap();
    assert!(saved.finished, "the download did not finish");
    assert_eq!(std::fs::metadata(&saved.path).unwrap().len(), SIZE);

    let mut got = std::fs::File::open(&saved.path).unwrap();
    let mut out_hash = Sha256::new();
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = got.read(&mut chunk).unwrap();
        if n == 0 { break; }
        out_hash.update(&chunk[..n]);
    }
    assert_eq!(hex::encode(out_hash.finalize()), source_hash, "the 2 GiB that came back must be the bytes that went in");
    drop(chunk);

    if let Some(kb) = peak_rss_kb() {
        eprintln!("peak process memory (VmHWM) after 2 GiB in each direction: {:.1} MiB", kb as f64 / 1024.0);
        assert!(kb < 200 * 1024, "the process grew to {:.1} MiB, over the 200 MiB limit", kb as f64 / 1024.0);
    }
    let _ = &src;
    // `work` removes the source, the relay's copy lives in the Net tempdir, and both go when the test ends.
}
