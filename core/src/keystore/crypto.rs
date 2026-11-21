//! The decryption a Java keystore needs, written out so the core crate takes on no new
//! dependency: AES, DES / 3DES and RC2 block decryption with CBC, HMAC, PBKDF2 and the
//! PKCS#12 key derivation. Only decryption, only what JKS / PKCS#12 files use; every piece
//! is checked against published test vectors (tests/apkscope-core/keystore.rs).

use sha1::Digest as _;

/// A hash: its function, output length and block length (for HMAC / the PKCS#12 KDF).
#[derive(Clone, Copy)]
pub struct Hash {
    pub f: fn(&[u8]) -> Vec<u8>,
    pub out: usize,
    pub block: usize,
}

pub const SHA1: Hash = Hash {
    f: |d| sha1::Sha1::digest(d).to_vec(),
    out: 20,
    block: 64,
};
pub const SHA256: Hash = Hash {
    f: |d| sha2::Sha256::digest(d).to_vec(),
    out: 32,
    block: 64,
};
pub const SHA384: Hash = Hash {
    f: |d| sha2::Sha384::digest(d).to_vec(),
    out: 48,
    block: 128,
};
pub const SHA512: Hash = Hash {
    f: |d| sha2::Sha512::digest(d).to_vec(),
    out: 64,
    block: 128,
};

pub fn hmac(h: Hash, key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = if key.len() > h.block {
        (h.f)(key)
    } else {
        key.to_vec()
    };
    k.resize(h.block, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&(h.f)(&inner));
    (h.f)(&outer)
}

/// PBKDF2 (RFC 8018) with HMAC over `h`.
pub fn pbkdf2(h: Hash, password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut block = 1u32;
    while out.len() < len {
        let mut s = salt.to_vec();
        s.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac(h, password, &s);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = hmac(h, password, &u);
            t.iter_mut().zip(&u).for_each(|(a, b)| *a ^= b);
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(len);
    out
}

/// The PKCS#12 key derivation (RFC 7292 appendix B.2). `password` is already the BMPString
/// (UTF-16BE plus two zero bytes); `id` 1 = key, 2 = IV, 3 = MAC key.
pub fn pkcs12_kdf(
    h: Hash,
    password: &[u8],
    salt: &[u8],
    id: u8,
    iterations: u32,
    n: usize,
) -> Vec<u8> {
    let v = h.block;
    let fill = |src: &[u8]| -> Vec<u8> {
        if src.is_empty() {
            return vec![];
        }
        let len = v * src.len().div_ceil(v);
        src.iter().copied().cycle().take(len).collect()
    };
    let mut i = fill(salt);
    i.extend(fill(password));
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut a = vec![id; v];
        a.extend_from_slice(&i);
        let mut a = (h.f)(&a);
        for _ in 1..iterations {
            a = (h.f)(&a);
        }
        out.extend_from_slice(&a);
        if out.len() >= n {
            break;
        }
        let b: Vec<u8> = a.iter().copied().cycle().take(v).collect();
        for chunk in i.chunks_mut(v) {
            // chunk = (chunk + b + 1) mod 2^(8v)
            let mut carry = 1u16;
            for k in (0..chunk.len()).rev() {
                let s = chunk[k] as u16 + b[k] as u16 + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
    }
    out.truncate(n);
    out
}

/// A block cipher's decryption of one block, in place.
pub trait Decrypt {
    const BLOCK: usize;
    fn decrypt(&self, block: &mut [u8]);
}

/// CBC decryption and PKCS#7 padding removal; None when the padding is wrong (a wrong
/// password almost always shows up here).
pub fn cbc_decrypt<C: Decrypt>(c: &C, iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let n = C::BLOCK;
    if data.is_empty() || !data.len().is_multiple_of(n) || iv.len() != n {
        return None;
    }
    let mut out = data.to_vec();
    let mut prev = iv.to_vec();
    for block in out.chunks_mut(n) {
        let saved = block.to_vec();
        c.decrypt(block);
        block.iter_mut().zip(&prev).for_each(|(a, b)| *a ^= b);
        prev = saved;
    }
    let pad = *out.last()? as usize;
    if pad == 0 || pad > n || !out[out.len() - pad..].iter().all(|&b| b as usize == pad) {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

// ───────────────────────────── AES ─────────────────────────────

fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0 }
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}

/// (S-box, inverse S-box), computed from the field inverse and the affine map instead of
/// typed in.
fn sboxes() -> &'static ([u8; 256], [u8; 256]) {
    static T: std::sync::OnceLock<([u8; 256], [u8; 256])> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut s = [0u8; 256];
        let mut inv = [0u8; 256];
        for x in 0..=255u8 {
            let i = if x == 0 {
                0
            } else {
                (1..=255u8).find(|&y| gmul(x, y) == 1).unwrap()
            };
            let mut r = i;
            for k in 1..5 {
                r ^= i.rotate_left(k);
            }
            r ^= 0x63;
            s[x as usize] = r;
            inv[r as usize] = x;
        }
        (s, inv)
    })
}

