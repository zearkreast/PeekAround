//! MEGA crypto (mirrors `remote/crypto.ts` + the parts of megajs 1.3 the client needs):
//!
//! - a file node key is 32 bytes: `k[0..16] ⊕ k[16..32]` = AES-128 key ("folded"), `k[16..24]` = CTR
//!   nonce, `k[24..32]` = meta MAC; folder keys are 16 bytes and used as they are;
//! - file data = AES-128-CTR, IV = nonce(8) ‖ big-endian 64-bit block index → random access;
//! - node keys in listings and the master key = AES-128-ECB; attributes / file attributes =
//!   AES-128-CBC with a zero IV (zero padded);
//! - login: v2 = PBKDF2-HMAC-SHA512(password, salt, 100 000) (v1: the 65 536-round AES prepareKey),
//!   the session id is RSA-encrypted with the account key (MPI format, CRT private key).

use aes::cipher::{
    generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit, KeyIvInit, StreamCipher,
};
use aes::Aes128;
use base64::Engine;
use num_bigint::BigUint;

type Ctr = ctr::Ctr128BE<Aes128>;

/// base64url without padding (MEGA's `e64`)
pub fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Lenient base64 decode like Node's `Buffer.from(s, 'base64')`: standard or url-safe alphabet,
/// padding optional, stops at the first character outside the alphabet.
pub fn from_b64(s: &str) -> Vec<u8> {
    let clean: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_' | '='))
        .filter(|c| *c != '=')
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let mut t = clean;
    if t.len() % 4 == 1 {
        t.pop(); // a dangling 6 bits carry no byte
    }
    // Node's Buffer.from(…, 'base64') ignores non-zero trailing bits ("…Uv" of a 22-char key)
    const LENIENT: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    LENIENT.decode(t.as_bytes()).unwrap_or_default()
}

fn aes(key: &[u8]) -> Aes128 {
    Aes128::new(GenericArray::from_slice(&key[..16]))
}

/// AES key of a node key: 32 byte file keys are folded, 16 byte keys returned as they are.
pub fn fold_key(key: &[u8]) -> Result<[u8; 16], String> {
    let mut out = [0u8; 16];
    match key.len() {
        16 => out.copy_from_slice(key),
        n if n >= 32 => {
            for i in 0..16 {
                out[i] = key[i] ^ key[i + 16];
            }
        }
        _ => return Err("wrong key length".into()),
    }
    Ok(out)
}

/// CTR IV of the 16-byte block containing `offset`
pub fn ctr_iv(key: &[u8], offset: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[..8].copy_from_slice(&key[16..24]);
    iv[8..].copy_from_slice(&(offset / 16).to_be_bytes());
    iv
}

/// Decrypts a ciphertext stream that starts at file offset `aligned_start` (multiple of 16) and
/// yields exactly the plaintext bytes `[start, end]` (inclusive).
pub struct CtrRange {
    cipher: Ctr,
    skip: u64,
    left: u64,
}

impl CtrRange {
    pub fn new(key: &[u8], aligned_start: u64, start: u64, end: u64) -> Result<Self, String> {
        if aligned_start % 16 != 0 {
            return Err("alignedStart must be a multiple of 16".into());
        }
        if key.len() < 32 {
            return Err("wrong key length".into());
        }
        let k = fold_key(key)?;
        Ok(Self {
            cipher: Ctr::new(
                GenericArray::from_slice(&k),
                GenericArray::from_slice(&ctr_iv(key, aligned_start)),
            ),
            skip: start - aligned_start,
            left: end + 1 - start,
        })
    }

    /// Decrypt one chunk in place and return the part of it inside the range.
    pub fn update<'a>(&mut self, chunk: &'a mut [u8]) -> &'a [u8] {
        if self.left == 0 {
            return &chunk[..0];
        }
        self.cipher.apply_keystream(chunk);
        let s = self.skip.min(chunk.len() as u64) as usize;
        self.skip -= s as u64;
        let n = ((chunk.len() - s) as u64).min(self.left) as usize;
        self.left -= n as u64;
        &chunk[s..s + n]
    }

    pub fn remaining(&self) -> u64 {
        self.left
    }
}

