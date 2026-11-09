//! Day/night palette (docs/re/ui.md §4.2, animations.md §4.1) and the sietch water ripple
//! (animations.md §4.3).

use crate::assets::Assets;
use dune_formats::palette::Palette;

/// sub_1395C: SKY/SKYDN entry for the clock value t.
pub fn sky_index(assets: &Assets, t: u16) -> u8 {
    let tab = assets.exe_bytes(0x2280, 16).map(|b| b.to_vec()).unwrap_or_else(|| vec![8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 10, 10, 11, 11, 11]);
    tab[(t & 15) as usize].wrapping_add(((t >> 2) & 0x1C) as u8)
}

/// Colour ranges written by the sky palette: SKYDN 73..223 or SKY 128..207, plus 240..255.
pub fn ranges(dn: bool) -> [(usize, usize); 2] {
    if dn {
        [(73, 151), (240, 16)]
    } else {
        [(128, 80), (240, 16)]
    }
}

/// sub_13971 + sub_1398C: writes sky entry `idx` of SKYDN (`dn`) or SKY into `pal`.
pub fn apply(assets: &Assets, pal: &mut Palette, idx: u8, dn: bool) {
    let Ok(d) = assets.dat.read(if dn { "SKYDN.HSQ" } else { "SKY.HSQ" }) else { return };
    let rd = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let Some(t) = rd(0) else { return };
    let Some(e) = rd(t + 2 * idx as usize).map(|r| t + r + 6) else { return };
    let mut at = e;
    for (first, n) in ranges(dn) {
        for i in 0..n {
            if let Some(c) = d.get(at + 3 * i..at + 3 * i + 3) {
                pal.0[first + i] = [c[0] & 63, c[1] & 63, c[2] & 63];
            }
        }
        at += 3 * n;
    }
}

