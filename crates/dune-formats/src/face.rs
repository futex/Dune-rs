//! Character close-up banks (LETO.HSQ, BARO.HSQ, PAUL.HSQ, ...): the talking faces.
//!
//! Reversed from `sub_191A0` / `sub_19908` / `sub_19BEE` / `sub_19D2D`:
//! a normal sprite sheet whose *last* table entry points to a metadata block M:
//! ```text
//! M+4   u16 x0, y0, x1, y1      face rectangle (origin of all part coordinates)
//! M+12  u16 rel                 S = M+12 + rel ; P = M+14 (part table)
//! P     u16 offsets (relative to P), part n (n >= 2) at P + P[n-2]:
//!         (u8 sprite (1-based), u8 x, u8 y)* 00
//! S     u16 offsets (relative to S): animation sequences = frames until FF; a frame is a
//!         byte list of part numbers terminated by 00 (byte 01 = escape: 0x100 | next byte).
//!         Each frame lists every part to draw over the clean background.
//!         Sequences 0..3 are idle animations (picked at random, sub_1994F).
//! ```

use crate::sprite::{DrawOpts, SpriteSheet};

pub struct FaceBank {
    pub sheet: SpriteSheet,
    data: Vec<u8>,
    pub rect: (u16, u16, u16, u16),
    parts_base: usize,
    seq_base: usize,
    /// Data of the last sequence entry: the mouth table (2-byte entries `part, 00`).
    pub mouth_table: usize,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

impl FaceBank {
    pub fn parse(data: Vec<u8>) -> Option<FaceBank> {
        let sheet = SpriteSheet::parse(&data)?;
        let t = u16_at(&data, 0)? as usize;
        let n = u16_at(&data, t)? as usize / 2;
        let m = t + u16_at(&data, t + 2 * (n - 1))? as usize;
        let rect = (u16_at(&data, m + 4)?, u16_at(&data, m + 6)?, u16_at(&data, m + 8)?, u16_at(&data, m + 10)?);
        let seq_base = m + 12 + u16_at(&data, m + 12)? as usize;
        let nseq = u16_at(&data, seq_base)? as usize;
        let mouth_table = seq_base + u16_at(&data, seq_base + nseq - 2)? as usize;
        Some(FaceBank { sheet, rect, parts_base: m + 14, seq_base, mouth_table, data })
    }

    pub fn sequence_count(&self) -> usize {
        u16_at(&self.data, self.seq_base).unwrap_or(0) as usize / 2
    }

    /// Sprites (0-based index, x, y relative to the rect origin) of the first frame of `seq`.
    pub fn frame(&self, seq: usize) -> Vec<(usize, u8, u8)> {
        self.sequence(seq).into_iter().next().unwrap_or_default()
    }

    /// All frames of sequence `seq` (frames end with 00, the sequence with FF).
    pub fn sequence(&self, seq: usize) -> Vec<Vec<(usize, u8, u8)>> {
        let mut frames = Vec::new();
        let Some(off) = u16_at(&self.data, self.seq_base + 2 * seq) else { return frames };
        let mut q = self.seq_base + off as usize;
        while self.data.get(q).is_some_and(|&b| b != 0xFF) && frames.len() < 256 {
            let (f, next) = self.frame_at(q);
            frames.push(f);
            q = next;
        }
        frames
    }

    /// Pointer to the data of sequence-table entry at byte offset `bp` (S + [S + bp]).
    pub fn seq_ptr(&self, bp: usize) -> usize {
        self.seq_base + u16_at(&self.data, self.seq_base + bp).unwrap_or(0) as usize
    }

    /// Byte at a frame pointer (FF = end of sequence).
    pub fn byte(&self, ptr: usize) -> u8 {
        self.data.get(ptr).copied().unwrap_or(0xFF)
    }

    /// Skips `n` 00-terminated frames (repne scasb loop of sub_19A60 / sub_1996C).
    pub fn skip_frames(&self, mut ptr: usize, n: usize) -> usize {
        for _ in 0..n {
            while self.data.get(ptr).is_some_and(|&b| b != 0) {
                ptr += 1;
            }
            ptr += 1;
        }
        ptr
    }

    /// Entries of the mouth-table entry `idx` (sub_19BEE on 47D2 + 2*idx).
    pub fn mouth_entries(&self, idx: usize) -> Vec<(usize, u8, u8)> {
        self.frame_at(self.mouth_table + 2 * idx).0
    }

    /// Decodes the frame at `q`: its entries and the pointer after its 00 (sub_19BEE).
    pub fn frame_at(&self, mut q: usize) -> (Vec<(usize, u8, u8)>, usize) {
        let mut out = Vec::new();
        while let Some(&b) = self.data.get(q) {
            q += 1;
            if b == 0 {
                break;
            }
            let part = if b == 1 {
                let v = 0x100 | *self.data.get(q).unwrap_or(&0) as usize;
                q += 1;
                v
            } else {
                b as usize
            };
            let Some(rel) = part.checked_sub(2).and_then(|i| u16_at(&self.data, self.parts_base + 2 * i)) else { continue };
            let mut r = self.parts_base + rel as usize;
            while let (Some(&s), Some(&x), Some(&y)) = (self.data.get(r), self.data.get(r + 1), self.data.get(r + 2)) {
                if s == 0 {
                    break;
                }
                out.push((s as usize - 1, x, y));
                r += 3;
            }
        }
        (out, q)
    }

    /// Draws frame 0 of sequence `seq`. `shift` is sub_109C7's dx: when the rect starts
    /// left of it, the rect is moved right by `shift`.
    pub fn draw(&self, fb: &mut [u8], fb_w: usize, seq: usize, shift: u16, clip: (i32, i32, i32, i32)) {
        self.draw_frame(fb, fb_w, &self.frame(seq), shift, clip);
    }

    pub fn draw_frame(&self, fb: &mut [u8], fb_w: usize, frame: &[(usize, u8, u8)], shift: u16, clip: (i32, i32, i32, i32)) {
        let x0 = if self.rect.0 < shift { self.rect.0 + shift } else { self.rect.0 } as i32;
        let y0 = self.rect.1 as i32;
        for &(s, x, y) in frame {
            if let Some(sp) = self.sheet.sprites.get(s) {
                sp.draw(fb, fb_w, x0 + x as i32, y0 + y as i32, DrawOpts { clip: Some(clip), ..Default::default() });
            }
        }
    }
}
