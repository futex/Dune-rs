//! HNM: Cryo's video format (the "HNM1" flavour used by Dune CD).
//!
//! ```text
//! u16          header size H
//! palette      chunk list terminated by FF FF (see `palette`)
//! FF*          padding
//! u32[]        frame offsets relative to H, up to H; the last one is the end
//! frames...
//! ```
//! A frame is `u16 size` followed by optional tagged chunks
//! (`2 lowercase chars`, `u16 size including the 4-byte chunk header`):
//! * `pl` palette chunks
//! * `sd` 8-bit unsigned PCM audio
//!
//! and then the picture, which uses a sprite header:
//! ```text
//! u16 width (bits 0-8) | flags << 9
//!     flag 0x01 (0x0200): payload is HSQ compressed
//!     flag 0x02 (0x0400): full-screen picture without position prefix
//!     flag 0x40 (0x8000): pixels are RLE packed (sprite RLE)
//! u8  height
//! u8  mode: 0xFE opaque, 0xFF transparent (index 0 is skipped)
//! payload: [u16 x, u16 y] (unless flag 0x02), 8bpp pixels (RLE or raw)
//! ```
//! A picture of width 0 means "keep the previous frame".

use crate::{hsq, palette::Palette, sprite::unrle};

pub struct Hnm {
    data: Vec<u8>,
    pub palette: Palette,
    frames: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct Picture {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub transparent: bool,
    /// Full-screen picture (flag 0x400): the game decompresses it straight to the draw
    /// target at offset 0, without the driver's drawing origin.
    pub full: bool,
    pub pixels: Vec<u8>,
}

#[derive(Default, Debug, Clone)]
pub struct Frame {
    /// Palette chunk data to apply before showing this frame.
    pub palette: Option<Vec<u8>>,
    /// Unsigned 8-bit mono PCM.
    pub sound: Option<Vec<u8>>,
    pub picture: Option<Picture>,
}

impl Hnm {
    pub fn parse(data: Vec<u8>) -> Option<Hnm> {
        if data.len() < 8 {
            return None;
        }
        let hs = u16::from_le_bytes([data[0], data[1]]) as usize;
        if hs > data.len() {
            return None;
        }
        let mut palette = Palette::black();
        let mut p = 2 + palette.apply_chunks(&data[2..hs]);
        while p < hs && data[p] == 0xFF {
            p += 1;
        }
        let rd = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize + hs;
        let n = (hs - p) / 4;
        let offs: Vec<usize> = (0..n).map(|i| rd(p + 4 * i)).collect();
        if offs.iter().any(|&o| o > data.len()) {
            return None;
        }
        let frames = offs.windows(2).map(|w| (w[0], w[1])).collect();
        Some(Hnm { data, palette, frames })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn frame(&self, i: usize) -> Frame {
        let (start, end) = self.frames[i];
        let d = &self.data[start..end];
        let mut f = Frame::default();
        if d.len() < 2 {
            return f;
        }
        let size = (u16::from_le_bytes([d[0], d[1]]) as usize).min(d.len());
        let mut q = 2;
        while q + 4 <= size {
            let (a, b) = (d[q], d[q + 1]);
            if a.is_ascii_lowercase() && b.is_ascii_lowercase() {
                let cs = u16::from_le_bytes([d[q + 2], d[q + 3]]) as usize;
                if cs < 4 || q + cs > size {
                    break;
                }
                let body = d[q + 4..q + cs].to_vec();
                match &[a, b] {
                    b"pl" => f.palette = Some(body),
                    b"sd" => f.sound = Some(body),
                    _ => {}
                }
                q += cs;
                continue;
            }
            f.picture = decode_picture(&d[q..size]);
            break;
        }
        f
    }
}

fn decode_picture(d: &[u8]) -> Option<Picture> {
    let wf = u16::from_le_bytes([d[0], d[1]]);
    let width = wf & 0x1FF;
    let flags = wf >> 9;
    let height = d[2] as u16;
    let transparent = d[3] == 0xFF;
    if width == 0 || height == 0 {
        return None;
    }
    let body = &d[4..];
    let payload;
    let mut buf: &[u8] = if flags & 0x01 != 0 && hsq::header_ok(body) {
        payload = hsq::decompress(body).ok()?;
        &payload
    } else {
        body
    };
    let (mut x, mut y) = (0, 0);
    if flags & 0x02 == 0 && buf.len() >= 4 {
        x = u16::from_le_bytes([buf[0], buf[1]]);
        y = u16::from_le_bytes([buf[2], buf[3]]);
        buf = &buf[4..];
    }
    let n = width as usize * height as usize;
    let mut pixels = if flags & 0x40 != 0 { unrle(buf, n) } else { buf[..n.min(buf.len())].to_vec() };
    pixels.resize(n, 0);
    Some(Picture { x, y, width, height, transparent, full: flags & 0x02 != 0, pixels })
}

/// Composites pictures onto a persistent 320x200 framebuffer.
pub struct HnmPlayer {
    pub hnm: Hnm,
    pub palette: Palette,
    pub fb: Vec<u8>,
    pub next: usize,
}

pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 200;

impl HnmPlayer {
    pub fn new(hnm: Hnm) -> Self {
        let palette = hnm.palette.clone();
        HnmPlayer { hnm, palette, fb: vec![0; SCREEN_W * SCREEN_H], next: 0 }
    }

    pub fn done(&self) -> bool {
        self.next >= self.hnm.frame_count()
    }

    /// Decodes the next frame into `fb`. Returns the frame's audio, if any.
    pub fn step(&mut self) -> Option<Vec<u8>> {
        if self.done() {
            return None;
        }
        let f = self.hnm.frame(self.next);
        self.next += 1;
        if let Some(p) = &f.palette {
            self.palette.apply_chunks(p);
        }
        if let Some(pic) = &f.picture {
            blit_picture(&mut self.fb, pic);
        }
        f.sound
    }
}

pub fn blit_picture(fb: &mut [u8], pic: &Picture) {
    let (w, h) = (pic.width as usize, pic.height as usize);
    for sy in 0..h {
        let dy = pic.y as usize + sy;
        if dy >= SCREEN_H {
            break;
        }
        for sx in 0..w {
            let dx = pic.x as usize + sx;
            if dx >= SCREEN_W {
                break;
            }
            let c = pic.pixels[sy * w + sx];
            if !(pic.transparent && c == 0) {
                fb[dy * SCREEN_W + dx] = c;
            }
        }
    }
}
