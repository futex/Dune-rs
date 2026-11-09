//! Cryo sprite sheets (most `.HSQ` image resources once decompressed).
//!
//! ```text
//! u16       offset of the sprite table (from file start)
//! ...       palette chunks (see `palette`), present when the offset > 2
//! table:    u16 offsets, relative to the table; count = first offset / 2
//! sprite:   u16 width (9 bits) | 0x8000 if RLE
//!           u8  height
//!           u8  palette offset; >= 0xFE means 8bpp, otherwise 4bpp and
//!               colour = nibble + offset (nibble 0 = transparent)
//!           data
//! ```
//! RLE: `n & 0x80` -> repeat next byte `257 - n` times, else copy `n + 1` bytes.
//! 4bpp rows (raw or RLE) are padded to 4 pixels.
//! Nibbles are low first.

use crate::palette::Palette;

#[derive(Clone, Debug)]
pub struct Sprite {
    pub width: u16,
    pub height: u16,
    pub pal_offset: u8,
    pub rle: bool,
    /// Palette indices, `width * height`, 0 = transparent.
    pub pixels: Vec<u8>,
}

impl Sprite {
    pub fn is_8bpp(&self) -> bool {
        self.pal_offset >= 0xFE
    }

    /// Decodes one sprite from its header + data.
    pub fn decode(data: &[u8]) -> Option<Sprite> {
        if data.len() < 4 {
            return None;
        }
        let w = u16::from_le_bytes([data[0], data[1]]);
        let rle = w & 0x8000 != 0;
        // Bits 9..14 are flags (0x2000 V-flip, 0x4000 H-flip), only set at draw time.
        let width = w & 0x1FF;
        let height = data[2] as u16;
        let pal_offset = data[3];
        let body = &data[4..];
        let bpp8 = pal_offset >= 0xFE;
        // 4bpp rows are padded to 4 pixels, raw or RLE (DNVGA driver).
        let stride = if bpp8 { width as usize } else { (width as usize + 3) / 4 * 2 };
        let need = stride * height as usize;
        let packed: Vec<u8> = if rle { unrle(body, need) } else { body[..need.min(body.len())].to_vec() };
        let mut pixels = vec![0u8; width as usize * height as usize];
        for y in 0..height as usize {
            let row = packed.get(y * stride..).unwrap_or(&[]);
            for x in 0..width as usize {
                let v = if bpp8 {
                    row.get(x).copied().unwrap_or(0)
                } else {
                    let b = row.get(x / 2).copied().unwrap_or(0);
                    let n = if x & 1 == 0 { b & 0xF } else { b >> 4 };
                    if n == 0 { 0 } else { n.wrapping_add(pal_offset) }
                };
                pixels[y * width as usize + x] = v;
            }
        }
        Some(Sprite { width, height, pal_offset, rle, pixels })
    }

    /// Blits onto an 8-bit framebuffer, skipping transparent (0) pixels.
    pub fn blit(&self, fb: &mut [u8], fb_w: usize, fb_h: usize, x: i32, y: i32, flip_x: bool) {
        let (w, h) = (self.width as i32, self.height as i32);
        for sy in 0..h {
            let dy = y + sy;
            if dy < 0 || dy >= fb_h as i32 {
                continue;
            }
            for sx in 0..w {
                let dx = x + sx;
                if dx < 0 || dx >= fb_w as i32 {
                    continue;
                }
                let src_x = if flip_x { w - 1 - sx } else { sx };
                let c = self.pixels[(sy * w + src_x) as usize];
                if c != 0 {
                    fb[dy as usize * fb_w + dx as usize] = c;
                }
            }
        }
    }
}

/// Options for [`Sprite::draw`], mirroring the game's sprite driver.
#[derive(Clone, Copy, Default)]
pub struct DrawOpts {
    pub hflip: bool,
    pub vflip: bool,
    /// Shrink factor in 1/256 (256 = 1:1). Values < 256 are treated as 256.
    pub scale: u16,
    /// Replaces the sprite's palette offset when non-zero (4bpp only).
    pub pal_override: u8,
    /// Clip rectangle (x0, y0, x1, y1), exclusive upper bounds.
    pub clip: Option<(i32, i32, i32, i32)>,
}

