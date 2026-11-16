//! Password invites: a friend types a password the inviter chose and is let in. The password protects a sealed copy of an
//! ordinary invite link (which already carries the relay's sign-up secret when there is one). The relay is given only an
//! id and ciphertext, both derived from the password with a memory-hard function salted by the relay's address, so it
//! can read neither the password nor the link. The password delivers the invite and nothing else: expiry, use limits,
//! bans and roster rules are still enforced by the community's admins when the join request arrives.
use crate::transport::{Api, HttpResp};
use crate::vault::KdfParams;
use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, XChaCha20Poly1305, XNonce};
use commons_proto::{b64, unb64, PwLookupReq, PwLookupResp};
use rand::{Rng, RngCore};
use sha2::{Digest, Sha256};

const VER: u8 = 1;
/// The plaintext is padded to a multiple of this, so the size of the sealed copy says nothing about the link.
const PAD: usize = 1024;
/// Most letters, digits and symbols the relay would take for a password: bounds the work of one derivation.
const MAX_PASSWORD: usize = 200;
/// The relay's operator, or anyone with a copy of its database, can test guesses offline at one Argon2id run each, so the floor is set
/// where a well-equipped guesser needs centuries, not weeks (docs/security-0.3.6.md, M4). Apps before 0.3.6 accepted 35.
pub const MIN_BITS: f64 = 50.0;
pub const GOOD_BITS: f64 = 60.0;
/// Words in one suggestion.
pub const SUGGEST_WORDS: usize = 5;

pub const WORDS: &str = include_str!("pwwords.txt");
fn words() -> Vec<&'static str> { WORDS.split_whitespace().collect() }

/// "Amber  Fox-teal" and "amber-fox-teal" are the same password: people say these aloud and type them on phones.
pub fn normalize(pw: &str) -> String {
    pw.trim().to_lowercase().split(|c: char| c.is_whitespace() || c == '-' || c == '_').filter(|t| !t.is_empty()).collect::<Vec<_>>().join("-")
}

