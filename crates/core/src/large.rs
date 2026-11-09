//! Files too large for one blob: encrypted chunk by chunk, uploaded chunk by chunk, and written to disk chunk by
//! chunk. Neither side ever holds the whole file.
//!
//! The `File` message carries a random 32-byte key and 12-byte nonce, as an ordinary attachment does. Each chunk's
//! key and nonce are derived from that key and the chunk index; the chunk index and the chunk's plaintext length are
//! authenticated as additional data. Reordering, dropping or swapping chunks therefore fails authentication, and the
//! whole-file root hash (in the message) catches a relay or sender that assembles a different set of well-formed
//! chunks. One 8 MB chunk is the most this code holds.
use crate::client::*;
use crate::envelope::*;
use crate::state::*;
use anyhow::{anyhow, bail, Result};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, ChaCha20Poly1305, Nonce};
use commons_proto::*;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Save the resume position and chunk hashes every this many chunks. A crash then costs at most this much
/// re-reading, and the relay itself remembers which chunks actually arrived.
const SAVE_EVERY: u64 = 8;

pub fn hash_chunk(plain: &[u8]) -> [u8; 32] { Sha256::digest(plain).into() }

pub fn chunk_key(file_key: &[u8; 32], index: u64) -> [u8; 32] {
    sha256(&[b"commons/large-file/key/v1", file_key, &index.to_be_bytes()])
}
pub fn chunk_nonce(file_key: &[u8; 32], index: u64) -> [u8; 12] {
    let h = sha256(&[b"commons/large-file/nonce/v1", file_key, &index.to_be_bytes()]);
    h[..12].try_into().unwrap()
}
/// Binds a chunk to its file, its place and its exact plaintext length.
pub fn chunk_aad(blob: &str, index: u64, len: u64) -> Vec<u8> {
    let mut a = b"commons/large-file/chunk/v1".to_vec();
    a.extend_from_slice(blob.as_bytes());
    a.push(0);
    a.extend_from_slice(&index.to_be_bytes());
    a.extend_from_slice(&len.to_be_bytes());
    a
}
pub fn encrypt_chunk(file_key: &[u8; 32], blob: &str, index: u64, plain: &[u8]) -> Result<Vec<u8>> {
    let aad = chunk_aad(blob, index, plain.len() as u64);
    ChaCha20Poly1305::new((&chunk_key(file_key, index)).into())
        .encrypt(Nonce::from_slice(&chunk_nonce(file_key, index)), Payload { msg: plain, aad: &aad })
        .map_err(|_| anyhow!("encrypt"))
}
pub fn decrypt_chunk(file_key: &[u8; 32], blob: &str, index: u64, plain_len: u64, ct: &[u8]) -> Result<Vec<u8>> {
    let aad = chunk_aad(blob, index, plain_len);
    ChaCha20Poly1305::new((&chunk_key(file_key, index)).into())
        .decrypt(Nonce::from_slice(&chunk_nonce(file_key, index)), Payload { msg: ct, aad: &aad })
        .map_err(|_| anyhow!("this part of the file is corrupt or was tampered with on the relay (authentication failed)"))
}
/// SHA-256 over a domain tag, the chunk count, the real size, and every chunk's plaintext SHA-256.
pub fn root_hash(chunks: u64, size: u64, hashes: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"commons/large-file/root/v1");
    h.update(chunks.to_be_bytes());
    h.update(size.to_be_bytes());
    for x in hashes { h.update(x); }
    h.finalize().into()
}

/// What the UI needs to drive one upload.
#[derive(Clone, Debug)]
pub struct UploadStart {
    pub id: String,
    pub chunk_size: u64,
    pub chunks: u64,
    pub uploaded: Vec<u64>,
    pub expires: u64,
    pub complete: bool,
    pub name: String,
}
pub struct PreparedChunk { pub ct: Vec<u8>, pub needed: bool }
#[derive(Clone, Debug)]
pub struct DownloadStep { pub done: u64, pub chunks: u64, pub size: u64, pub finished: bool, pub path: String }

