//! Minimal AES-128 (FIPS-197) + CBC mode, vendored so HLS AES-128 works
//! with zero new dependencies (offline builds keep working).
//!
//! Only what streaming decryption needs: block encrypt (test fixtures),
//! block decrypt, CBC decrypt. Verified against FIPS-197 Appendix B and
//! NIST SP 800-38A vectors in the tests below. Clean-room implementation
//! from the published standard — no third-party AES code.
//!
//! The S-boxes are COMPUTED (Galois inverse + affine map), never hand-copied,
//! so table typos are impossible by construction.

const fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

/// AES S-box byte: multiplicative inverse in GF(2^8) then affine transform.
const fn sbox_byte(x: u8) -> u8 {
    let mut inv = 0u8;
    if x != 0 {
        // x^254 by square-and-multiply.
        let mut e = 254u32;
        let mut base = x;
        let mut r = 1u8;
        while e > 0 {
            if e & 1 == 1 {
                r = gf_mul(r, base);
            }
            base = gf_mul(base, base);
            e >>= 1;
        }
        inv = r;
    }
    let mut y = 0u8;
    let mut i = 0u32;
    while i < 8 {
        let bit = ((inv >> i) & 1)
            ^ ((inv >> ((i + 4) % 8)) & 1)
            ^ ((inv >> ((i + 5) % 8)) & 1)
            ^ ((inv >> ((i + 6) % 8)) & 1)
            ^ ((inv >> ((i + 7) % 8)) & 1)
            ^ ((0x63u8 >> i) & 1);
        y |= bit << i;
        i += 1;
    }
    y
}

const fn build_sbox() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        t[i] = sbox_byte(i as u8);
        i += 1;
    }
    t
}

const fn build_sbox_inv() -> [u8; 256] {
    let fwd = build_sbox();
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        t[fwd[i] as usize] = i as u8;
        i += 1;
    }
    t
}

const SBOX: [u8; 256] = build_sbox();
const SBOX_INV: [u8; 256] = build_sbox_inv();

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

/// Expand a 16-byte key into 11 round keys (176 bytes), column-major.
fn expand(key: &[u8; 16]) -> [u8; 176] {
    let mut w = [0u8; 176];
    w[..16].copy_from_slice(key);
    let mut i = 16;
    while i < 176 {
        let mut t = [w[i - 4], w[i - 3], w[i - 2], w[i - 1]];
        if i % 16 == 0 {
            // RotWord + SubWord + Rcon.
            let u = t[0];
            t[0] = SBOX[t[1] as usize] ^ RCON[i / 16 - 1];
            t[1] = SBOX[t[2] as usize];
            t[2] = SBOX[t[3] as usize];
            t[3] = SBOX[u as usize];
        }
        w[i] = w[i - 16] ^ t[0];
        w[i + 1] = w[i - 15] ^ t[1];
        w[i + 2] = w[i - 14] ^ t[2];
        w[i + 3] = w[i - 13] ^ t[3];
        i += 4;
    }
    w
}

fn add_key(state: &mut [u8; 16], rk: &[u8]) {
    for i in 0..16 {
        state[i] ^= rk[i];
    }
}

fn sub(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

fn inv_sub(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = SBOX_INV[*b as usize];
    }
}

/// ShiftRows left (encrypt): row r rotates left by r.
fn shift(state: &mut [u8; 16]) {
    let s = *state;
    for r in 0..4 {
        for c in 0..4 {
            state[r + 4 * c] = s[r + 4 * ((c + r) % 4)];
        }
    }
}

/// ShiftRows right (decrypt): row r rotates right by r.
fn inv_shift(state: &mut [u8; 16]) {
    let s = *state;
    for r in 0..4 {
        for c in 0..4 {
            state[r + 4 * c] = s[r + 4 * ((c + 4 - r) % 4)];
        }
    }
}

fn mix_col(s: &mut [u8; 4]) {
    let (a0, a1, a2, a3) = (s[0], s[1], s[2], s[3]);
    s[0] = gmul(a0, 2) ^ gmul(a1, 3) ^ a2 ^ a3;
    s[1] = a0 ^ gmul(a1, 2) ^ gmul(a2, 3) ^ a3;
    s[2] = a0 ^ a1 ^ gmul(a2, 2) ^ gmul(a3, 3);
    s[3] = gmul(a0, 3) ^ a1 ^ a2 ^ gmul(a3, 2);
}