/// A fresh suggestion: five random words and a two digit number, e.g. "amber-fox-teal-moss-rain-47" (about 56 bits).
pub fn suggest() -> String {
    let w = words();
    let mut r = rand::rngs::OsRng;
    let mut picked: Vec<&str> = vec![];
    while picked.len() < SUGGEST_WORDS {
        let x = w[r.gen_range(0..w.len())];
        if !picked.contains(&x) { picked.push(x); }
    }
    format!("{}-{:02}", picked.join("-"), r.gen_range(0..100))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Strength { pub bits: f64, pub ok: bool, pub warn: bool }

/// A word that is not in the small built-in list is still a word: a guesser's dictionary of common words of this length. Short words are
/// cheap; a long one is worth a little more, never what the same letters drawn at random would be.
fn word_bits(len: usize) -> f64 { (10.0 + 0.8 * len.saturating_sub(4) as f64).min(16.0) }

/// The same letters drawn at random (an upper bound for any token, and all a token of mixed characters is worth).
fn random_bits(chars: &[char]) -> f64 {
    let mut distinct = chars.to_vec(); distinct.sort(); distinct.dedup();
    let (digits, lower, other) = (chars.iter().filter(|c| c.is_ascii_digit()).count(), chars.iter().filter(|c| c.is_alphabetic()).count(), chars.iter().filter(|c| !c.is_ascii_digit() && !c.is_alphabetic()).count());
    let pool: f64 = (if digits > 0 { 10.0 } else { 0.0 }) + (if lower > 0 { 26.0 } else { 0.0 }) + (if other > 0 { 32.0 } else { 0.0 });
    let eff = distinct.len() as f64 + (chars.len() - distinct.len()) as f64 * 0.3;
    // Counting up or down ("abcdef", "123456") is not random.
    let run = chars.windows(2).filter(|w| (w[1] as i64 - w[0] as i64).abs() == 1).count();
    let eff = (eff - run as f64 * 0.8).max(0.0);
    eff * pool.max(2.0).log2() * 0.6
}

/// One run of letters, digits or other characters inside a token ("blue", "2024", "!").
fn piece_bits(chars: &[char]) -> f64 {
    if chars.iter().all(|c| c.is_ascii_digit()) {
        // A one or two digit number drawn at random is worth what it looks like; a longer one is most likely a year, a date or a pattern.
        return if chars.len() <= 2 { chars.len() as f64 * 10f64.log2() } else { (3.0 + chars.len() as f64).min(random_bits(chars)) };
    }
    if chars.iter().all(|c| c.is_alphabetic()) { return word_bits(chars.len()).min(random_bits(chars)); }
    random_bits(chars)
}

/// A deliberately cautious estimate of how many guesses a password takes, in bits. People pick words, names, years and patterns, so
/// every run of letters counts as a dictionary word (a few bits per word, whatever its letters), digits as a short number or a date,
/// a word said twice as nothing, and the worst-known passwords and passphrases as nothing at all.
pub fn strength(pw: &str) -> Strength {
    let n = normalize(pw);
    let list = words();
    let bad = ["password", "passwort", "qwerty", "letmein", "welcome", "admin", "megatron", "mega", "invite", "friend", "friends", "secret", "chat", "hello", "monkey", "dragon", "football", "iloveyou", "abc123", "123456", "12345678", "111111"];
    // Famous passphrases: every guesser's list starts with them.
    let bad_phrases = ["correct-horse-battery-staple", "the-quick-brown-fox", "to-be-or-not-to-be", "mary-had-a-little-lamb", "all-your-base-are-belong-to-us", "open-sesame"];
    if bad_phrases.iter().any(|p| n.contains(p)) { return Strength { bits: 0.0, ok: false, warn: true }; }
    let mut bits = 0.0;
    let mut seen: Vec<&str> = vec![];
    for tok in n.split('-') {
        if seen.contains(&tok) { continue; }       // saying a word twice adds nothing
        seen.push(tok);
        if bad.contains(&tok) { continue; }
        if list.binary_search(&tok).is_ok() { bits += (list.len() as f64).log2(); continue; }
        let chars: Vec<char> = tok.chars().collect();
        // "bluefish2024" is a word and a year, not twelve random characters: each run is scored on its own, plus a little for the mix.
        let mut pieces: Vec<&[char]> = vec![];
        let mut start = 0;
        let class = |c: char| if c.is_ascii_digit() { 0 } else if c.is_alphabetic() { 1 } else { 2 };
        for i in 1..=chars.len() {
            if i == chars.len() || class(chars[i]) != class(chars[start]) { pieces.push(&chars[start..i]); start = i; }
        }
        // Four or more runs ("xq7!Lp2#") is not a word with a number on it; that is what a random draw looks like.
        bits += match pieces.len() {
            1 => piece_bits(&chars),
            n if n >= 4 => random_bits(&chars),
            n => (pieces.iter().map(|p| piece_bits(p)).sum::<f64>() + (n as f64 - 1.0)).min(random_bits(&chars)),
        };
    }
    let ok = bits >= MIN_BITS && pw.chars().count() >= 8 && pw.chars().count() <= MAX_PASSWORD;
    Strength { bits, ok, warn: bits < GOOD_BITS }
}

/// Refuse a password that anyone could guess.
pub fn check(pw: &str) -> Result<()> {
    if pw.chars().count() > MAX_PASSWORD { bail!("that password is far too long"); }
    let s = strength(pw);
    if !s.ok { bail!("that password is too easy to guess, and anyone who guessed it could join. Use the suggestion, or something longer."); }
    Ok(())
}

/// The relay's address reduced to what names the server: scheme, user-visible case, a trailing slash, a default port (80 or 443),
/// a trailing dot on the host and any query or fragment do not count. "HTTPS://Chat.Example.org:443/" and "chat.example.org" are one relay.
pub fn canon_relay(relay: &str) -> String {
    let mut s = relay.trim().to_lowercase();
    for p in ["https://", "http://"] {
        if let Some(r) = s.strip_prefix(p) { s = r.to_string(); break; }
    }
    let s = s.split(['?', '#']).next().unwrap_or("");
    let (hostport, path) = match s.find('/') { Some(i) => (&s[..i], &s[i..]), None => (s, "") };
    let path = path.trim_end_matches('/');
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) && !h.ends_with(':') => (h, p.trim_start_matches('0')),
        _ => (hostport, ""),
    };
    let host = host.trim_end_matches('.');
    let port = if port.is_empty() || port == "80" || port == "443" { String::new() } else { format!(":{port}") };
    format!("{host}{port}{path}")
}
/// True when two spellings name the same relay.
pub fn same_relay(a: &str, b: &str) -> bool { canon_relay(a) == canon_relay(b) }