/// encrypt/decrypt a buffer with CTR starting at file offset `offset` (tests, fakes)
pub fn ctr_xor(key: &[u8], data: &[u8], offset: u64) -> Vec<u8> {
    let aligned = offset - offset % 16;
    let k = fold_key(key).expect("file key");
    let mut c = Ctr::new(
        GenericArray::from_slice(&k),
        GenericArray::from_slice(&ctr_iv(key, aligned)),
    );
    let pad = (offset - aligned) as usize;
    let mut buf = vec![0u8; pad];
    buf.extend_from_slice(data);
    c.apply_keystream(&mut buf);
    buf.split_off(pad)
}

pub fn ecb_encrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let c = aes(key);
    let mut out = data.to_vec();
    for b in out.chunks_exact_mut(16) {
        c.encrypt_block(GenericArray::from_mut_slice(b));
    }
    out
}

pub fn ecb_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let c = aes(key);
    let mut out = data.to_vec();
    for b in out.chunks_exact_mut(16) {
        c.decrypt_block(GenericArray::from_mut_slice(b));
    }
    out
}

/// AES-128-CBC, zero IV, no padding (input length must be a multiple of 16; a tail is ignored)
pub fn cbc_encrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let c = aes(key);
    let mut prev = [0u8; 16];
    let mut out = Vec::with_capacity(data.len());
    for b in data.chunks_exact(16) {
        let mut x = [0u8; 16];
        for i in 0..16 {
            x[i] = b[i] ^ prev[i];
        }
        c.encrypt_block(GenericArray::from_mut_slice(&mut x));
        out.extend_from_slice(&x);
        prev = x;
    }
    out
}

pub fn cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let c = aes(key);
    let mut prev = [0u8; 16];
    let mut out = Vec::with_capacity(data.len());
    for b in data.chunks_exact(16) {
        let mut x = [0u8; 16];
        x.copy_from_slice(b);
        c.decrypt_block(GenericArray::from_mut_slice(&mut x));
        for i in 0..16 {
            x[i] ^= prev[i];
        }
        out.extend_from_slice(&x);
        prev.copy_from_slice(b);
    }
    out
}

fn zero_pad(b: &[u8]) -> Vec<u8> {
    let mut v = b.to_vec();
    v.resize(b.len().div_ceil(16) * 16, 0);
    v
}

/// Node attributes (`a` / `at`): `MEGA{json}` CBC-encrypted with the folded node key.
pub fn decrypt_attributes(
    node_key: &[u8],
    at: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let k = fold_key(node_key).ok()?;
    let plain = cbc_decrypt(&k, &from_b64(at));
    let end = plain.iter().position(|&b| b == 0).unwrap_or(plain.len());
    let s = std::str::from_utf8(&plain[..end]).ok()?;
    if !s.starts_with("MEGA{\"") {
        return None;
    }
    match serde_json::from_str(&s[4..]).ok()? {
        serde_json::Value::Object(m) => Some(m),
        _ => None,
    }
}

/// the inverse of [`decrypt_attributes`] (fake servers / tests)
pub fn encrypt_attributes(node_key: &[u8], attrs: &serde_json::Value) -> String {
    let k = fold_key(node_key).expect("node key");
    b64url(&cbc_encrypt(
        &k,
        &zero_pad(format!("MEGA{attrs}").as_bytes()),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAttrRef {
    /// storage cluster id
    pub cluster: u32,
    /// 0 = 120 px square thumbnail, 1 = preview (~1000 px, aspect kept), 8/9 = media info
    pub kind: u32,
    /// 8-byte attribute handle
    pub handle: [u8; 8],
}

/// "123:0*AbCdEfGhIjK/456:1*…" → refs (malformed parts are skipped)
pub fn parse_fa(fa: Option<&str>) -> Vec<FileAttrRef> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(\d+):(\d+)\*([A-Za-z0-9_-]{11})$").unwrap()
    });
    let Some(fa) = fa.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    fa.split('/')
        .filter_map(|part| {
            let c = RE.captures(part.trim())?;
            let h = from_b64(&c[3]);
            let handle: [u8; 8] = h.try_into().ok()?;
            Some(FileAttrRef {
                cluster: c[1].parse().ok()?,
                kind: c[2].parse().ok()?,
                handle,
            })
        })
        .collect()
}