pub struct Aes {
    /// round keys, (rounds + 1) * 16 bytes
    keys: Vec<u8>,
    rounds: usize,
}

impl Aes {
    pub fn new(key: &[u8]) -> Option<Aes> {
        let nk = match key.len() {
            16 | 24 | 32 => key.len() / 4,
            _ => return None,
        };
        let rounds = nk + 6;
        let (s, _) = sboxes();
        let total = 4 * (rounds + 1);
        let mut w: Vec<[u8; 4]> = key.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut rcon = 1u8;
        for i in nk..total {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = [
                    s[t[1] as usize] ^ rcon,
                    s[t[2] as usize],
                    s[t[3] as usize],
                    s[t[0] as usize],
                ];
                rcon = xtime(rcon);
            } else if nk > 6 && i % nk == 4 {
                t = t.map(|b| s[b as usize]);
            }
            let prev = w[i - nk];
            w.push([
                prev[0] ^ t[0],
                prev[1] ^ t[1],
                prev[2] ^ t[2],
                prev[3] ^ t[3],
            ]);
        }
        Some(Aes {
            keys: w.concat(),
            rounds,
        })
    }
}

impl Decrypt for Aes {
    const BLOCK: usize = 16;
    fn decrypt(&self, b: &mut [u8]) {
        let (_, inv) = sboxes();
        let add = |b: &mut [u8], r: usize| {
            b.iter_mut()
                .zip(&self.keys[16 * r..16 * r + 16])
                .for_each(|(x, k)| *x ^= k)
        };
        // state is column-major: byte (row r, column c) at 4c + r
        let inv_shift = |b: &mut [u8]| {
            let t = b.to_vec();
            for c in 0..4 {
                for r in 0..4 {
                    b[4 * ((c + r) % 4) + r] = t[4 * c + r];
                }
            }
        };
        add(b, self.rounds);
        for round in (0..self.rounds).rev() {
            inv_shift(b);
            b.iter_mut().for_each(|x| *x = inv[*x as usize]);
            add(b, round);
            if round > 0 {
                for c in 0..4 {
                    let col = [b[4 * c], b[4 * c + 1], b[4 * c + 2], b[4 * c + 3]];
                    for r in 0..4 {
                        b[4 * c + r] = gmul(col[r], 14)
                            ^ gmul(col[(r + 1) % 4], 11)
                            ^ gmul(col[(r + 2) % 4], 13)
                            ^ gmul(col[(r + 3) % 4], 9);
                    }
                }
            }
        }
    }
}

// ───────────────────────────── DES / 3DES ─────────────────────────────

const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61,
    53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];
const FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];
const E: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];
const P: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];
const PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];
const PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];
const SHIFTS: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
const S: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1,
        13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

/// Bits of `x` (the low `n` bits, bit 1 = most significant) picked by `table`.
fn permute(x: u64, n: u32, table: &[u8]) -> u64 {
    table
        .iter()
        .fold(0u64, |acc, &p| acc << 1 | (x >> (n - p as u32)) & 1)
}

pub struct Des {
    subkeys: [u64; 16],
}

impl Des {
    pub fn new(key: &[u8]) -> Des {
        let k = u64::from_be_bytes(key[..8].try_into().unwrap());
        let cd = permute(k, 64, &PC1);
        let (mut c, mut d) = ((cd >> 28) as u32 & 0x0fff_ffff, cd as u32 & 0x0fff_ffff);
        let rot = |x: u32, s: u32| ((x << s) | (x >> (28 - s))) & 0x0fff_ffff;
        let mut subkeys = [0u64; 16];
        for (i, sk) in subkeys.iter_mut().enumerate() {
            c = rot(c, SHIFTS[i]);
            d = rot(d, SHIFTS[i]);
            *sk = permute((c as u64) << 28 | d as u64, 56, &PC2);
        }
        Des { subkeys }
    }

