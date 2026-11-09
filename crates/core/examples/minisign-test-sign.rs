//! A throwaway minisign signer for the browser-update tests. Never used by the app, never touches the owner's key.
//!
//!   cargo run -q -p commons-core --example minisign-test-sign -- gen <dir>
//!       makes a TEST key pair in <dir> and prints (one line) the public key to embed in a test build, in the same
//!       base64 form as app/src-tauri/tauri.conf.json plugins.updater.pubkey
//!   cargo run -q -p commons-core --example minisign-test-sign -- sign <dir> <file> <version>
//!       signs <file> (pre-hashed, with the version in the trusted comment, as `tauri signer sign --app-version` does),
//!       writes <file>.sig, and prints (one line) the base64 of the signature text, which is what the manifest holds
use base64::Engine as _;
use blake2::Digest as _;
use ed25519_dalek::{Signer as _, SigningKey};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") if args.len() == 2 => gen(&args[1]),
        Some("sign") if args.len() == 4 => sign(&args[1], &args[2], &args[3]),
        _ => {
            eprintln!("usage: minisign-test-sign gen <dir> | sign <dir> <file> <version>");
            std::process::exit(2);
        }
    }
}

fn b64(data: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn key_id(dir: &Path) -> [u8; 8] {
    let hex = std::fs::read_to_string(dir.join("test.key_id")).expect("test key id");
    let bytes = hex.trim().as_bytes();
    let mut id = [0u8; 8];
    for (i, slot) in id.iter_mut().enumerate() {
        *slot = u8::from_str_radix(std::str::from_utf8(&bytes[i * 2..i * 2 + 2]).unwrap(), 16).unwrap();
    }
    id
}

fn secret(dir: &Path) -> SigningKey {
    let hex = std::fs::read_to_string(dir.join("test.secret")).expect("test secret key");
    let bytes = hex.trim().as_bytes();
    let mut seed = [0u8; 32];
    for (i, slot) in seed.iter_mut().enumerate() {
        *slot = u8::from_str_radix(std::str::from_utf8(&bytes[i * 2..i * 2 + 2]).unwrap(), 16).unwrap();
    }
    SigningKey::from_bytes(&seed)
}

fn gen(dir: &str) {
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir).expect("key folder");
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let mut id = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let key = SigningKey::from_bytes(&seed);
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    std::fs::write(dir.join("test.secret"), hex(&seed)).unwrap();
    std::fs::write(dir.join("test.key_id"), hex(&id)).unwrap();
    let mut bin = Vec::with_capacity(42);
    bin.extend_from_slice(&[0x45, 0x44]);
    bin.extend_from_slice(&id);
    bin.extend_from_slice(key.verifying_key().as_bytes());
    let pub_text = format!(
        "untrusted comment: minisign public key: {}\n{}\n",
        hex(&id).to_uppercase(),
        b64(&bin)
    );
    std::fs::write(dir.join("test.pub"), &pub_text).unwrap();
    println!("{}", b64(pub_text.as_bytes()));
}

fn sign(dir: &str, file: &str, version: &str) {
    let dir = Path::new(dir);
    let key = secret(dir);
    let id = key_id(dir);
    let data = std::fs::read(file).expect("file to sign");
    let mut h = [0u8; 64];
    {
        let mut hasher = blake2::Blake2b512::new();
        hasher.update(&data);
        h.copy_from_slice(&hasher.finalize());
    }
    let sig: [u8; 64] = key.sign(&h).to_bytes();
    let mut bin1 = Vec::with_capacity(74);
    bin1.extend_from_slice(&[0x45, 0x44]); // pre-hashed, as the Tauri signer writes
    bin1.extend_from_slice(&id);
    bin1.extend_from_slice(&sig);
    let trusted = format!(
        "timestamp:{}\tfile:{}\tprehashed\tversion:{version}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        Path::new(file).file_name().unwrap_or_default().to_string_lossy()
    );
    let mut global = Vec::with_capacity(64 + trusted.len());
    global.extend_from_slice(&sig);
    global.extend_from_slice(trusted.as_bytes());
    let global = key.sign(&global).to_bytes();
    let text = format!(
        "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {}\n{}\n",
        b64(&bin1),
        trusted,
        b64(global)
    );
    std::fs::write(format!("{file}.sig"), &text).unwrap();
    println!("{}", b64(text.as_bytes()));
}
