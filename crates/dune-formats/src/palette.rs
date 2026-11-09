//! VGA palettes as stored by Cryo.
//!
//! A palette block is a list of chunks terminated by `FF FF`:
//! `u8 first, u8 count (0 = 256), count * (r, g, b)` with 6-bit components.

#[derive(Clone)]
pub struct Palette(pub [[u8; 3]; 256]);

impl Default for Palette {
    fn default() -> Self {
        // Greyscale fallback so that sprites are visible without a palette.
        let mut p = [[0u8; 3]; 256];
        for (i, c) in p.iter_mut().enumerate() {
            *c = [i as u8 >> 2; 3];
        }
        Palette(p)
    }
}

impl Palette {
    pub fn black() -> Self {
        Palette([[0; 3]; 256])
    }

    /// Applies a chunked palette block. Returns the number of bytes consumed.
    pub fn apply_chunks(&mut self, data: &[u8]) -> usize {
        let mut p = 0;
        loop {
            if p + 1 >= data.len() {
                return data.len().min(p + 2);
            }
            let (first, count) = (data[p], data[p + 1]);
            if first == 0xFF && count == 0xFF {
                return p + 2;
            }
            p += 2;
            let count = if count == 0 { 256 } else { count as usize };
            for i in 0..count {
                let idx = first as usize + i;
                if p + 3 > data.len() {
                    return data.len();
                }
                if idx < 256 {
                    self.0[idx] = [data[p] & 0x3F, data[p + 1] & 0x3F, data[p + 2] & 0x3F];
                }
                p += 3;
            }
        }
    }

    /// 8-bit RGB for an index (6-bit VGA value scaled to 0..=255).
    #[inline]
    pub fn rgb(&self, i: u8) -> [u8; 3] {
        let c = self.0[i as usize];
        [c[0] << 2 | c[0] >> 4, c[1] << 2 | c[1] >> 4, c[2] << 2 | c[2] >> 4]
    }

    #[inline]
    pub fn rgb_u32(&self, i: u8) -> u32 {
        let [r, g, b] = self.rgb(i);
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    }
}