fn key_of(s: &str) -> Result<[u8; 32]> { hex32(s) }
fn name_of(name: &str) -> String {
    let safe: String = name.chars().filter(|c| !c.is_control()).collect();
    let safe = cut_name(safe.trim(), 200);
    if safe.is_empty() { "file".into() } else { safe }
}
fn chunk_count(size: u64) -> u64 { LargeFile::chunk_count(size, LARGE_CHUNK_BYTES) }
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() { return first; }
    let (stem, ext) = match name.rfind('.') { Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]), _ => (name, "") };
    for i in 1..10_000 {
        let p = dir.join(format!("{stem} ({i}){ext}"));
        if !p.exists() { return p; }
    }
    dir.join(format!("{stem}-{}", rand_hex(4)))
}

impl Client {
    /// Begin (or rejoin) a large upload. The id is derived from the file's key, so offering the same file again
    /// while the saved session exists resumes it; an upload that finished and was sent gets a fresh key and id.
    pub fn large_upload_begin(&mut self, gid: &str, name: &str, mime: &str, size: u64, last_modified: u64, caption: &str) -> Result<UploadStart> {
        self.check_chan(gid)?;
        if size <= LARGE_FILE_THRESHOLD { bail!("this file is small enough to send in one piece"); }
        if size > 1u64 << 40 { bail!("this file is too large (1 TB at most)"); }
        if caption.len() > MAX_TEXT { bail!("the caption is longer than {MAX_TEXT} bytes"); }
        let name = name_of(name);
        // A file with the same name and key in the same room is the same upload; sessions are removed when it is
        // sent or cancelled, so a deliberate second send of the same file starts a fresh one.
        if let Some((id, s)) = self.st.large_uploads.iter().find(|(_, s)| s.gid == gid && s.name == name && s.size == size && s.last_modified == last_modified).map(|(id, s)| (id.clone(), s.clone())) {
            return Ok(UploadStart { id, chunk_size: s.chunk_size, chunks: s.chunks, uploaded: s.uploaded.clone(), expires: s.expires, complete: false, name: s.name });
        }
        let mut key = [0u8; 32]; let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut key); rand::rngs::OsRng.fill_bytes(&mut nonce);
        let id = {
            let h = sha256(&[b"commons/large-upload/v1", &key, &nonce, gid.as_bytes(), name.as_bytes(), &size.to_be_bytes(), &last_modified.to_be_bytes()]);
            hex::encode(&h[..16])
        };
        let chunks = chunk_count(size);
        let s = LargeUpload { gid: gid.into(), name: name.clone(), mime: clean_mime(mime), size, last_modified, chunk_size: LARGE_CHUNK_BYTES, chunks,
            blob: id.clone(), key: hex::encode(key), nonce: hex::encode(nonce), caption: caption.into(), expires: 0, uploaded: vec![], started: now_secs() };
        self.st.large_uploads.insert(id.clone(), s);
        self.save()?;
        Ok(UploadStart { id, chunk_size: LARGE_CHUNK_BYTES, chunks, uploaded: vec![], expires: 0, complete: false, name })
    }
    /// The relay answered the begin: which chunks it already holds, and when it will remove the file.
    pub fn large_upload_started(&mut self, id: &str, resp: &LargeInfoResp) -> Result<UploadStart> {
        let s = self.st.large_uploads.get_mut(id).ok_or_else(|| anyhow!("no such upload"))?;
        if resp.size != s.size || resp.chunk_size != s.chunk_size || resp.chunks != s.chunks { bail!("the relay has a different file under this id"); }
        s.uploaded = resp.uploaded.clone();
        s.expires = resp.expires;
        let out = UploadStart { id: id.into(), chunk_size: s.chunk_size, chunks: s.chunks, uploaded: s.uploaded.clone(), expires: s.expires, complete: resp.complete, name: s.name.clone() };
        self.save()?;
        Ok(out)
    }
    /// Encrypt one chunk, and say whether it still has to be sent. Its plaintext hash is kept for the whole-file
    /// root hash; every chunk the UI offers is hashed, including ones the relay already has.
    pub fn large_chunk_prepare(&mut self, id: &str, index: u64, plain: &[u8]) -> Result<PreparedChunk> {
        let s = self.st.large_uploads.get(id).ok_or_else(|| anyhow!("no such upload"))?;
        if index >= s.chunks { bail!("that is not a chunk of this file"); }
        let want = if index + 1 == s.chunks { s.size - s.chunk_size * (s.chunks - 1) } else { s.chunk_size };
        if plain.len() as u64 != want { bail!("that chunk is not the size it should be"); }
        let h = hash_chunk(plain);
        self.large_hashes.entry(id.to_string()).or_default().insert(index, h);
        let needed = !s.uploaded.contains(&index);
        let ct = if needed { encrypt_chunk(&key_of(&s.key)?, &s.blob, index, plain)? } else { vec![] };
        Ok(PreparedChunk { ct, needed })
    }
    pub fn large_chunk_done(&mut self, id: &str, index: u64) {
        if let Some(s) = self.st.large_uploads.get_mut(id) { if !s.uploaded.contains(&index) { s.uploaded.push(index); } }
    }
    /// The relay should have every chunk by now: compute the whole-file hash it must commit to.
    pub fn large_finish_prepare(&mut self, id: &str) -> Result<(String, String)> {
        let s = self.st.large_uploads.get(id).ok_or_else(|| anyhow!("no such upload"))?;
        let hashes = self.large_hashes.get(id).ok_or_else(|| anyhow!("the file was not read from the start"))?;
        if hashes.len() as u64 != s.chunks { bail!("some parts of the file were not read; start the upload again"); }
        let ordered: Vec<[u8; 32]> = (0..s.chunks).map(|i| hashes.get(&i).copied().ok_or_else(|| anyhow!("missing part {i}"))).collect::<Result<_>>()?;
        let root = hex::encode(root_hash(s.chunks, s.size, &ordered));
        Ok((s.blob.clone(), root))
    }
    /// The relay accepted the finished file: now the (ordinary-shaped) File message may go out.
    pub fn large_finish_done(&mut self, id: &str) -> Result<String> {
        let s = self.st.large_uploads.get(id).ok_or_else(|| anyhow!("no such upload"))?.clone();
        let hashes = self.large_hashes.get(id).ok_or_else(|| anyhow!("the file was not read from the start"))?;
        let ordered: Vec<[u8; 32]> = (0..s.chunks).map(|i| hashes.get(&i).copied().ok_or_else(|| anyhow!("missing part {i}"))).collect::<Result<_>>()?;
        let sha = hex::encode(root_hash(s.chunks, s.size, &ordered));
        let env = Envelope::File {
            id: rand_hex(16), name: s.name.clone(), mime: s.mime.clone(), size: s.size.min(MAX_BLOB_BYTES as u64),
            blob: s.blob.clone(), key: s.key.clone(), nonce: s.nonce.clone(), caption: s.caption.clone(), reply: None, ts: now_secs(),
            large: Some(LargeFile { size: s.size, chunks: s.chunks, chunk_size: s.chunk_size, sha256: sha, expires: s.expires }),
        };
        env.validate().map_err(|e| anyhow!("{e}"))?;
        let msg_id = match &env { Envelope::File { id, .. } => id.clone(), _ => unreachable!() };
        self.queue_env(&s.gid, env);
        self.st.large_uploads.remove(id);
        self.large_hashes.remove(id);
        self.save()?;
        Ok(msg_id)
    }
    /// Give up: forget the saved session and tell the caller which relay file to delete.
    pub fn large_upload_cancel(&mut self, id: &str) -> Result<Option<String>> {
        self.large_hashes.remove(id);
        let gone = self.st.large_uploads.remove(id);
        self.save()?;
        Ok(gone.map(|s| s.blob))
    }

    // ------------------------------------------------------------------ downloads
    pub fn large_download_begin(&mut self, gid: &str, msg_id: &str) -> Result<LargeDownload> {
        self.large_download_begin_in(gid, msg_id, None)
    }

    /// [`Self::large_download_begin`], saving into `folder` for this one file. `None` or an empty folder means the
    /// downloads folder from Settings, which this never changes. A different folder from the one a file was saved to or
    /// was partly saved to starts it again there.
    pub fn large_download_begin_in(&mut self, gid: &str, msg_id: &str, folder: Option<&str>) -> Result<LargeDownload> {
        let folder = folder.map(str::trim).filter(|f| !f.is_empty()).map(PathBuf::from);
        if folder.as_ref().is_some_and(|f| !f.is_absolute()) { bail!("the folder must be a full path, like /home/you/Downloads"); }
        let m = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == msg_id)).ok_or_else(|| anyhow!("no such message"))?;
        let MsgBody::File { name, mime, blob, key, nonce, large: Some(large), .. } = &m.body else { bail!("not a large file") };
        if large.expires <= now_secs() { bail!("this file has expired and the relay has removed it"); }
        let same_place = |d: &LargeDownload| folder.as_ref().is_none_or(|f| Path::new(&d.dir) == f.as_path());
        if self.st.large_downloads.get(msg_id).is_some_and(|d| d.finished && Path::new(&d.path).exists() && same_place(d)) {
            return Ok(self.st.large_downloads[msg_id].clone());
        }
        let dir = folder.clone().unwrap_or_else(|| self.download_dir());
        std::fs::create_dir_all(&dir)?;
        let reusable = self.st.large_downloads.get(msg_id).is_some_and(|d| !d.finished && d.blob == *blob && d.size == large.size && d.chunks == large.chunks && Path::new(&d.part).exists() && same_place(d));
        if !reusable {
            if let Some(old) = self.st.large_downloads.remove(msg_id) {
                if !old.finished { let _ = std::fs::remove_file(&old.part); }
            }
        }
        if !self.st.large_downloads.contains_key(msg_id) {
            let save_name = safe_file_name(name);
            let final_path = unique_path(&dir, &save_name);
            let part = dir.join(format!(".megatron-{blob}.part"));
            let f = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&part)?;
            f.sync_all().ok();
            self.st.large_downloads.insert(msg_id.to_string(), LargeDownload {
                gid: gid.into(), msg_id: msg_id.into(), blob: blob.clone(), name: name.clone(), save_name, mime: mime.clone(),
                size: large.size, chunks: large.chunks, chunk_size: large.chunk_size, sha256: large.sha256.clone(),
                key: key.clone(), nonce: nonce.clone(), expires: large.expires, dir: dir.display().to_string(), part: part.display().to_string(),
                done: 0, hashes: vec![], finished: false, path: final_path.display().to_string(),
            });
        }
        // Rebuild the resume point from the part file itself, not only from the saved count: a crash may have left
        // whole verified chunks on disk that were never counted, or half of one. Only complete chunks count, and
        // hashing them again is what lets the whole-file check be finished after any restart.
        let (chunks, size, chunk_size, part) = {
            let d = self.st.large_downloads.get(msg_id).unwrap();
            (d.chunks, d.size, d.chunk_size, d.part.clone())
        };
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut full = if have >= size { chunks } else { (have / chunk_size).min(chunks) };
        if full > 0 && have < size && full == chunks { full = chunks - 1; }
        let mut hashes = Vec::with_capacity(full as usize);
        if full > 0 {
            let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&part)?;
            let mut buf = vec![0u8; chunk_size as usize];
            for i in 0..full {
                let len = if i + 1 == chunks { size - chunk_size * (chunks - 1) } else { chunk_size } as usize;
                f.read_exact(&mut buf[..len])?;
                hashes.push(hex::encode(hash_chunk(&buf[..len])));
            }
            // Drop a partially written chunk at the end.
            let keep = if full == chunks { size } else { full * chunk_size };
            f.set_len(keep)?;
            f.sync_all().ok();
        }
        let d = self.st.large_downloads.get_mut(msg_id).unwrap();
        d.done = full;
        d.hashes = hashes;
        let out = d.clone();
        self.save()?;
        Ok(out)
    }
    /// Decrypt, verify and append the next chunk. The last one finishes the file, checks the whole-file hash and
    /// moves it into place.
    pub fn large_download_write(&mut self, gid: &str, msg_id: &str, ct: &[u8]) -> Result<DownloadStep> {
        let _ = gid;
        let d = self.st.large_downloads.get(msg_id).ok_or_else(|| anyhow!("no download in progress"))?.clone();
        if d.finished { return Ok(DownloadStep { done: d.done, chunks: d.chunks, size: d.size, finished: true, path: d.path }); }
        if d.done >= d.chunks { bail!("this download is already complete"); }
        let index = d.done;
        let plain_len = if index + 1 == d.chunks { d.size - d.chunk_size * (d.chunks - 1) } else { d.chunk_size };
        let plain = decrypt_chunk(&key_of(&d.key)?, &d.blob, index, plain_len, ct)?;
        if plain.len() as u64 != plain_len { bail!("this part of the file is not the size it should be"); }
        let h = hash_chunk(&plain);
        {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&d.part)?;
            f.write_all(&plain)?;
            if index + 1 == d.chunks { f.sync_all()?; }
        }
        let (done, chunks, size, path) = {
            let d = self.st.large_downloads.get_mut(msg_id).unwrap();
            d.done += 1;
            d.hashes.push(hex::encode(h));
            (d.done, d.chunks, d.size, d.path.clone())
        };
        if done == chunks {
            let (root, part, want, final_path) = {
                let d = self.st.large_downloads.get(msg_id).unwrap();
                let ordered: Vec<[u8; 32]> = d.hashes.iter().map(|x| hex32(x)).collect::<Result<_>>()?;
                (hex::encode(root_hash(d.chunks, d.size, &ordered)), d.part.clone(), d.sha256.clone(), d.path.clone())
            };
            let ok = root == want && std::fs::metadata(&part).map(|m| m.len() == size).unwrap_or(false);
            if !ok {
                let _ = std::fs::remove_file(&part);
                self.st.large_downloads.remove(msg_id);
                self.save()?;
                bail!("the file did not arrive intact (the whole-file check failed), so nothing was saved");
            }
            std::fs::rename(&part, &final_path)?;
            if let Some(d) = self.st.large_downloads.get_mut(msg_id) { d.finished = true; d.path = final_path.clone(); }
            self.save()?;
            return Ok(DownloadStep { done, chunks, size, finished: true, path: final_path });
        }
        if done % SAVE_EVERY == 0 { self.save()?; }
        Ok(DownloadStep { done, chunks, size, finished: false, path })
    }
    pub fn large_download_state(&self, msg_id: &str) -> Option<LargeDownload> { self.st.large_downloads.get(msg_id).cloned() }
    /// Throw away a partial download and its file. A finished one is the person's file and stays.
    pub fn large_download_cancel(&mut self, msg_id: &str) -> Result<()> {
        if let Some(d) = self.st.large_downloads.remove(msg_id) {
            if !d.finished { let _ = std::fs::remove_file(&d.part); }
        }
        self.save()
    }

    pub fn download_dir(&self) -> PathBuf {
        let set = self.st.download_dir.trim();
        if !set.is_empty() { return PathBuf::from(set); }
        if let Some(d) = std::env::var_os("COMMONS_DOWNLOAD_DIR") { if !d.is_empty() { return PathBuf::from(d); } }
        default_download_dir()
    }
    pub fn set_download_dir(&mut self, dir: &str) -> Result<String> {
        let dir = dir.trim();
        if dir.is_empty() { self.st.download_dir.clear(); }
        else {
            let p = Path::new(dir);
            if !p.is_absolute() { bail!("the downloads folder must be a full path, like /home/you/Downloads"); }
            std::fs::create_dir_all(p)?;
            self.st.download_dir = dir.to_string();
        }
        self.save()?;
        Ok(self.download_dir().display().to_string())
    }
    /// Open the folder holding a finished download (in a separate process: this must never block the UI).
    pub fn show_download_in_folder(&self, msg_id: &str) -> Result<bool> {
        let d = self.st.large_downloads.get(msg_id).ok_or_else(|| anyhow!("no such download"))?;
        if !d.finished || !Path::new(&d.path).exists() { bail!("the file is not saved yet"); }
        Ok(show_in_folder(Path::new(&d.path)))
    }
    /// The uploader (any device of that account) may remove the relay's copy. Returns the file id to delete there.
    pub fn large_delete_prepare(&mut self, gid: &str, msg_id: &str) -> Result<String> {
        let m = self.st.chans.get(gid).and_then(|l| l.msgs.iter().find(|m| m.id == msg_id)).ok_or_else(|| anyhow!("no such message"))?;
        if m.sender != self.my_account() { bail!("only the sender can delete this file"); }
        let MsgBody::File { blob, large: Some(_), .. } = &m.body else { bail!("not a large file") };
        Ok(blob.clone())
    }
    /// The relay copy is gone: remove any local partial download, and tell everyone the message is deleted.
    pub fn large_delete_done(&mut self, gid: &str, msg_id: &str) -> Result<()> {
        if let Some(d) = self.st.large_downloads.remove(msg_id) { let _ = std::fs::remove_file(&d.part); }
        self.queue_env(gid, Envelope::Delete { target: msg_id.into() });
        self.save()
    }
}