fn inv_mix_col(s: &mut [u8; 4]) {
    let (a0, a1, a2, a3) = (s[0], s[1], s[2], s[3]);
    s[0] = gmul(a0, 0x0e) ^ gmul(a1, 0x0b) ^ gmul(a2, 0x0d) ^ gmul(a3, 0x09);
    s[1] = gmul(a0, 0x09) ^ gmul(a1, 0x0e) ^ gmul(a2, 0x0b) ^ gmul(a3, 0x0d);
    s[2] = gmul(a0, 0x0d) ^ gmul(a1, 0x09) ^ gmul(a2, 0x0e) ^ gmul(a3, 0x0b);
    s[3] = gmul(a0, 0x0b) ^ gmul(a1, 0x0d) ^ gmul(a2, 0x09) ^ gmul(a3, 0x0e);
}

fn mix(state: &mut [u8; 16]) {
    for c in 0..4 {
        let mut col = [state[c * 4], state[c * 4 + 1], state[c * 4 + 2], state[c * 4 + 3]];
        mix_col(&mut col);
        state[c * 4..c * 4 + 4].copy_from_slice(&col);
    }
}

fn inv_mix(state: &mut [u8; 16]) {
    for c in 0..4 {
        let mut col = [state[c * 4], state[c * 4 + 1], state[c * 4 + 2], state[c * 4 + 3]];
        inv_mix_col(&mut col);
        state[c * 4..c * 4 + 4].copy_from_slice(&col);
    }
}

/// One AES-128 block encrypt (CTR mode's key stream: Mega files).
pub fn encrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let w = expand(key);
    let mut s = *block;
    add_key(&mut s, &w[0..16]);
    for round in 1..10 {
        sub(&mut s);
        shift(&mut s);
        mix(&mut s);
        add_key(&mut s, &w[round * 16..round * 16 + 16]);
    }
    sub(&mut s);
    shift(&mut s);
    add_key(&mut s, &w[160..176]);
    s
}

/// One AES-128 block decrypt (inverse cipher: mirrors encryption).
pub fn decrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let w = expand(key);
    let mut s = *block;
    add_key(&mut s, &w[160..176]);
    for round in (1..10).rev() {
        inv_shift(&mut s);
        inv_sub(&mut s);
        add_key(&mut s, &w[round * 16..round * 16 + 16]);
        inv_mix(&mut s);
    }
    inv_shift(&mut s);
    inv_sub(&mut s);
    add_key(&mut s, &w[0..16]);
    s
}

/// AES-128-CBC encrypt (test fixtures only).
#[cfg(test)]
pub fn cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut buf = data.to_vec();
    while buf.len() % 16 != 0 {
        buf.push(0);
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut prev = *iv;
    for chunk in buf.chunks_exact(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev[i];
        }
        let ct = encrypt_block(key, &block);
        out.extend_from_slice(&ct);
        prev = ct;
    }
    out
}

/// AES-128-CBC decrypt (raw blocks, no unpadding — HLS segments are
/// block-aligned by the packager).
pub fn cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err("input is not block-aligned".to_string());
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = *iv;
    for chunk in data.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let mut pt = decrypt_block(key, &block);
        for i in 0..16 {
            pt[i] ^= prev[i];
        }
        out.extend_from_slice(&pt);
        prev = block;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn zero_vector() {
        // The most quoted AES-128 vector: key=0, pt=0.
        let z = [0u8; 16];
        let want = hex("66e94bd4ef8a2c3b884cfa59ca342b2e");
        assert_eq!(&encrypt_block(&z, &z)[..], &want[..]);
        assert_eq!(&decrypt_block(&z, &want.try_into().unwrap())[..], &z[..]);
    }

    #[test]
    fn fips197_appendix_b() {
        // FIPS-197 B: key 000102..0f, pt 001122..eeff → ct 69c4e0d8...
        let key: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let pt: [u8; 16] = hex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let want = hex("69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(&encrypt_block(&key, &pt)[..], &want[..]);
        assert_eq!(&decrypt_block(&key, &want.try_into().unwrap())[..], &pt[..]);
    }

    #[test]
    fn nist_sp800_38a_f21_cbc() {
        // F.2.1 CBC-AES128 decrypt vector.
        let key: [u8; 16] = hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let ct = hex("7649abac8119b246cee98e9b12e9197d");
        let want = hex("6bc1bee22e409f96e93d7e117393172a");
        assert_eq!(cbc_decrypt(&key, &iv, &ct).unwrap(), want);
        // Roundtrip over two blocks.
        let pt2 = [ct.clone(), want.clone()].concat();
        let enc = cbc_encrypt(&key, &iv, &pt2);
        assert_eq!(cbc_decrypt(&key, &iv, &enc).unwrap(), pt2);
        assert!(cbc_decrypt(&key, &iv, &ct[..15]).is_err());
    }
}