/// What older apps salted with: the address exactly as spelled (lower case, no final slash), scheme included.
fn relay_key_v1(relay: &str) -> String { relay.trim().trim_end_matches('/').to_lowercase() }

pub struct Derived { pub lid: String, pub key: [u8; 32] }

fn derive_salted(pw: &str, salt_text: &str, kdf: KdfParams) -> Result<Derived> {
    let salt = &Sha256::digest(salt_text.as_bytes())[..16];
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32)).map_err(|e| anyhow!("kdf params: {e}"))?;
    let mut master = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(normalize(pw).as_bytes(), salt, &mut master).map_err(|e| anyhow!("kdf: {e}"))?;
    let part = |label: &str| -> [u8; 32] { let mut h = Sha256::new(); h.update(label.as_bytes()); h.update(master); h.finalize().into() };
    Ok(Derived { lid: hex::encode(part("mega/pwinvite/id")), key: part("mega/pwinvite/key") })
}

/// The lookup id and the key, from the password and the relay's address. The salt is per relay (by its canonical address, so the way
/// the address is spelled does not matter), so one table of guesses cannot serve two relays. The two outputs are split by hashing,
/// so the id reveals nothing about the key.
pub fn derive(pw: &str, relay: &str, kdf: KdfParams) -> Result<Derived> {
    derive_salted(pw, &format!("mega/pwinvite/v2/{}", canon_relay(relay)), kdf)
}

/// The derivation of apps before the address was canonicalised. Only used to find invites those apps made.
pub fn derive_legacy(pw: &str, relay: &str, kdf: KdfParams) -> Result<Derived> {
    derive_salted(pw, &format!("mega/pwinvite/v1/{}", relay_key_v1(relay)), kdf)
}

/// version | nonce | ciphertext. The id is bound in, so a copy cannot be moved under another password's id.
pub fn seal(d: &Derived, link: &str) -> Result<Vec<u8>> {
    let mut pt = (link.len() as u32).to_be_bytes().to_vec();
    pt.extend_from_slice(link.as_bytes());
    pt.resize(pt.len().div_ceil(PAD) * PAD, 0);
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = XChaCha20Poly1305::new((&d.key).into()).encrypt(XNonce::from_slice(&nonce), Payload { msg: &pt, aad: &aad(&d.lid) }).map_err(|_| anyhow!("seal"))?;
    let mut out = vec![VER];
    out.extend_from_slice(&nonce);
    out.extend(ct);
    Ok(out)
}
fn aad(lid: &str) -> Vec<u8> { let mut a = b"mega-pwinvite-v1/".to_vec(); a.extend_from_slice(lid.as_bytes()); a }

pub fn open(d: &Derived, blob: &[u8]) -> Result<String> {
    if blob.len() < 1 + 24 + 16 + 4 || blob[0] != VER { bail!("unreadable"); }
    let pt = XChaCha20Poly1305::new((&d.key).into()).decrypt(XNonce::from_slice(&blob[1..25]), Payload { msg: &blob[25..], aad: &aad(&d.lid) }).map_err(|_| anyhow!("unreadable"))?;
    let n = u32::from_be_bytes(pt[..4].try_into().unwrap()) as usize;
    if 4 + n > pt.len() { bail!("unreadable"); }
    String::from_utf8(pt[4..4 + n].to_vec()).map_err(|_| anyhow!("unreadable"))
}

pub const NOT_FOUND: &str = "No invite goes with that password. Check it, ask for it again, or it may have expired or been used up.";