fn default_download_dir() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.map(|h| h.join("Downloads")).filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::temp_dir().join("Megatron downloads"))
}

/// The system's own folder chooser, for the Save to dialog. `Ok(None)` when the person cancelled it. Blocks until they
/// answer, so callers must not hold the client while it is open.
pub fn pick_folder(start: &str) -> Result<Option<String>> {
    let start = Some(Path::new(start.trim())).filter(|p| p.is_dir());
    native_folder_picker(start)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn picked(out: std::process::Output) -> Option<String> {
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']).to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(target_os = "linux")]
fn native_folder_picker(start: Option<&Path>) -> Result<Option<String>> {
    use std::process::Command;
    let kdialog = || {
        let mut c = Command::new("kdialog");
        c.args(["--title", "Save to", "--getexistingdirectory"]).arg(start.map(Path::as_os_str).unwrap_or(std::ffi::OsStr::new(".")));
        c
    };
    let zenity = || {
        let mut c = Command::new("zenity");
        c.args(["--file-selection", "--directory", "--title=Save to"]);
        if let Some(p) = start { c.arg(format!("--filename={}/", p.display())); }
        c
    };
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_uppercase().contains("KDE"));
    let order = if kde { [kdialog(), zenity()] } else { [zenity(), kdialog()] };
    for mut picker in order {
        match picker.output() {
            Ok(out) => return Ok(picked(out)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => bail!("could not open the folder chooser: {e}"),
        }
    }
    bail!("this computer has no folder chooser (zenity or kdialog); type the folder instead")
}

#[cfg(target_os = "macos")]
fn native_folder_picker(start: Option<&Path>) -> Result<Option<String>> {
    let mut c = std::process::Command::new("osascript");
    match start {
        Some(p) => { c.args(["-e", "on run argv", "-e", "POSIX path of (choose folder default location (POSIX file (item 1 of argv)))", "-e", "end run"]).arg(p); }
        None => { c.args(["-e", "POSIX path of (choose folder)"]); }
    }
    Ok(picked(c.output().map_err(|e| anyhow!("could not open the folder chooser: {e}"))?))
}

#[cfg(windows)]
fn native_folder_picker(start: Option<&Path>) -> Result<Option<String>> {
    use std::os::windows::process::CommandExt;
    const SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; \
        $d=New-Object System.Windows.Forms.FolderBrowserDialog; if($env:MEGATRON_PICK_START){$d.SelectedPath=$env:MEGATRON_PICK_START}; \
        if($d.ShowDialog() -eq 'OK'){[Console]::Out.Write($d.SelectedPath)}";
    let mut c = std::process::Command::new("powershell");
    c.args(["-NoProfile", "-STA", "-Command", SCRIPT]).creation_flags(0x0800_0000);   // CREATE_NO_WINDOW
    if let Some(p) = start { c.env("MEGATRON_PICK_START", p); }
    Ok(picked(c.output().map_err(|e| anyhow!("could not open the folder chooser: {e}"))?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn native_folder_picker(_start: Option<&Path>) -> Result<Option<String>> { bail!("no folder chooser on this device") }

fn show_in_folder(path: &Path) -> bool {
    let dir = path.parent().unwrap_or(path);
    #[cfg(target_os = "linux")]
    let cmd = std::process::Command::new("xdg-open").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(windows)]
    let cmd = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let cmd: std::io::Result<std::process::Child> = Err(std::io::Error::other("no file manager"));
    cmd.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_round_trip_and_each_position_is_bound() {
        let key = [7u8; 32];
        let plain = vec![42u8; 1000];
        let ct = encrypt_chunk(&key, "00112233445566778899aabbccddeeff", 3, &plain).unwrap();
        assert_ne!(ct[..100], plain[..100]);
        assert_eq!(decrypt_chunk(&key, "00112233445566778899aabbccddeeff", 3, 1000, &ct).unwrap(), plain);

        // the same bytes in another position, or another file, or another length: refused
        assert!(decrypt_chunk(&key, "00112233445566778899aabbccddeeff", 4, 1000, &ct).is_err());
        assert!(decrypt_chunk(&key, "ffeeddccbbaa99887766554433221100", 3, 1000, &ct).is_err());
        assert!(decrypt_chunk(&key, "00112233445566778899aabbccddeeff", 3, 999, &ct).is_err());
        // a different file key: refused
        assert!(decrypt_chunk(&[8u8; 32], "00112233445566778899aabbccddeeff", 3, 1000, &ct).is_err());
        // tampered ciphertext: refused
        let mut bad = ct.clone(); bad[0] ^= 1;
        assert!(decrypt_chunk(&key, "00112233445566778899aabbccddeeff", 3, 1000, &bad).is_err());
    }
    #[test]
    fn the_whole_file_hash_covers_order_and_size() {
        let h: Vec<[u8; 32]> = (0..3).map(|i| hash_chunk(&[i as u8; 100])).collect();
        let a = root_hash(3, 300, &h);
        assert_eq!(a, root_hash(3, 300, &h));
        let mut swapped = h.clone(); swapped.swap(0, 1);
        assert_ne!(a, root_hash(3, 300, &swapped));
        assert_ne!(a, root_hash(3, 299, &h));
        assert_ne!(a, root_hash(4, 300, &h));
    }
    #[test]
    fn the_last_chunk_is_the_remainder() {
        let l = LargeFile { size: LARGE_CHUNK_BYTES * 2 + 5, chunks: 3, chunk_size: LARGE_CHUNK_BYTES, sha256: "00".repeat(32), expires: 1 };
        assert_eq!(l.plain_len(0), Some(LARGE_CHUNK_BYTES));
        assert_eq!(l.plain_len(2), Some(5));
        assert_eq!(l.plain_len(3), None);
    }
}
