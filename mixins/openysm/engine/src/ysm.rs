//! Encrypted `.ysm` model files, crypto version 3: file hash, modified XChaCha20,
//! MT19937-64 mask, padding and obfuscated Zstandard. Ported from Sparkle-Morpher's
//! `YsmCrypt`, `XChaCha20`, `CityHash`, `MT19937` and `YsmZstd` (MIT, Copyright (c) 2026
//! Marciken). The result is the YSM binary model payload, which is parsed elsewhere.
use std::io::Read;
use std::num::Wrapping;

const SEED_KEY_DERIVATION: u64 = 0xD017_CBBA_7B5D_3581;
const SEED_FILE_VERIFICATION: u64 = 0x9E55_99DB_80C6_7C29;
const SEED_RES_VERIFICATION: u64 = 0xA62B_1A2C_4384_2BC3;
/// What an exported file starts with: a UTF-8 byte order mark and `YSGP`.
const MAGIC: &[u8] = b"\xef\xbb\xbfYSGP";

type W = Wrapping<u64>;
// CityHash64 with this format's own constants and a changed 128-to-64 mix.
const K0: W = Wrapping(0xE498_6A23_0E5A_AA17);
const K1: W = Wrapping(0x91AF_1080_2CAB_25A5);
const K2: W = Wrapping(0xAF29_CE77_8879_D9C7);
const K_MUL: W = Wrapping(0xDE0F_6EE0_9BDB_AB91);
fn fetch64(s: &[u8], at: usize) -> W {
    Wrapping(u64::from_le_bytes(s[at..at + 8].try_into().unwrap()))
}
fn fetch32(s: &[u8], at: usize) -> W {
    Wrapping(u64::from(u32::from_le_bytes(s[at..at + 4].try_into().unwrap())))
}
fn rotate(v: W, shift: u32) -> W {
    Wrapping(v.0.rotate_right(shift))
}
fn swap(v: W) -> W {
    Wrapping(v.0.swap_bytes())
}
fn shift_mix(v: W) -> W {
    v ^ (v >> 47)
}
fn hash_len16(u: W, v: W) -> W {
    let first = (u ^ v) * K_MUL;
    let second = (shift_mix(first) ^ u) * K_MUL;
    K_MUL * shift_mix(second)
}
fn hash_len16_mul(u: W, v: W, mul: W) -> W {
    let mut a = (u ^ v) * mul;
    a ^= a >> 47;
    let mut b = (v ^ a) * mul;
    b ^= b >> 47;
    b * mul
}
fn weak_hash32(s: &[u8], at: usize, a: W, b: W) -> (W, W) {
    let (w, x, y, z) = (fetch64(s, at), fetch64(s, at + 8), fetch64(s, at + 16), fetch64(s, at + 24));
    let mut a = a + w;
    let mut b = rotate(b + a + z, 21);
    let c = a;
    a += x;
    a += y;
    b += rotate(a, 44);
    (a + z, b + c)
}
fn city_hash64(s: &[u8]) -> W {
    let len = s.len();
    let n = Wrapping(len as u64);
    let mul = K2 + n * Wrapping(2);
    if len == 0 {
        return K2;
    }
    if len < 4 {
        let y = Wrapping(u64::from(s[0]) + (u64::from(s[len >> 1]) << 8));
        let z = Wrapping(len as u64 + (u64::from(s[len - 1]) << 2));
        return shift_mix((y * K2) ^ (z * K0)) * K2;
    }
    if len < 8 {
        return hash_len16_mul(n + (fetch32(s, 0) << 3), fetch32(s, len - 4), mul);
    }
    if len <= 16 {
        let a = fetch64(s, 0) + K2;
        let b = fetch64(s, len - 8);
        let c = rotate(b, 37) * mul + a;
        let d = (rotate(a, 25) + b) * mul;
        return hash_len16_mul(c, d, mul);
    }
    if len <= 32 {
        let a = fetch64(s, 0) * K1;
        let b = fetch64(s, 8);
        let c = fetch64(s, len - 8) * mul;
        let d = fetch64(s, len - 16) * K2;
        return hash_len16_mul(rotate(a + b, 43) + rotate(c, 30) + d, a + rotate(b + K2, 18) + c, mul);
    }
    if len <= 64 {
        let a = fetch64(s, 0) * K2;
        let b = fetch64(s, 8);
        let c = fetch64(s, len - 24);
        let d = fetch64(s, len - 32);
        let e = fetch64(s, 16) * K2;
        let f = fetch64(s, 24) * Wrapping(9);
        let g = fetch64(s, len - 8);
        let h = fetch64(s, len - 16) * mul;
        let u = rotate(a + g, 43) + (rotate(b, 30) + c) * Wrapping(9);
        let v = ((a + g) ^ d) + f + Wrapping(1);
        let w = swap((u + v) * mul) + h;
        let x = rotate(e + f, 42) + c;
        let y = (swap((v + w) * mul) + g) * mul;
        let z = e + f + c;
        let a = swap((x + z) * mul + y) + b;
        let b = shift_mix((z + a) * mul + d + h) * mul;
        return b + x;
    }
    // The end is hashed first; the loop then keeps v, w, x, y and z.
    let mut x = fetch64(s, len - 40);
    let mut y = fetch64(s, len - 16) + fetch64(s, len - 56);
    let mut z = hash_len16(fetch64(s, len - 48) + n, fetch64(s, len - 24));
    let mut v = weak_hash32(s, len - 64, n, z);
    let mut w = weak_hash32(s, len - 32, y + K1, x);
    x = x * K1 + fetch64(s, 0);
    let mut at = 0;
    let end = (len - 1) & !63;
    while at < end {
        x = rotate(x + y + v.0 + fetch64(s, at + 8), 37) * K1;
        y = rotate(y + v.1 + fetch64(s, at + 48), 42) * K1;
        x ^= w.1;
        y += v.0 + fetch64(s, at + 40);
        z = rotate(z + w.0, 33) * K1;
        v = weak_hash32(s, at, v.1 * K1, x + w.0);
        w = weak_hash32(s, at + 32, z + w.1, y + fetch64(s, at + 16));
        std::mem::swap(&mut x, &mut z);
        at += 64;
    }
    hash_len16(hash_len16(v.0, w.0) + shift_mix(y) * K1 + z, hash_len16(v.1, w.1) + x)
}
fn city_hash(s: &[u8], seed: u64) -> u64 {
    hash_len16(city_hash64(s) - K2, Wrapping(seed)).0
}
/// MT19937-64.
struct Twister {
    state: [u64; 312],
    index: usize,
}
impl Twister {
    fn new(seed: u64) -> Self {
        let mut state = [0u64; 312];
        state[0] = seed;
        for i in 1..312 {
            let previous = state[i - 1];
            state[i] = 6_364_136_223_846_793_005u64.wrapping_mul(previous ^ (previous >> 62)).wrapping_add(i as u64);
        }
        Self { state, index: 312 }
    }
    fn next(&mut self) -> u64 {
        if self.index >= 312 {
            for i in 0..312 {
                let x = (self.state[i] & 0xFFFF_FFFF_8000_0000) | (self.state[(i + 1) % 312] & 0x7FFF_FFFF);
                let mut twisted = x >> 1;
                if x & 1 != 0 {
                    twisted ^= 0xB502_6F5A_A966_19E9;
                }
                self.state[i] = self.state[(i + 156) % 312] ^ twisted;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= (y >> 29) & 0x5555_5555_5555_5555;
        y ^= (y << 17) & 0x71D6_7FFF_EDA6_0000;
        y ^= (y << 37) & 0xFFF7_EEE0_0000_0000;
        y ^ (y >> 43)
    }
}
/// XORs the twister's output, seeded from the 56 key and nonce bytes, over `data`.
fn mask(data: &mut [u8], key_iv: &[u8]) {
    let mut twister = Twister::new(city_hash(key_iv, SEED_KEY_DERIVATION));
    for chunk in data.chunks_mut(8) {
        for (byte, key) in chunk.iter_mut().zip(twister.next().to_le_bytes()) {
            *byte ^= key;
        }
    }
}
const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
fn double_rounds(x: &mut [u32; 16], rounds: u32) {
    fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(16);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(12);
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(8);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(7);
    }
    for _ in 0..rounds / 2 {
        quarter(x, 0, 4, 8, 12);
        quarter(x, 1, 5, 9, 13);
        quarter(x, 2, 6, 10, 14);
        quarter(x, 3, 7, 11, 15);
        quarter(x, 0, 5, 10, 15);
        quarter(x, 1, 6, 11, 12);
        quarter(x, 2, 7, 8, 13);
        quarter(x, 3, 4, 9, 14);
    }
}
/// XChaCha20 whose round count, in the key derivation too, is a parameter, and
/// whose state the file format rekeys between chunks.
struct Cipher {
    state: [u32; 16],
    rounds: u32,
}
impl Cipher {
    /// `key_iv`: 32 key bytes, then 24 nonce bytes.
    fn new(key_iv: &[u8], rounds: u32) -> Self {
        let word = |i: usize| u32::from_le_bytes(key_iv[i * 4..i * 4 + 4].try_into().unwrap());
        let mut x = [0u32; 16];
        x[..4].copy_from_slice(&SIGMA);
        for i in 0..12 {
            x[4 + i] = word(i);
        }
        double_rounds(&mut x, rounds);
        let mut state = [0u32; 16];
        state[..4].copy_from_slice(&SIGMA);
        state[4..8].copy_from_slice(&x[..4]);
        state[8..12].copy_from_slice(&x[12..]);
        state[14] = word(12);
        state[15] = word(13);
        Self { state, rounds }
    }
    /// Each call starts at a block boundary: the rest of a partial block's key stream is dropped.
    fn apply(&mut self, data: &mut [u8]) {
        for chunk in data.chunks_mut(64) {
            let mut block = self.state;
            double_rounds(&mut block, self.rounds);
            for (i, (word, start)) in block.iter().zip(self.state).enumerate() {
                let key = word.wrapping_add(start).to_le_bytes();
                for (byte, key) in chunk.iter_mut().skip(i * 4).zip(key) {
                    *byte ^= key;
                }
            }
            self.state[12] = self.state[12].wrapping_add(1);
            if self.state[12] == 0 {
                self.state[13] = self.state[13].wrapping_add(1);
            }
        }
    }
    /// Mixes the hash of the chunk just decrypted into the state; the next chunk's length.
    fn rekey(&mut self, hash: u64) -> usize {
        self.rounds = 10 * (hash % 3) as u32 + 10;
        for (i, word) in self.state.iter_mut().enumerate().skip(4) {
            *word ^= if i % 2 == 0 { hash as u32 } else { (hash >> 32) as u32 };
        }
        chunk_length(hash)
    }
}
fn chunk_length(hash: u64) -> usize {
    (((hash & 0x3F) | 0x40) << 6) as usize
}
/// Decrypts in place. Chunks are 4096 to 8128 bytes; each one's plaintext hash rekeys the next.
fn decrypt_chunks(data: &mut [u8], key_iv: &[u8], seed: u64) {
    let start = city_hash(key_iv, seed);
    let mut cipher = Cipher::new(key_iv, 10 * (start % 3) as u32 + 10);
    let mut length = chunk_length(start);
    let total = data.len();
    let mut at = 0;
    while at < total {
        let end = (at + length).min(total);
        let chunk = &mut data[at..end];
        cipher.apply(chunk);
        if end < total {
            length = cipher.rekey(city_hash(chunk, seed));
        }
        at = end;
    }
}
/// The files carry Zstandard frames whose block headers are rearranged and
/// masked; this writes the standard headers back.
fn restore_block_headers(frame: &mut [u8]) -> Result<(), String> {
    if frame.len() < 5 || frame[..4] != [0x28, 0xB5, 0x2F, 0xFD] {
        return Err("not a Zstandard frame".into());
    }
    let descriptor = frame[4];
    let single_segment = descriptor & 0x20 != 0;
    let dictionary = [0, 1, 2, 4][usize::from(descriptor & 3)];
    let content_size = match descriptor >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut at = 5 + usize::from(!single_segment) + dictionary + content_size;
    loop {
        let Some(header) = frame.get_mut(at..at + 3) else {
            return Err("truncated Zstandard frame".into());
        };
        let last = u32::from(header[0] >> 7);
        let kind = [2u32, 1, 3, 0][usize::from((header[0] >> 5) & 3)];
        let masked = (u32::from(header[0] & 0x1F) << 16) | u32::from(header[1]) | (u32::from(header[2]) << 8);
        let size = masked ^ 0xD4E9;
        let standard = last | (kind << 1) | (size << 3);
        header.copy_from_slice(&standard.to_le_bytes()[..3]);
        // A run-length block stores one byte, whatever its size says.
        at += 3 + if kind == 1 { 1 } else { size as usize };
        if at > frame.len() {
            return Err("truncated Zstandard frame".into());
        }
        if last == 1 {
            return Ok(());
        }
    }
}
/// Whether `bytes` start like an exported `.ysm` file.
pub fn is_ysm_file(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}
/// The YSM binary payload of an encrypted `.ysm` file, at most `max_output` bytes.
/// Only crypto version 3 is read; the file's hash is checked before anything else.
pub fn decrypt_file(file: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    // A text header, a zero byte, the crypto version, the data, 56 key bytes and a hash.
    if file.len() < 72 {
        return Err(".ysm file is too short".into());
    }
    let (hashed, hash) = file.split_at(file.len() - 8);
    if city_hash(hashed, SEED_FILE_VERIFICATION) != u64::from_le_bytes(hash.try_into().unwrap()) {
        return Err(".ysm file hash mismatch".into());
    }
    let (body, key_iv) = hashed.split_at(hashed.len() - 56);
    let header = body.iter().position(|byte| *byte == 0).ok_or(".ysm file has no header end")?;
    let Some(version) = body.get(header + 1..header + 5) else {
        return Err(".ysm file has no crypto version".into());
    };
    let version = u32::from_le_bytes(version.try_into().unwrap());
    if version != 3 {
        return Err(format!("unsupported .ysm crypto version {version}"));
    }
    let mut data = body[header + 5..].to_vec();
    decrypt_chunks(&mut data, key_iv, SEED_RES_VERIFICATION);
    mask(&mut data, key_iv);
    // Two bytes give the length of the random padding before the frame.
    if data.len() < 2 {
        return Err(".ysm file has no data".into());
    }
    let padding = usize::from(u16::from_le_bytes([data[0], data[1]]) & 0x3FF);
    let frame = data.get_mut(2 + padding..).ok_or(".ysm file padding exceeds its data")?;
    restore_block_headers(frame)?;
    let decoder = ruzstd::decoding::StreamingDecoder::new(&*frame).map_err(|error| format!("invalid .ysm data: {error}"))?;
    let mut output = Vec::new();
    decoder
        .take(max_output as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|error| format!("invalid .ysm data: {error}"))?;
    if output.len() > max_output {
        return Err(".ysm data exceeds the byte budget".into());
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    //! The expected values were recorded from Sparkle-Morpher's own classes
    //! (`target/ysm-golden` compiles them unchanged); the payloads are formulas.
    use super::*;
    const VECTORS: &str = include_str!("../tests/fixtures/ysm/vectors.json");
    fn lcg(seed: u64, length: usize) -> Vec<u8> {
        let mut x = seed;
        (0..length)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                (x >> 56) as u8
            })
            .collect()
    }
    fn hex(value: &serde_json::Value) -> u64 {
        u64::from_str_radix(value.as_str().unwrap(), 16).unwrap()
    }
    /// Signs a modified file again, to reach the checks behind the hash.
    fn sealed(mut file: Vec<u8>) -> Vec<u8> {
        let end = file.len() - 8;
        let hash = city_hash(&file[..end], SEED_FILE_VERIFICATION);
        file[end..].copy_from_slice(&hash.to_le_bytes());
        file
    }
    #[test]
    fn hash_and_twister_match_the_recorded_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let hashes = vectors["cityhash"].as_array().unwrap();
        assert_eq!(hashes.len(), 30);
        for entry in hashes {
            let length = entry[0].as_u64().unwrap() as usize;
            assert_eq!(city_hash(&lcg(7, length), SEED_RES_VERIFICATION), hex(&entry[1]), "length {length}");
        }
        assert_eq!(city_hash(&lcg(7, 500)[13..313], SEED_FILE_VERIFICATION), hex(&vectors["cityhash_window"]));
        let mut twister = Twister::new(0x0123_4567_89AB_CDEF);
        let outputs: Vec<u64> = (0..700).map(|_| twister.next()).collect();
        let recorded = vectors["mt19937"].as_array().unwrap();
        assert_eq!(recorded.len(), 7);
        for entry in recorded {
            assert_eq!(outputs[entry[0].as_u64().unwrap() as usize], hex(&entry[1]), "output {}", entry[0]);
        }
    }
    #[test]
    fn packet_cipher_matches_the_upstream_golden_packet() {
        // The same bytes as GOLDEN_PACKET_B64 in upstream's YsmCryptGoldenTest:
        // 30-round XChaCha20, then the twister mask, then a trailing hash.
        let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let text = vectors["packet"].as_str().unwrap();
        let packet: Vec<u8> = (0..text.len() / 2).map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let key_iv = [0x5Au8; 56];
        let (body, hash) = packet.split_at(packet.len() - 8);
        assert_eq!(city_hash(body, 0xEE6F_A63D_570B_D77B), u64::from_le_bytes(hash.try_into().unwrap()));
        let mut body = body.to_vec();
        mask(&mut body, &key_iv);
        Cipher::new(&key_iv, 30).apply(&mut body);
        assert_eq!(body, b"Hello YSM golden protocol");
    }
    #[test]
    fn recorded_files_decrypt_to_their_payloads() {
        let pattern = lcg(2, 1000);
        let repeat: Vec<u8> = (0..300_000).map(|i| pattern[i % 1000]).collect();
        let text: Vec<u8> = lcg(3, 6000).iter().map(|byte| b'a' + (byte & 0x0F)).collect();
        // One block; chunks rekeyed with raw blocks; several blocks; a Huffman-coded block.
        let cases: [(&[u8], Vec<u8>); 4] = [
            (include_bytes!("../tests/fixtures/ysm/tiny.ysm"), b"ysm binary payload \x00\x01\x02 magic".to_vec()),
            (include_bytes!("../tests/fixtures/ysm/random.ysm"), lcg(1, 20_000)),
            (include_bytes!("../tests/fixtures/ysm/repeat.ysm"), repeat),
            (include_bytes!("../tests/fixtures/ysm/text.ysm"), text),
        ];
        for (file, payload) in &cases {
            assert!(is_ysm_file(file));
            let decrypted = decrypt_file(file, payload.len()).unwrap();
            assert!(decrypted == *payload, "payload of {} bytes", payload.len());
            assert!(decrypt_file(file, payload.len() - 1).unwrap_err().contains("budget"));
        }
        assert!(!is_ysm_file(b"PK\x03\x04"));
    }
    #[test]
    fn damaged_files_are_errors() {
        let file: &[u8] = include_bytes!("../tests/fixtures/ysm/text.ysm");
        // Any changed byte fails the hash; so does every shorter file.
        for at in [0, 7, 8, 12, 500, file.len() - 60, file.len() - 9, file.len() - 1] {
            let mut damaged = file.to_vec();
            damaged[at] ^= 1;
            assert!(decrypt_file(&damaged, 1 << 20).unwrap_err().contains("hash"), "byte {at}");
        }
        for length in [0, 1, 71, 72, 100, file.len() - 1] {
            assert!(decrypt_file(&file[..length], 1 << 20).is_err(), "length {length}");
        }
        // Behind a valid hash: other crypto versions, and data that is not what the key expects.
        let header = file.iter().position(|byte| *byte == 0).unwrap();
        for version in [0u8, 1, 2, 4] {
            let mut other = file.to_vec();
            other[header + 1] = version;
            assert!(decrypt_file(&sealed(other), 1 << 20).unwrap_err().contains(&format!("crypto version {version}")));
        }
        let original = decrypt_file(file, 1 << 20).unwrap();
        for at in [header + 5, file.len() - 64, file.len() - 40, file.len() - 20] {
            let mut damaged = file.to_vec();
            damaged[at] ^= 0x40;
            assert!(decrypt_file(&sealed(damaged), 1 << 20).is_err(), "byte {at}");
        }
        // The frame has no checksum of its own: re-signed damage inside it is an
        // error or other bytes, never the original and never a panic.
        for at in (header + 140..file.len() - 64).step_by(97) {
            let mut damaged = file.to_vec();
            damaged[at] ^= 0x40;
            assert!(decrypt_file(&sealed(damaged), 1 << 20).map_or(true, |bytes| bytes != original), "byte {at}");
        }
        // The padding length is ten bits; the word's other six are random and unused.
        let mut unused = file.to_vec();
        unused[header + 6] ^= 0x40;
        assert!(decrypt_file(&sealed(unused), 1 << 20).unwrap() == original);
        // No header end, and nothing after it.
        assert!(decrypt_file(&sealed(vec![1; 80]), 1 << 20).unwrap_err().contains("header"));
        // Of 72 bytes only the first eight are not key or hash.
        let mut short = vec![1u8; 72];
        short[6] = 0;
        assert!(decrypt_file(&sealed(short), 1 << 20).unwrap_err().contains("crypto version"));
        let mut empty = vec![1u8; 76];
        empty[7] = 0;
        empty[8..12].copy_from_slice(&3u32.to_le_bytes());
        assert!(decrypt_file(&sealed(empty), 1 << 20).is_err());
    }
    #[test]
    fn block_headers_are_restored_within_bounds() {
        // Frame header: single segment, one content-size byte. One raw block of 3 bytes, last.
        let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, 3];
        let masked = 3u32 ^ 0xD4E9;
        frame.extend_from_slice(&[0x80 | (3 << 5) | (masked >> 16) as u8, masked as u8, (masked >> 8) as u8, b'a', b'b', b'c']);
        let mut restored = frame.clone();
        restore_block_headers(&mut restored).unwrap();
        assert_eq!(restored[6..9], [1 | (3 << 3), 0, 0]);
        let mut output = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(restored.as_slice()).unwrap().read_to_end(&mut output).unwrap();
        assert_eq!(output, b"abc");
        // A block that runs past the end, a missing last block and a wrong magic.
        assert!(restore_block_headers(&mut frame[..11].to_vec()).is_err());
        let mut open = frame.clone();
        open[6] &= 0x7F;
        assert!(restore_block_headers(&mut open).is_err());
        assert!(restore_block_headers(&mut [0x28, 0xB5, 0x2F, 0xFE, 0x20, 0, 0, 0, 0]).is_err());
        assert!(restore_block_headers(&mut [0x28, 0xB5, 0x2F]).is_err());
    }
}