/// Asks the relay for the sealed copy and opens it with the password. A wrong password, an expired invite, a revoked
/// one and a used-up one all read the same here, and on the relay.
///
/// Invites made before the address was canonicalised sit under the older id. A relay that knows `alt` is asked for both ids in one
/// request (one miss, not two). An older relay is asked for the new id and then, on a miss, for the old one.
pub fn lookup(api: &Api, relay: &str, pw: &str, kdf: KdfParams) -> Result<String> {
    check_len(pw)?;
    let info = api.info(None).map_err(|e| anyhow!("could not reach the server: {e}"))?;
    if !info.pwinvite { bail!("This server is too old to take password invites. Ask for an invite link instead."); }
    let d = derive(pw, relay, kdf)?;
    let legacy = derive_legacy(pw, relay, kdf)?;
    // None: the relay has nothing under that id.
    let ask = |lid: &str, alt: Option<&str>| -> Result<Option<Vec<u8>>> {
        let body = serde_json::to_vec(&PwLookupReq { lid: lid.to_string(), alt: alt.map(|a| a.to_string()) })?;
        let r: HttpResp = api.t.request(None, "POST", "/v1/pwinvite/lookup", &body, std::time::Duration::from_secs(15))?;
        match r.status {
            200 => {
                let resp: PwLookupResp = serde_json::from_slice(&r.body).map_err(|_| anyhow!(NOT_FOUND))?;
                Ok(Some(unb64(&resp.blob).map_err(|_| anyhow!(NOT_FOUND))?))
            }
            429 => bail!("Too many tries just now. Wait a few minutes and try again."),
            _ => Ok(None),
        }
    };
    let mut blob = ask(&d.lid, info.pwalt.then_some(legacy.lid.as_str()))?;
    if blob.is_none() && !info.pwalt { blob = ask(&legacy.lid, None)?; }
    let blob = blob.ok_or_else(|| anyhow!(NOT_FOUND))?;
    // The id that matched decides the key; the sealed copy is bound to its own id, so only the right pair opens it.
    open(&d, &blob).or_else(|_| open(&legacy, &blob)).map_err(|_| anyhow!(NOT_FOUND))
}
fn check_len(pw: &str) -> Result<()> {
    if pw.trim().is_empty() { bail!("Type the invite password you were given."); }
    if pw.chars().count() > MAX_PASSWORD { bail!(NOT_FOUND); }
    Ok(())
}