/// cut the zero padding after a JPEG's EOI marker (other formats are returned unchanged)
fn trim_jpeg(b: Vec<u8>) -> Vec<u8> {
    if b.len() < 4 || b[0] != 0xff || b[1] != 0xd8 {
        return b;
    }
    let mut end = b.len();
    while end > 2 && b[end - 1] == 0 {
        end -= 1;
    }
    if end >= 2 && b[end - 2] == 0xff && b[end - 1] == 0xd9 {
        let mut b = b;
        b.truncate(end);
        b
    } else {
        b
    }
}

/// decrypt a downloaded file attribute (thumbnail / preview JPEG)
pub fn decrypt_file_attr(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let len = data.len() - data.len() % 16;
    if len == 0 {
        return Ok(Vec::new());
    }
    Ok(trim_jpeg(cbc_decrypt(&fold_key(key)?, &data[..len])))
}

/// encrypt a file attribute like the MEGA uploaders do (zero padded to 16) — tests and fakes
pub fn encrypt_file_attr(key: &[u8], plain: &[u8]) -> Vec<u8> {
    cbc_encrypt(&fold_key(key).expect("key"), &zero_pad(plain))
}

/// body of a file attribute POST answer: repeated `[8-byte handle][u32 LE length][data]`;
/// a truncated trailing part is ignored. Keys are the handles in hex, in order.
pub fn parse_file_attr_response(buf: &[u8]) -> Vec<([u8; 8], Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 12 <= buf.len() {
        let h: [u8; 8] = buf[p..p + 8].try_into().unwrap();
        let len = u32::from_le_bytes(buf[p + 8..p + 12].try_into().unwrap()) as usize;
        p += 12;
        if p + len > buf.len() {
            break;
        }
        out.push((h, buf[p..p + len].to_vec()));
        p += len;
    }
    out
}