/// sub_138B4: the four SKY.HSQ bands (8 x 40 px each), into a 320-wide buffer.
pub fn draw_bands(assets: &mut Assets, fb: &mut [u8]) {
    let Some(sky) = assets.sheet("SKY.HSQ") else { return };
    let mut y = 0;
    for band in 0..4usize {
        if let Some(s) = sky.sprites.get(band) {
            let mut x = 0;
            while x < 320 {
                s.draw(fb, 320, x, y, dune_formats::sprite::DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
                x += 40;
            }
        }
        y += 20;
    }
}

/// Task 0x3916: the k=39 fade toward the new slot's sky palette (64 or 48 steps, 16 ticks).
#[derive(Clone)]
pub struct Fade {
    pub target: Palette,
    pub dn: bool,
    /// ds:46D7: remaining steps.
    pub n: u8,
    pub acc: u32,
}

impl Fade {
    /// One k=39 step on the sky ranges of `pal`; returns false when finished.
    pub fn step(&mut self, pal: &mut Palette) -> bool {
        for (first, count) in ranges(self.dn) {
            crate::transition::pal_step(pal, &self.target, first, count, self.n as i32);
        }
        self.n = self.n.saturating_sub(1);
        self.n > 0
    }
}

/// DNVGA 0x28EC: one quarter-point of the ripple ellipse per plot call, cycling.
pub struct Ripple {
    pub r: i32,
    pub s: i32,
    pub acc: u32,
}

impl Ripple {
    pub fn new() -> Ripple {
        Ripple { r: 8, s: 1, acc: 0 }
    }

    /// Task 0xC0B6 (fn 0x15A effect 0x0C, DNVGA 0x276C). `screen` is the displayed view,
    /// `back` the clean room. Returns true when the drip sound is due (returned cx == 0x18).
    pub fn step(&mut self, screen: &mut [u8], back: &[u8]) -> bool {
        let (r, s) = (self.r, self.s);
        if r != 8 {
            self.ellipse(r - 8, s - 1, screen, back, false);
        }
        self.ellipse(r, s, screen, back, true);
        self.r += 8;
        self.s += 1;
        if self.r >= 0x212 {
            self.r = 8;
            self.s = 1;
        }
        // The returned cx is the new r (the routine leaves cs:2768 in cx) [M]: the drip
        // plays on the second ring of each cycle.
        self.r == 0x18
    }

    fn plot(&mut self, x: u16, y: u16, screen: &mut [u8], back: &[u8], mosaic: bool) {
        // Plot filter 0x2823 / 0x2887.
        let mut y = y as i32;
        let a = 0x5B - y;
        if a >= 0 {
            y = 0x5B - (a >> 1);
        }
        if !(0..0x4E).contains(&(y - 0x47)) || x >= 0x140 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let at = |xx: usize, yy: usize| yy * 320 + xx;
        let inside = |xx: usize, yy: usize| xx < 320 && yy < 152;
        if mosaic {
            for dy in 0..4 {
                for dx in 0..4 {
                    let (xx, yy) = (x + dx, y + dy);
                    if !inside(xx, yy) || screen[at(xx, yy)] & 0x80 == 0 {
                        return;
                    }
                }
            }
            let v = back[at(x, y)];
            if v >= 0xF0 {
                return;
            }
            for dy in 0..4 {
                for dx in 0..4 {
                    screen[at(x + dx, y + dy)] = v;
                }
            }
        } else {
            for dy in 0..4 {
                for dx in 0..4 {
                    let (xx, yy) = (x + dx, y + dy);
                    if inside(xx, yy) && screen[at(xx, yy)] & 0x80 != 0 {
                        screen[at(xx, yy)] = back[at(xx, yy)];
                    }
                }
            }
        }
    }

    /// DNVGA 0x28EC (literal port of ripple.py): 32-bit midpoint ellipse centred at
    /// (176, 91); each plot emits one of the four symmetric points, cycling.
    fn ellipse(&mut self, r: i32, s: i32, screen: &mut [u8], back: &[u8], mosaic: bool) {
        const M: u64 = 0xFFFF_FFFF;
        let a = ((r as u64) << 8) & M;
        let mut e = (2 * a + 512) & M;
        let mut d = (2 * a).wrapping_sub(512) & M;
        let q = if s == 0 { 0 } else { (a / s as u64) & 0xFFFF };
        let mut k = ((q * q) >> 8) & M;
        let k2 = (2 * k) & M;
        let (cx0, cy0) = (0xB0i32, 0x5Bi32);
        let mut xl = (cx0 - r) as u16;
        let mut xr = (cx0 + r) as u16;
        let (mut yt, mut yb) = (cy0 as u16, cy0 as u16);
        let mut mode: u8 = 0;
        let mut plot = |this: &mut Ripple, xl: u16, xr: u16, yt: u16, yb: u16, screen: &mut [u8]| {
            mode = mode.wrapping_add(1);
            let (x, y) = match mode & 3 {
                0 => (xl, yb),
                1 => (xr, yb),
                2 => (xl, yt),
                _ => (xr, yt),
            };
            this.plot(x, y, screen, back, mosaic);
        };
        let mut guard = 0;
        // Region 1.
        loop {
            guard += 1;
            if guard > 20000 {
                return;
            }
            plot(self, xl, xr, yt, yb, screen);
            let borrow = e < k;
            e = e.wrapping_sub(k) & M;
            if !borrow {
                k = (k + k2) & M;
                yb = yb.wrapping_add(1);
                yt = yt.wrapping_sub(1);
                continue;
            }
            xl = xl.wrapping_add(1);
            xr = xr.wrapping_sub(1);
            let carry = e + d > M;
            e = (e + d) & M;
            d = d.wrapping_sub(512) & M;
            k = (k + k2) & M;
            if carry {
                yb = yb.wrapping_add(1);
                yt = yt.wrapping_sub(1);
                continue;
            }
            break;
        }
        // Region 2.
        loop {
            guard += 1;
            if guard > 40000 {
                return;
            }
            plot(self, xl, xr, yt, yb, screen);
            xl = xl.wrapping_add(1);
            xr = xr.wrapping_sub(1);
            let carry = e + d > M;
            e = (e + d) & M;
            if carry {
                yb = yb.wrapping_add(1);
                yt = yt.wrapping_sub(1);
                e = e.wrapping_sub(k) & M;
                k = (k + k2) & M;
            }
            d = d.wrapping_sub(512) & M;
            if d & 0x8000_0000 != 0 {
                break;
            }
        }
        plot(self, xl, xr, yt, yb, screen);
    }
}
