//! 320x200 indexed framebuffer + palette, the way the VGA mode 13h game sees it.

use dune_formats::palette::Palette;

pub const W: usize = 320;
pub const H: usize = 200;

pub struct Screen {
    pub fb: Vec<u8>,
    pub pal: Palette,
    /// 0 = black, 256 = full brightness (used for fades).
    pub brightness: u16,
}

impl Screen {
    pub fn new() -> Self {
        Screen { fb: vec![0; W * H], pal: Palette::black(), brightness: 256 }
    }

    pub fn clear(&mut self, c: u8) {
        self.fb.fill(c);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        for yy in y.max(0)..(y + h).min(H as i32) {
            for xx in x.max(0)..(x + w).min(W as i32) {
                self.fb[yy as usize * W + xx as usize] = c;
            }
        }
    }

    /// Converts to 0xAARRGGBB-ordered bytes (SDL ARGB8888 is BGRA in memory on LE).
    pub fn to_argb(&self, out: &mut [u8]) {
        let mut lut = [[0u8; 4]; 256];
        for (i, e) in lut.iter_mut().enumerate() {
            let [r, g, b] = self.pal.rgb(i as u8);
            let k = |v: u8| ((v as u32 * self.brightness as u32) >> 8) as u8;
            *e = [k(b), k(g), k(r), 255];
        }
        for (i, &c) in self.fb.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&lut[c as usize]);
        }
    }
}