pub fn build_file_attr_response(parts: &[([u8; 8], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (h, d) in parts {
        out.extend_from_slice(h);
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(d);
    }
    out
}

/// MEGA's chunked CBC-MAC over the plaintext (chunks of 128 KiB, 256 KiB, … growing by 128 KiB up
/// to 1 MiB, then 1 MiB each) condensed to the 8-byte meta MAC stored in `k[24..32]`.
pub struct MacState {
    cipher: Aes128,
    nonce: [u8; 8],
    mac: [u8; 16],
    macs: Vec<[u8; 16]>,
    pos: u64,
    pos_next: u64,
    increment: u64,
    pending: Vec<u8>,
    /// blocks absorbed since the last chunk boundary
    dirty: bool,
}

impl MacState {
    pub fn new(node_key: &[u8]) -> Result<Self, String> {
        if node_key.len() < 24 {
            return Err("wrong key length".into());
        }
        let k = fold_key(node_key)?;
        let mut nonce = [0u8; 8];
        nonce.copy_from_slice(&node_key[16..24]);
        Ok(Self::with(&k, nonce))
    }

    /// raw AES key + nonce (the uploader's view: key = k[0..16], nonce = k[16..24] of the 24 random bytes)
    pub fn with(aes_key: &[u8; 16], nonce: [u8; 8]) -> Self {
        let mut mac = [0u8; 16];
        mac[..8].copy_from_slice(&nonce);
        mac[8..].copy_from_slice(&nonce);
        Self {
            cipher: aes(aes_key),
            nonce,
            mac,
            macs: Vec::new(),
            pos: 0,
            pos_next: 131_072,
            increment: 131_072,
            pending: Vec::new(),
            dirty: false,
        }
    }

    fn block(&mut self, b: &[u8]) {
        for (m, x) in self.mac.iter_mut().zip(b) {
            *m ^= x;
        }
        self.cipher
            .encrypt_block(GenericArray::from_mut_slice(&mut self.mac));
        self.pos += 16;
        self.dirty = true;
        if self.pos >= self.pos_next {
            self.dirty = false;
            self.macs.push(self.mac);
            self.mac[..8].copy_from_slice(&self.nonce);
            self.mac[8..].copy_from_slice(&self.nonce);
            if self.increment < 1_048_576 {
                self.increment += 131_072;
            }
            self.pos_next += self.increment;
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        if !self.pending.is_empty() {
            let need = 16 - self.pending.len();
            let take = need.min(data.len());
            self.pending.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.pending.len() < 16 {
                return;
            }
            let b = std::mem::take(&mut self.pending);
            self.block(&b);
        }
        let full = data.len() / 16 * 16;
        for b in data[..full].chunks_exact(16) {
            self.block(b);
        }
        self.pending.extend_from_slice(&data[full..]);
    }

    fn condense(&self, macs: &[[u8; 16]]) -> [u8; 8] {
        let mut mac = [0u8; 16];
        for m in macs {
            for i in 0..16 {
                mac[i] ^= m[i];
            }
            self.cipher
                .encrypt_block(GenericArray::from_mut_slice(&mut mac));
        }
        let w = |i: usize| u32::from_be_bytes(mac[i..i + 4].try_into().unwrap());
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&(w(0) ^ w(4)).to_be_bytes());
        out[4..].copy_from_slice(&(w(8) ^ w(12)).to_be_bytes());
        out
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            let mut b = std::mem::take(&mut self.pending);
            b.resize(16, 0);
            self.block(&b);
        }
    }

    /// The 8-byte meta MAC: one chunk MAC per chunk that holds data (the MEGA clients' result).
    pub fn finish(mut self) -> [u8; 8] {
        self.flush();
        let mut macs = self.macs.clone();
        if self.dirty || macs.is_empty() {
            macs.push(self.mac);
        }
        self.condense(&macs)
    }

    /// Whether `expected` is this data's meta MAC. Also accepts megajs' variant, which appends
    /// the freshly reset chunk MAC when a file ends exactly on a chunk boundary.
    pub fn verify(mut self, expected: &[u8]) -> bool {
        self.flush();
        let mut macs = self.macs.clone();
        if self.dirty || macs.is_empty() {
            macs.push(self.mac);
            return self.condense(&macs) == expected;
        }
        if self.condense(&macs) == expected {
            return true;
        }
        macs.push(self.mac);
        self.condense(&macs) == expected
    }
}

/// Encrypt a whole file like a MEGA uploader: random 24 bytes (key + nonce) → CTR ciphertext and
/// the 32-byte node key (`key ⊕ nonce‖mac`, nonce, mac). Fakes and tests.
pub fn mega_encrypt(data: &[u8], seed: Option<[u8; 24]>) -> (Vec<u8>, [u8; 32]) {
    let r: [u8; 24] = seed.unwrap_or_else(rand::random);
    let k: [u8; 16] = r[..16].try_into().unwrap();
    let nonce: [u8; 8] = r[16..24].try_into().unwrap();
    let mut mac = MacState::with(&k, nonce);
    mac.update(data);
    let meta = mac.finish();
    let mut node_key = [0u8; 32];
    node_key[..16].copy_from_slice(&k);
    node_key[16..24].copy_from_slice(&nonce);
    node_key[24..].copy_from_slice(&meta);
    for i in 0..16 {
        node_key[i] ^= node_key[16 + i];
    }
    (ctr_xor(&node_key, data, 0), node_key)
}

/// megajs `prepareKey` (v1 accounts): 65 536 × AES over the password blocks.
pub fn prepare_key_v1(password: &[u8]) -> [u8; 16] {
    let mut pkey: [u8; 16] = [
        147, 196, 103, 227, 125, 176, 199, 164, 209, 190, 63, 129, 1, 82, 203, 86,
    ];
    let keys: Vec<Aes128> = password
        .chunks(16)
        .map(|c| {
            let mut k = [0u8; 16];
            // megajs copies 4-byte words only when the word starts inside the password
            for i in (0..16).step_by(4) {
                if i < c.len() {
                    let n = (c.len() - i).min(4);
                    k[i..i + n].copy_from_slice(&c[i..i + n]);
                }
            }
            aes(&k)
        })
        .collect();
    for _ in 0..65_536 {
        for k in &keys {
            k.encrypt_block(GenericArray::from_mut_slice(&mut pkey));
        }
    }
    pkey
}

/// megajs `AES.stringhash` (v1 login hash of the e-mail)
pub fn stringhash(key: &[u8; 16], s: &[u8]) -> [u8; 8] {
    let mut h32 = [0i32; 4];
    let mut i = 0;
    while i < s.len() {
        let len = (s.len() - i).min(4);
        let mut w = [0u8; 4];
        w[..len].copy_from_slice(&s[i..i + len]);
        h32[(i / 4) & 3] ^= i32::from_be_bytes(w);
        i += 4;
    }
    let mut hash = [0u8; 16];
    for (k, v) in h32.iter().enumerate() {
        hash[k * 4..k * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    let c = aes(key);
    for _ in 0..16_384 {
        c.encrypt_block(GenericArray::from_mut_slice(&mut hash));
    }
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&hash[..4]);
    out[4..].copy_from_slice(&hash[8..12]);
    out
}

/// v2 login key derivation: (AES key of the master key, `uh` login hash)
pub fn prepare_key_v2(password: &[u8], salt: &[u8]) -> ([u8; 16], String) {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, salt, 100_000, &mut out);
    (out[..16].try_into().unwrap(), b64url(&out[16..]))
}

/// One MPI: 16-bit big-endian bit count + big-endian bytes. Returns (number, rest).
pub fn read_mpi(b: &[u8]) -> Option<(BigUint, &[u8])> {
    if b.len() < 2 {
        return None;
    }
    let bits = u16::from_be_bytes([b[0], b[1]]) as usize;
    let len = bits.div_ceil(8);
    let body = b.get(2..2 + len)?;
    Some((BigUint::from_bytes_be(body), &b[2 + len..]))
}

pub fn write_mpi(n: &BigUint) -> Vec<u8> {
    let bytes = n.to_bytes_be();
    let mut v = (n.bits() as u16).to_be_bytes().to_vec();
    v.extend_from_slice(&bytes);
    v
}

/// RSA private key as MEGA stores it: p, q, d, u (= p⁻¹ mod q)
pub struct RsaPrivateKey {
    pub p: BigUint,
    pub q: BigUint,
    pub d: BigUint,
    pub u: BigUint,
}

impl RsaPrivateKey {
    /// the decrypted `privk` blob (4 MPIs + padding)
    pub fn from_privk(b: &[u8]) -> Option<Self> {
        let (p, r) = read_mpi(b)?;
        let (q, r) = read_mpi(r)?;
        let (d, r) = read_mpi(r)?;
        let (u, _) = read_mpi(r)?;
        Some(Self { p, q, d, u })
    }

    /// megajs `RSAdecrypt` (Garner / CRT)
    pub fn decrypt(&self, c: &BigUint) -> BigUint {
        let one = BigUint::from(1u8);
        let xp = (c % &self.p).modpow(&(&self.d % (&self.p - &one)), &self.p);
        let xq = (c % &self.q).modpow(&(&self.d % (&self.q - &one)), &self.q);
        let t = if xq >= xp {
            ((&xq - &xp) * &self.u) % &self.q
        } else {
            let t = ((&xp - &xq) * &self.u) % &self.q;
            (&self.q - t) % &self.q
        };
        t * &self.p + xp
    }
}

/// session id from the `csid` of a "us" answer: RSA-decrypt, first 43 bytes, base64url
pub fn session_id(privk: &RsaPrivateKey, csid: &[u8]) -> Option<String> {
    let (c, _) = read_mpi(csid)?;
    let m = privk.decrypt(&c).to_bytes_be();
    (m.len() >= 43).then(|| b64url(&m[..43]))
}

/// Answer to an `X-Hashcash` challenge ("1:<easiness>:<ts>:<token>") — megajs `generateHashcashToken`.
pub fn hashcash(challenge: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let parts: Vec<&str> = challenge.split(':').collect();
    if parts.len() < 4 || parts[0] != "1" {
        return None;
    }
    let easiness: u32 = parts[1].parse().ok()?;
    let base = ((easiness & 63) << 1) + 1;
    let shifts = (easiness >> 6) * 7 + 3;
    let threshold = (base as u64) << shifts;
    let token = from_b64(parts[3]);
    if token.len() != 48 {
        return None;
    }
    let mut buf = vec![0u8; 4 + 262_144 * 48];
    for i in 0..262_144 {
        buf[4 + i * 48..4 + (i + 1) * 48].copy_from_slice(&token);
    }
    loop {
        let d = Sha256::digest(&buf);
        if u32::from_be_bytes(d[..4].try_into().unwrap()) as u64 <= threshold {
            return Some(format!("1:{}:{}", parts[3], b64url(&buf[..4])));
        }
        let mut j = 0;
        loop {
            buf[j] = buf[j].wrapping_add(1);
            let carry = buf[j] == 0;
            j += 1;
            if !carry || j >= 4 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        (0u8..32).collect()
    }

    // vectors computed with `openssl enc` (same as packages/server/test/remote-folders.test.ts)
    #[test]
    fn folds_keys_and_builds_ivs() {
        assert_eq!(hex::encode(fold_key(&key()).unwrap()), "10".repeat(16));
        assert_eq!(
            hex::encode(ctr_iv(&key(), 0)),
            "10111213141516170000000000000000"
        );
        assert_eq!(
            hex::encode(ctr_iv(&key(), 16 * 0x0102 + 5)),
            "10111213141516170000000000000102"
        );
        assert!(fold_key(&[0; 20]).is_err());
    }

    #[test]
    fn file_attribute_vector() {
        let ct = hex::decode("82c0bc02c96142da6ccd036430b8cc20ce72409ce665615334d40e9b5b2098dbbfa34a6031035f3416d01aad3dea8d6c").unwrap();
        let pt = decrypt_file_attr(&key(), &ct).unwrap();
        assert_eq!(&pt[..33], b"PeekAround MEGA attribute vector!");
        assert_eq!(
            encrypt_file_attr(&key(), b"PeekAround MEGA attribute vector!"),
            ct
        );
    }

    #[test]
    fn ctr_random_access_vector() {
        let plain = b"0123456789abcdefPeekAround CTR!!";
        let ct = hex::decode("adb3ea5bc315626304eccbd0e47658c67bd2271b719dd12ffb1de916faf852a4")
            .unwrap();
        assert_eq!(ctr_xor(&key(), plain, 0), ct);
        for (s, e) in [(0u64, 31u64), (3, 20), (16, 16), (17, 31), (15, 16)] {
            let aligned = s - s % 16;
            let mut d = CtrRange::new(&key(), aligned, s, e).unwrap();
            let mut out = Vec::new();
            // feed the ciphertext in odd chunks
            for c in ct[aligned as usize..].chunks(5) {
                let mut c = c.to_vec();
                out.extend_from_slice(d.update(&mut c));
            }
            assert_eq!(out, &plain[s as usize..=e as usize], "{s}-{e}");
        }
        assert_eq!(ctr_xor(&key(), &plain[7..], 7), ct[7..]);
    }

    #[test]
    fn fa_strings_and_attr_posts() {
        let h0: [u8; 8] = hex::decode("0102030405060708").unwrap().try_into().unwrap();
        let h1: [u8; 8] = hex::decode("a1a2a3a4a5a6a7a8").unwrap().try_into().unwrap();
        let fa = parse_fa(Some(&format!(
            "924:0*{}/924:1*{}/bad/12:8*x",
            b64url(&h0),
            b64url(&h1)
        )));
        assert_eq!(
            fa,
            vec![
                FileAttrRef {
                    cluster: 924,
                    kind: 0,
                    handle: h0
                },
                FileAttrRef {
                    cluster: 924,
                    kind: 1,
                    handle: h1
                }
            ]
        );
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend((0..37).map(|i| (i * 13 + 1) as u8));
        jpeg.extend([0xff, 0xd9]);
        let body = build_file_attr_response(&[
            (h0, encrypt_file_attr(&key(), b"other")),
            (h1, encrypt_file_attr(&key(), &jpeg)),
        ]);
        let parts = parse_file_attr_response(&body);
        assert_eq!(parts.iter().map(|p| p.0).collect::<Vec<_>>(), vec![h0, h1]);
        assert_eq!(decrypt_file_attr(&key(), &parts[1].1).unwrap(), jpeg);
        assert_eq!(parse_file_attr_response(&body[..body.len() - 3]).len(), 1);
    }

    #[test]
    fn attributes_round_trip() {
        let k = key();
        let at = encrypt_attributes(&k, &serde_json::json!({ "n": "Ünï ✓.jpg" }));
        assert_eq!(decrypt_attributes(&k, &at).unwrap()["n"], "Ünï ✓.jpg");
        assert!(decrypt_attributes(&[7u8; 32], &at).is_none());
        assert_eq!(from_b64("AQID"), vec![1, 2, 3]);
        assert_eq!(from_b64("_-8"), vec![0xff, 0xef]);
        assert_eq!(from_b64("/+8="), vec![0xff, 0xef]);
    }

    #[test]
    fn mac_matches_megajs_vector() {
        // megajs encrypt() with the key 00..17 over 300 000 bytes of (i * 7) & 255 (see tests/data)
        let data: Vec<u8> = (0..300_000u32).map(|i| ((i * 7) & 255) as u8).collect();
        let seed: [u8; 24] = (0u8..24).collect::<Vec<_>>().try_into().unwrap();
        let (ct, k) = mega_encrypt(&data, Some(seed));
        let v = include_str!("../tests/data/megajs-encrypt-vector.txt");
        let mut lines = v.lines();
        assert_eq!(hex::encode(k), lines.next().unwrap().trim());
        assert_eq!(hex::encode(&ct[..32]), lines.next().unwrap().trim());
        assert_eq!(
            hex::encode(&ct[ct.len() - 32..]),
            lines.next().unwrap().trim()
        );
        let mut m = MacState::new(&k).unwrap();
        for c in data.chunks(1000) {
            m.update(c);
        }
        assert!(m.verify(&k[24..]));
    }

    #[test]
    fn v1_login_vectors() {
        let v = include_str!("../tests/data/megajs-v1-vector.txt");
        let mut lines = v.lines();
        let pk = prepare_key_v1("correct horse".as_bytes());
        assert_eq!(hex::encode(pk), lines.next().unwrap().trim());
        assert_eq!(
            b64url(&stringhash(&pk, b"peek.user@example.com")),
            lines.next().unwrap().trim()
        );
    }

    #[test]
    fn rsa_crt_and_mpi() {
        // small textbook key: p=61, q=53, e=17, d=2753
        let k = RsaPrivateKey {
            p: 61u32.into(),
            q: 53u32.into(),
            d: 2753u32.into(),
            u: BigUint::from(61u32).modinv(&BigUint::from(53u32)).unwrap(),
        };
        let m = BigUint::from(65u32);
        let c = m.modpow(&17u32.into(), &(61u32 * 53).into());
        assert_eq!(k.decrypt(&c), m);
        let n = BigUint::from(0x1_0203u32);
        let w = write_mpi(&n);
        assert_eq!(w, vec![0, 17, 1, 2, 3]);
        assert_eq!(read_mpi(&w).unwrap().0, n);
    }
}