impl Sprite {
    pub fn draw(&self, fb: &mut [u8], fb_w: usize, x: i32, y: i32, o: DrawOpts) {
        let fb_h = fb.len() / fb_w;
        let (cx0, cy0, cx1, cy1) = o.clip.unwrap_or((0, 0, fb_w as i32, fb_h as i32));
        let scale = o.scale.max(256) as usize;
        let (w, h) = (self.width as usize, self.height as usize);
        let (wo, ho) = ((w << 8) / scale, (h << 8) / scale);
        let bpp8 = self.is_8bpp();
        for j in 0..ho {
            let sy = (j * scale) >> 8;
            let dy = y + if o.vflip { (ho - 1 - j) as i32 } else { j as i32 };
            if dy < cy0 || dy >= cy1 {
                continue;
            }
            for i in 0..wo {
                let sx = (i * scale) >> 8;
                let mut c = self.pixels[sy * w + sx];
                if c == 0 {
                    if !(bpp8 && self.pal_offset == 0xFE) {
                        continue;
                    }
                } else if !bpp8 && o.pal_override != 0 {
                    c = c.wrapping_sub(self.pal_offset).wrapping_add(o.pal_override);
                }
                let dx = x + if o.hflip { (wo - 1 - i) as i32 } else { i as i32 };
                if dx < cx0 || dx >= cx1 {
                    continue;
                }
                fb[dy as usize * fb_w + dx as usize] = c;
            }
        }
    }
}

pub fn unrle(src: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max);
    let mut p = 0;
    while p < src.len() && out.len() < max {
        let n = src[p];
        p += 1;
        if n & 0x80 != 0 {
            let v = src.get(p).copied().unwrap_or(0);
            p += 1;
            out.extend(std::iter::repeat(v).take(257 - n as usize));
        } else {
            let end = (p + n as usize + 1).min(src.len());
            out.extend_from_slice(&src[p..end]);
            p = end;
        }
    }
    out.truncate(max);
    out
}

#[derive(Clone, Default)]
pub struct SpriteSheet {
    /// Palette chunks embedded in the resource, applied over a base palette.
    pub palette_chunks: Vec<u8>,
    pub sprites: Vec<Sprite>,
}

impl SpriteSheet {
    pub fn parse(data: &[u8]) -> Option<SpriteSheet> {
        if data.len() < 4 {
            return None;
        }
        let table = u16::from_le_bytes([data[0], data[1]]) as usize;
        if table < 2 || table + 2 > data.len() {
            return None;
        }
        let first = u16::from_le_bytes([data[table], data[table + 1]]) as usize;
        let count = first / 2;
        if count == 0 || table + count * 2 > data.len() {
            return None;
        }
        let offs: Vec<usize> = (0..count)
            .map(|i| table + u16::from_le_bytes([data[table + 2 * i], data[table + 2 * i + 1]]) as usize)
            .collect();
        let mut sprites = Vec::with_capacity(count);
        for (i, &o) in offs.iter().enumerate() {
            let end = offs.get(i + 1).copied().unwrap_or(data.len()).max(o).min(data.len());
            if o >= data.len() {
                return None;
            }
            sprites.push(Sprite::decode(&data[o..end.max(o)]).unwrap_or(Sprite {
                width: 0,
                height: 0,
                pal_offset: 0,
                rle: false,
                pixels: vec![],
            }));
        }
        Some(SpriteSheet { palette_chunks: data[2..table].to_vec(), sprites })
    }

    /// Applies the embedded palette like the game's bank loader (sub_1C1BA): a chunk
    /// that sets only colour 0 (`00 01 r g b`) is skipped, so colour 0 stays as it was.
    pub fn apply_palette(&self, pal: &mut Palette) {
        let c = &self.palette_chunks;
        if c.len() >= 5 && c[0] == 0 && c[1] == 1 {
            let keep = pal.0[0];
            pal.apply_chunks(c);
            pal.0[0] = keep;
        } else if !c.is_empty() {
            pal.apply_chunks(c);
        }
    }
}
