//! Save games (`dune37s<N>.sav`), compatible with the original (sub_1B389 / sub_1B427 /
//! sub_1B4EA for saving, loc_1B3B0 / sub_1B4BB / sub_1B473 for loading).
//!
//! ```text
//! u16  game time (ds:2)
//! u16  F7 xx            RLE header: escape byte 0xF7 (high byte unused)
//! u16  size             bytes from the header start to the end marker
//! RLE  F7 count value | literal ...      then u16 0000
//! ```
//! Unpacked payload (0x567A bytes):
//! * 0x317F bytes: bits 5..4 of every byte of the map buffer (0xC5FC bytes), 4 per byte
//! * 0x00A2 bytes: code-segment variables cs:00AA..014B
//! * 0x11F8 bytes: ds:AA76.. (DIALOGUE.HSQ with its "already said" bits, and following data)
//! * 0x1261 bytes: ds:0000..1260 (game variables, characters, locations, rooms)

pub const MAP_SIZE: usize = 0xC5FC;
pub const MAP_PACKED: usize = MAP_SIZE / 4;
pub const CS_VARS: usize = 0xA2;
pub const DS_AA76: usize = 0x11F8;
pub const DS_LOW: usize = 0x1261;
pub const PAYLOAD: usize = MAP_PACKED + CS_VARS + DS_AA76 + DS_LOW;
const ESC: u8 = 0xF7;

#[derive(Clone)]
pub struct SaveGame {
    pub time: u16,
    pub map_bits: Vec<u8>,
    pub cs_vars: Vec<u8>,
    pub ds_aa76: Vec<u8>,
    pub ds_low: Vec<u8>,
}

/// sub_1B427 inner loop: 4 map bytes -> 1 byte (their bits 5..4, first byte in the top bits).
pub fn pack_map(map: &[u8]) -> Vec<u8> {
    map.chunks(4).map(|c| c.iter().fold(0u8, |acc, &b| acc << 2 | (b >> 4) & 3)).collect()
}

/// Inverse of [`pack_map`]: writes bits 5..4 back, keeping the other bits of `map`.
pub fn unpack_map(packed: &[u8], map: &mut [u8]) {
    for (i, &p) in packed.iter().enumerate() {
        for k in 0..4 {
            if let Some(b) = map.get_mut(i * 4 + k) {
                let bits = (p >> (6 - 2 * k)) & 3;
                *b = (*b & 0xCF) | bits << 4;
            }
        }
    }
}

fn rle_decode(src: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max);
    let mut i = 0;
    while i < src.len() && out.len() < max {
        if src[i] == ESC && i + 2 < src.len() {
            out.extend(std::iter::repeat(src[i + 2]).take(src[i + 1] as usize));
            i += 3;
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

/// sub_1B4EA: runs of up to 255; runs of 3+ (or any escape byte) are coded F7 n v.
fn rle_encode(src: &[u8]) -> Vec<u8> {
    let mut out = vec![ESC, 0, 0, 0];
    let mut i = 0;
    let mut last_run = 0u8;
    while i < src.len() {
        let v = src[i];
        let mut n = 1usize;
        while i + n < src.len() && src[i + n] == v && n < 0xFF {
            n += 1;
        }
        if v == ESC || n >= 3 {
            out.extend_from_slice(&[ESC, n as u8, v]);
        } else {
            out.extend(std::iter::repeat(v).take(n));
        }
        last_run = n as u8;
        i += n;
    }
    let size = out.len();
    out.extend_from_slice(&[0, 0]);
    out[1] = last_run;
    out[2..4].copy_from_slice(&(size as u16).to_le_bytes());
    out
}

impl SaveGame {
    pub fn parse(file: &[u8]) -> Option<SaveGame> {
        if file.len() < 8 || file[2] != ESC {
            return None;
        }
        let time = u16::from_le_bytes([file[0], file[1]]);
        let size = u16::from_le_bytes([file[4], file[5]]) as usize;
        let body = file.get(6..(2 + size).min(file.len()))?;
        let p = rle_decode(body, PAYLOAD);
        if p.len() < PAYLOAD {
            return None;
        }
        let (a, rest) = p.split_at(MAP_PACKED);
        let (b, rest) = rest.split_at(CS_VARS);
        let (c, d) = rest.split_at(DS_AA76);
        Some(SaveGame { time, map_bits: a.to_vec(), cs_vars: b.to_vec(), ds_aa76: c.to_vec(), ds_low: d[..DS_LOW].to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(PAYLOAD);
        payload.extend_from_slice(&self.map_bits);
        payload.extend_from_slice(&self.cs_vars);
        payload.extend_from_slice(&self.ds_aa76);
        payload.extend_from_slice(&self.ds_low);
        let mut out = self.time.to_le_bytes().to_vec();
        out.extend(rle_encode(&payload));
        out
    }
}