    fn f(r: u32, k: u64) -> u32 {
        let x = permute(r as u64, 32, &E) ^ k;
        let mut out = 0u32;
        for (i, sbox) in S.iter().enumerate() {
            let six = (x >> (42 - 6 * i)) & 0x3f;
            let row = ((six >> 4) & 2) | (six & 1);
            let col = (six >> 1) & 0xf;
            out = out << 4 | sbox[(row * 16 + col) as usize] as u32;
        }
        permute(out as u64, 32, &P) as u32
    }

    fn crypt(&self, block: &mut [u8], decrypt: bool) {
        let x = permute(u64::from_be_bytes(block[..8].try_into().unwrap()), 64, &IP);
        let (mut l, mut r) = ((x >> 32) as u32, x as u32);
        for i in 0..16 {
            let k = self.subkeys[if decrypt { 15 - i } else { i }];
            (l, r) = (r, l ^ Des::f(r, k));
        }
        let y = permute((r as u64) << 32 | l as u64, 64, &FP);
        block[..8].copy_from_slice(&y.to_be_bytes());
    }

    #[cfg(test)]
    pub fn encrypt_block(&self, block: &mut [u8]) {
        self.crypt(block, false)
    }
}

impl Decrypt for Des {
    const BLOCK: usize = 8;
    fn decrypt(&self, block: &mut [u8]) {
        self.crypt(block, true)
    }
}

/// Triple DES, EDE with three keys (24 bytes).
pub struct Des3(Des, Des, Des);

impl Des3 {
    pub fn new(key: &[u8]) -> Option<Des3> {
        (key.len() == 24).then(|| {
            Des3(
                Des::new(&key[..8]),
                Des::new(&key[8..16]),
                Des::new(&key[16..]),
            )
        })
    }
}

impl Decrypt for Des3 {
    const BLOCK: usize = 8;
    fn decrypt(&self, block: &mut [u8]) {
        self.2.crypt(block, true);
        self.1.crypt(block, false);
        self.0.crypt(block, true);
    }
}

// ───────────────────────────── RC2 ─────────────────────────────

const PITABLE: [u8; 256] = [
    0xd9, 0x78, 0xf9, 0xc4, 0x19, 0xdd, 0xb5, 0xed, 0x28, 0xe9, 0xfd, 0x79, 0x4a, 0xa0, 0xd8, 0x9d,
    0xc6, 0x7e, 0x37, 0x83, 0x2b, 0x76, 0x53, 0x8e, 0x62, 0x4c, 0x64, 0x88, 0x44, 0x8b, 0xfb, 0xa2,
    0x17, 0x9a, 0x59, 0xf5, 0x87, 0xb3, 0x4f, 0x13, 0x61, 0x45, 0x6d, 0x8d, 0x09, 0x81, 0x7d, 0x32,
    0xbd, 0x8f, 0x40, 0xeb, 0x86, 0xb7, 0x7b, 0x0b, 0xf0, 0x95, 0x21, 0x22, 0x5c, 0x6b, 0x4e, 0x82,
    0x54, 0xd6, 0x65, 0x93, 0xce, 0x60, 0xb2, 0x1c, 0x73, 0x56, 0xc0, 0x14, 0xa7, 0x8c, 0xf1, 0xdc,
    0x12, 0x75, 0xca, 0x1f, 0x3b, 0xbe, 0xe4, 0xd1, 0x42, 0x3d, 0xd4, 0x30, 0xa3, 0x3c, 0xb6, 0x26,
    0x6f, 0xbf, 0x0e, 0xda, 0x46, 0x69, 0x07, 0x57, 0x27, 0xf2, 0x1d, 0x9b, 0xbc, 0x94, 0x43, 0x03,
    0xf8, 0x11, 0xc7, 0xf6, 0x90, 0xef, 0x3e, 0xe7, 0x06, 0xc3, 0xd5, 0x2f, 0xc8, 0x66, 0x1e, 0xd7,
    0x08, 0xe8, 0xea, 0xde, 0x80, 0x52, 0xee, 0xf7, 0x84, 0xaa, 0x72, 0xac, 0x35, 0x4d, 0x6a, 0x2a,
    0x96, 0x1a, 0xd2, 0x71, 0x5a, 0x15, 0x49, 0x74, 0x4b, 0x9f, 0xd0, 0x5e, 0x04, 0x18, 0xa4, 0xec,
    0xc2, 0xe0, 0x41, 0x6e, 0x0f, 0x51, 0xcb, 0xcc, 0x24, 0x91, 0xaf, 0x50, 0xa1, 0xf4, 0x70, 0x39,
    0x99, 0x7c, 0x3a, 0x85, 0x23, 0xb8, 0xb4, 0x7a, 0xfc, 0x02, 0x36, 0x5b, 0x25, 0x55, 0x97, 0x31,
    0x2d, 0x5d, 0xfa, 0x98, 0xe3, 0x8a, 0x92, 0xae, 0x05, 0xdf, 0x29, 0x10, 0x67, 0x6c, 0xba, 0xc9,
    0xd3, 0x00, 0xe6, 0xcf, 0xe1, 0x9e, 0xa8, 0x2c, 0x63, 0x16, 0x01, 0x3f, 0x58, 0xe2, 0x89, 0xa9,
    0x0d, 0x38, 0x34, 0x1b, 0xab, 0x33, 0xff, 0xb0, 0xbb, 0x48, 0x0c, 0x5f, 0xb9, 0xb1, 0xcd, 0x2e,
    0xc5, 0xf3, 0xdb, 0x47, 0xe5, 0xa5, 0x9c, 0x77, 0x0a, 0xa6, 0x20, 0x68, 0xfe, 0x7f, 0xc1, 0xad,
];

