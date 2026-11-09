//! Creative Voice File (`.VOC`), as used for voices, sound effects and HNM audio.
//!
//! Dune voice lines carry a type-5 ("text") block in front of the audio. It holds
//! lip-sync data: a u16 followed by mouth-shape indices (0..=0x0C), `FF` terminated.

#[derive(Default, Debug, Clone)]
pub struct Voc {
    pub sample_rate: u32,
    /// Unsigned 8-bit mono PCM.
    pub pcm: Vec<u8>,
    /// Contents of the first type-5 block, if any (lip-sync track).
    pub lipsync: Option<Vec<u8>>,
}

pub const MAGIC: &[u8] = b"Creative Voice File\x1a";

pub fn is_voc(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

impl Voc {
    pub fn parse(data: &[u8]) -> Option<Voc> {
        if !is_voc(data) || data.len() < 26 {
            return None;
        }
        let mut p = u16::from_le_bytes([data[20], data[21]]) as usize;
        let mut v = Voc { sample_rate: 11111, ..Default::default() };
        while p < data.len() {
            let ty = data[p];
            if ty == 0 || p + 4 > data.len() {
                break;
            }
            let len = data[p + 1] as usize | (data[p + 2] as usize) << 8 | (data[p + 3] as usize) << 16;
            let body = &data[p + 4..(p + 4 + len).min(data.len())];
            match ty {
                1 if body.len() >= 2 => {
                    v.sample_rate = 1_000_000 / (256 - body[0] as u32);
                    v.pcm.extend_from_slice(&body[2..]);
                }
                2 => v.pcm.extend_from_slice(body),
                3 if body.len() >= 3 => {
                    // silence: u16 length - 1, u8 rate
                    let n = u16::from_le_bytes([body[0], body[1]]) as usize + 1;
                    v.pcm.extend(std::iter::repeat(0x80).take(n));
                }
                5 if v.lipsync.is_none() => v.lipsync = Some(body.to_vec()),
                _ => {}
            }
            p += 4 + len;
        }
        Some(v)
    }
}