pub fn put_body(d: &Derived, link: &str, ttl: u64, uses: Option<u32>) -> Result<Vec<u8>> {
    let fetches = uses.map(|u| u.saturating_mul(3).saturating_add(3)).unwrap_or(100).min(200);
    Ok(serde_json::to_vec(&commons_proto::PwPutReq { blob: b64(&seal(d, link)?), ttl, fetches })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    const K: KdfParams = KdfParams::TEST;
    #[test]
    fn same_password_same_ids_and_other_relay_differs() {
        let a = derive("Amber Fox teal 47", "https://r.example/", K).unwrap();
        let b = derive("amber-fox-TEAL-47", "https://R.example", K).unwrap();
        assert_eq!((a.lid.clone(), a.key), (b.lid.clone(), b.key));
        let c = derive("amber-fox-teal-47", "https://other.example", K).unwrap();
        assert_ne!(a.lid, c.lid);
        assert_ne!(a.key, c.key);
        let d = derive("amber-fox-teal-48", "https://r.example", K).unwrap();
        assert_ne!(a.lid, d.lid);
        assert_ne!(a.lid, hex::encode(a.key), "the id says nothing about the key");
        assert_eq!(a.lid.len(), 64);
    }
    #[test]
    fn the_spelling_of_the_relay_address_does_not_change_the_id() {
        let want = derive("amber-fox-teal-47", "https://chat.example.org", K).unwrap().lid;
        for same in ["https://chat.example.org/", "HTTPS://Chat.Example.ORG", "http://chat.example.org", "https://chat.example.org:443", "https://chat.example.org:443/", "chat.example.org", " https://chat.example.org// ", "http://chat.example.org:80", "https://chat.example.org./", "https://chat.example.org?x=1#y"] {
            assert_eq!(derive("amber-fox-teal-47", same, K).unwrap().lid, want, "{same}");
            assert!(same_relay(same, "https://chat.example.org"), "{same}");
        }
        for other in ["https://chat.example.org:8443", "https://chat.example.org/relay", "https://www.chat.example.org", "https://chat.example.net", "http://127.0.0.1:8787", "http://127.0.0.1:8788"] {
            assert_ne!(derive("amber-fox-teal-47", other, K).unwrap().lid, want, "{other}");
            assert!(!same_relay(other, "https://chat.example.org"), "{other}");
        }
        assert_eq!(canon_relay("http://[::1]:8787/"), "[::1]:8787");
        assert_eq!(canon_relay("http://127.0.0.1:08787"), "127.0.0.1:8787");
        // And the older derivation is still available, to find what older apps filed.
        let old = derive_legacy("amber-fox-teal-47", "https://chat.example.org/", K).unwrap();
        assert_ne!(old.lid, want);
        assert_eq!(old.lid, derive_legacy("amber-fox-teal-47", "https://CHAT.example.org", K).unwrap().lid, "the old rule: case and a final slash only");
        assert_ne!(old.lid, derive_legacy("amber-fox-teal-47", "chat.example.org", K).unwrap().lid, "the old rule: the scheme counted");
    }
    #[test]
    fn seals_and_opens_but_only_for_the_password() {
        let a = derive("amber-fox-teal-47", "https://r.example", K).unwrap();
        let link = "commons://join/abcdef";
        let blob = seal(&a, link).unwrap();
        assert_eq!(open(&a, &blob).unwrap(), link);
        assert_eq!((blob.len() - 1 - 24 - 16) % PAD, 0, "padded");
        let w = derive("amber-fox-teal-48", "https://r.example", K).unwrap();
        assert!(open(&w, &blob).is_err());
        // The same key under another id (a copy moved) does not open either.
        let moved = Derived { lid: w.lid.clone(), key: a.key };
        assert!(open(&moved, &blob).is_err());
        let mut t = blob.clone(); let n = t.len(); t[n - 3] ^= 1;
        assert!(open(&a, &t).is_err());
        // Two seals of one link differ, and a long and a short link give the same size.
        assert_ne!(blob, seal(&a, link).unwrap());
        assert_eq!(blob.len(), seal(&a, &"x".repeat(400)).unwrap().len());
    }
    #[test]
    fn suggestions_pass_and_short_or_known_bad_ones_do_not() {
        for _ in 0..200 { let s = suggest(); assert!(check(&s).is_ok(), "{s}"); assert_eq!(s.split('-').count(), SUGGEST_WORDS + 1); assert!(strength(&s).bits >= 55.0, "{s}"); }
        for bad in ["", "hunter2", "password", "Password1", "12345678", "qwertyuiop", "abcdefgh", "letmein123", "bluefish2024", "aaaaaaaaaaaa", "mega-mega-mega", "friend friend"] { assert!(check(bad).is_err(), "{bad} should be refused"); }
        for good in ["amber-fox-teal-moss-rain-47", "my dog ate the red couch twice", "seven green parrots argue over lunch", "Kettle Window Marble Thunder Pencil 9"] { assert!(check(good).is_ok(), "{good} should pass ({} bits)", strength(good).bits); }
        assert!(strength("amber-fox-teal-moss-rain-47").warn, "a suggestion is fine but not strong enough to call good");
        assert!(!strength("my dog ate the red couch twice and then slept").warn);
    }
    /// Audit M4: ordinary words outside the built-in list were scored as random letters, so two common words passed as 37 bits.
    #[test]
    fn ordinary_words_count_as_words_not_random_letters() {
        for weak in ["sunshine-princess", "sunshine princess 2024", "correct horse battery staple", "amber-fox-teal-47", "football-sunshine-1987", "Kathmandu-Elephant", "ThisIsMyPassword", "london-paris-berlin-rome"] {
            let s = strength(weak);
            assert!(!s.ok, "{weak} passed with {} bits", s.bits);
        }
        assert!(strength("sunshine-princess").bits < 32.0, "two common words are worth about two dictionary draws: {}", strength("sunshine-princess").bits);
        // The old three-word suggestion is below the floor, so every app from 0.3.6 on asks for more; a random string is still credited.
        assert!(strength("amber-fox-teal-47").bits < MIN_BITS);
        assert!(strength("xq7!Lp2#vB9wR4z&").ok, "{}", strength("xq7!Lp2#vB9wR4z&").bits);
    }
    #[test]
    fn wordlist_is_sorted_unique() {
        let w = words();
        assert!(w.len() >= 1000);
        assert!(w.windows(2).all(|p| p[0] < p[1]));
    }
}