#[cfg(test)]
pub fn pitable() -> &'static [u8; 256] {
    &PITABLE
}

/// RC2 (RFC 2268) with an effective key length in bits.
pub struct Rc2 {
    k: [u16; 64],
}

impl Rc2 {
    pub fn new(key: &[u8], effective_bits: usize) -> Rc2 {
        let t = key.len();
        let mut l = [0u8; 128];
        l[..t].copy_from_slice(key);
        for i in t..128 {
            l[i] = PITABLE[(l[i - 1] as usize + l[i - t] as usize) & 0xff];
        }
        let t8 = effective_bits.div_ceil(8);
        let tm = (0xffu32 >> (8 * t8 - effective_bits)) as u8;
        l[128 - t8] = PITABLE[(l[128 - t8] & tm) as usize];
        for i in (0..128 - t8).rev() {
            l[i] = PITABLE[(l[i + 1] ^ l[i + t8]) as usize];
        }
        let mut k = [0u16; 64];
        for (i, w) in k.iter_mut().enumerate() {
            *w = u16::from_le_bytes([l[2 * i], l[2 * i + 1]]);
        }
        Rc2 { k }
    }
}

impl Decrypt for Rc2 {
    const BLOCK: usize = 8;
    fn decrypt(&self, b: &mut [u8]) {
        let mut r = [0u16; 4];
        for (i, w) in r.iter_mut().enumerate() {
            *w = u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
        }
        let mut j: i32 = 63;
        let s = [1u32, 2, 3, 5];
        let rmix = |r: &mut [u16; 4], j: &mut i32| {
            for i in (0..4).rev() {
                r[i] = r[i].rotate_right(s[i]);
                r[i] = r[i]
                    .wrapping_sub(self.k[*j as usize])
                    .wrapping_sub(r[(i + 3) % 4] & r[(i + 2) % 4])
                    .wrapping_sub(!r[(i + 3) % 4] & r[(i + 1) % 4]);
                *j -= 1;
            }
        };
        let rmash = |r: &mut [u16; 4]| {
            for i in (0..4).rev() {
                r[i] = r[i].wrapping_sub(self.k[(r[(i + 3) % 4] & 63) as usize]);
            }
        };
        for _ in 0..5 {
            rmix(&mut r, &mut j);
        }
        rmash(&mut r);
        for _ in 0..6 {
            rmix(&mut r, &mut j);
        }
        rmash(&mut r);
        for _ in 0..5 {
            rmix(&mut r, &mut j);
        }
        for (i, w) in r.iter().enumerate() {
            b[2 * i..2 * i + 2].copy_from_slice(&w.to_le_bytes());
        }
    }
}
