//! The rotating planet (EXE seg000:B8A7/BA75/B9E0/BA2D + DNVGA fn 0x145/0x148).
//! Port of docs/re/globe.md; validated against STARS.HSQ sprite 35 (pixel-identical).
//!
//! Inputs: TABLAT.BIN (99 latitude records, big-endian words), GLOBDATA.HSQ (projection
//! tables), and the live map (MAP.HSQ): low nibble = relief shade, bits 4-5 = territory.
//! The disc is fixed at x 96..223, y 25..133.

const M16: i32 = 0xFFFF;
const MAP_BASE: i32 = 0x62FC;
const COLS: usize = 0xCDA;

fn s8(v: i32) -> i32 {
    let v = v & 0xFF;
    if v & 0x80 != 0 {
        v - 0x100
    } else {
        v
    }
}
fn s16(v: i32) -> i32 {
    let v = v & M16;
    if v & 0x8000 != 0 {
        v - 0x10000
    } else {
        v
    }
}

pub struct Globe {
    t: [[i32; 4]; 99],
    g: Vec<u8>,
    pub tilt: i32,
    tt: Vec<i32>,
    /// Territory colours (SEE RESULTS view).
    pub results: bool,
    line: Option<LineCursor>,
}

#[derive(Clone, Copy)]
struct LineCursor {
    half: u8,
    di: usize,
    row: i32,
    step: i32,
}

impl Globe {
    pub fn new(tablat: &[u8], globdata: Vec<u8>) -> Option<Globe> {
        if tablat.len() < 0x18C * 2 {
            return None;
        }
        let mut t = [[0i32; 4]; 99];
        for i in 0..99 {
            for k in 0..4 {
                let o = 2 * (4 * i + k);
                t[i][k] = u16::from_be_bytes([tablat[o], tablat[o + 1]]) as i32;
            }
        }
        Some(Globe { t, g: globdata, tilt: 0, tt: vec![0; 0xC4], results: false, line: None })
    }

    /// sub_1BA75(dx = 16-bit longitude ds:197C, bx = latitude ds:197E).
    pub fn set_view(&mut self, lon16: u16, lat: i16) {
        let lon = (0x18E * lon16 as i32) >> 16;
        self.t[0][2] = lon;
        self.t[0][3] = 0;
        let mut ax = lat as i32 & M16;
        if ax < 0x20 {
            ax = 0x20; // unsigned compares (sic)
        }
        if ax >= 0xFFE0 {
            ax = 0xFFE0;
        }
        self.tilt = s16(ax);
        self.scale(lon);
        self.tilt_table();
    }

    pub fn longitude(&self) -> i32 {
        self.t[0][2]
    }

    fn scale(&mut self, lon: i32) {
        let bx = ((((lon as u32) << 16) | 0x8000) / 0x18E) as u64;
        for i in 1..99 {
            let p = (self.t[i][1] as u64 * bx * 2) & 0xFFFF_FFFF;
            self.t[i][2] = (p >> 16) as i32;
            self.t[i][3] = (p & 0xFFFF) as i32;
        }
    }

    /// sub_1B9E0: rotate by `ax` pixels (wraps at 398).
    pub fn rotate(&mut self, ax: i32) {
        let mut dx = s16(self.t[0][2] + ax);
        if dx < 0 {
            dx += 0x18E;
        }
        if dx >= 0x18E {
            dx -= 0x18E;
        }
        self.t[0][2] = dx;
        self.scale(dx);
    }

    /// sub_1BA15: tilt += ax, clamped to +-0x62.
    pub fn add_tilt(&mut self, ax: i32) {
        self.tilt = (self.tilt + ax).clamp(-0x62, 0x62);
        self.tilt_table();
    }

    /// loc_1BA2D, exact emulation of the al/ax arithmetic.
    fn tilt_table(&mut self) {
        let mut out: Vec<i32> = Vec::new();
        let mut bx: i32 = 0x62;
        let mut cx: i32 = 0xC4;
        let mut ax: i32 = (self.tilt + bx) & M16;
        let setal = |ax: i32, v: i32| (ax & 0xFF00) | (v & 0xFF);
        if s16(ax) > bx {
            bx = -0x62;
            ax = setal(ax, s8(ax) + bx + bx);
            loop {
                ax = setal(ax, s8(ax) - 1);
                out.push(ax);
                if s8(ax) <= bx {
                    break;
                }
                cx -= 1;
                if cx == 0 {
                    break;
                }
            }
            bx = 0x62;
            ax = bx;
            cx -= 1;
        }
        loop {
            out.push(ax);
            ax = setal(ax, s8(ax) - 1);
            if s8(ax) < 0 {
                break;
            }
            cx -= 1;
            if cx == 0 {
                break;
            }
        }
        cx -= 1;
        if cx > 0 {
            ax = (-ax) & M16;
            loop {
                out.push(ax);
                ax = setal(ax, s8(ax) + 1);
                if s8(ax) > bx {
                    break;
                }
                cx -= 1;
                if cx == 0 {
                    break;
                }
            }
            cx -= 1;
            if cx > 0 {
                ax = setal(ax, -s8(ax));
                loop {
                    ax = setal(ax, s8(ax) + 1);
                    out.push(ax);
                    cx -= 1;
                    if cx == 0 {
                        break;
                    }
                }
            }
        }
        out.resize(0xC4, 0);
        self.tt = out;
    }

    fn tword(&self, idx: i32) -> i32 {
        self.tt.get((0x62 + idx) as usize).copied().unwrap_or(0)
    }

    fn colour(&self, v: u8) -> u8 {
        let v = v as i32;
        if !self.results {
            let (mut al, ah) = (v & 0x0F, v & 0x30);
            if ah == 0x10 && al < 8 {
                al += 12;
            }
            (al + 0x10) as u8
        } else {
            let (mut al, mut ah) = ((v & 0x0F) | 0x10, v & 0x30);
            if ah >= 0x10 {
                ah = ((ah - 0x10) >> 1) & 0x10;
                al = al + 0x10 + ah;
            }
            (al & 0xFF) as u8
        }
    }

    fn tword_t(&self, byte_off: i32) -> i32 {
        let i = (byte_off / 8) as usize;
        let k = ((byte_off % 8) / 2) as usize;
        self.t.get(i).map(|e| e[k]).unwrap_or(0)
    }

    fn sample(&self, idx: i32, j: usize) -> (i32, i32) {
        let w = self.tword(idx);
        let k = s8(w);
        let col = COLS + 200 * j;
        let kk = k.unsigned_abs() as usize;
        let lat2 = *self.g.get(col + kk).unwrap_or(&0) as i32;
        let mut a = *self.g.get(col + kk + 100).unwrap_or(&0) as i32;
        let off = lat2 * 4;
        let (mut rs, half, lon) = (self.tword_t(off), self.tword_t(off + 2), self.tword_t(off + 4));
        if k < 0 {
            a = half - a;
        }
        if w & 0x8000 != 0 {
            rs = -rs;
        }
        let l = 2 * half;
        let mut bp = lon - a;
        if bp < 0 {
            bp += l;
        }
        let left = rs + bp;
        let mut bp = lon + a - l;
        if bp < 0 {
            bp += l;
        }
        (left, rs + bp)
    }

    fn pix(&self, map: &[u8], i: i32) -> u8 {
        self.colour(*map.get((MAP_BASE + i).max(0) as usize).unwrap_or(&0))
    }

    /// Draws one globe line into a 320x200 buffer (DNVGA fn 0x148). Returns true when the
    /// frame is complete (the next call starts a new frame).
    pub fn render_line(&mut self, fb: &mut [u8], map: &[u8]) -> bool {
        let mut c = self.line.unwrap_or(LineCursor { half: 0, di: 1, row: 0x6360, step: -320 });
        let g = &self.g;
        let v0 = *g.get(c.di).unwrap_or(&0xFF);
        if v0 >= 0x80 {
            // End of a hemisphere.
            if c.half == 1 {
                self.line = None;
                return true;
            }
            c = LineCursor { half: 1, di: (1 - s8(g[0] as i32)) as usize, row: 0x64A0, step: 320 };
            let v = *g.get(c.di).unwrap_or(&0xFF);
            if v >= 0x80 {
                self.line = None;
                return true;
            }
        }
        let mut di = c.di;
        let mut v = g[di] as i32;
        di += 1;
        let (mut l, mut r) = (c.row - 1, c.row);
        let mut j = 0usize;
        loop {
            let idx = if c.half == 1 { -v } else { v };
            let (li, ri) = self.sample(idx, j);
            if (0..64000).contains(&l) {
                fb[l as usize] = self.pix(map, li);
            }
            if (0..64000).contains(&r) {
                fb[r as usize] = self.pix(map, ri);
            }
            l -= 1;
            r += 1;
            j += 1;
            v = *self.g.get(di).unwrap_or(&0xFF) as i32;
            di += 1;
            if v >= 0x80 {
                break;
            }
        }
        c.di = di;
        c.row += c.step;
        self.line = Some(c);
        false
    }

    /// Restarts the progressive renderer at line 0 (after a full render).
    pub fn restart(&mut self) {
        self.line = None;
    }

    /// sub_1BAF2: screen position of a map position (lon16, lat) on the globe, as the
    /// point under Paul's marker (ICONES 0x36 is drawn at (x, y - height)).
    pub fn marker(&self, lon16: u16, lat: i16) -> Option<(i32, i32)> {
        let g = &self.g;
        let gb = |i: usize| *g.get(i).unwrap_or(&0) as i32;
        let mut b0 = 0;
        let mut b1 = 0;
        let mut bx = lat as i32 * 8;
        if bx < 0 {
            b1 = 0xFF;
            bx = -bx;
        }
        let e = (bx / 8).min(98) as usize;
        let half = self.t[e][1];
        let p = (2 * half as i64 * lon16 as i64) as u64;
        let mut dx = ((p >> 16) + ((p >> 15) & 1)) as i32;
        let mut ax = 0;
        dx -= self.t[e][2];
        if dx < 0 {
            dx = -dx;
            ax = 0xFFFF;
        }
        if dx >= half {
            dx = 2 * half - dx;
            ax ^= 0xFFFF;
        }
        let q = half >> 1;
        if dx >= q {
            b0 = 0x80;
            dx = 2 * q - dx;
        }
        let hs = ax & 0xFF;
        let lat2 = bx >> 2;
        let mut di = COLS;
        let mut cx = 100i32;
        let mut j = 0usize;
        let mut saved: Option<i32> = None;
        loop {
            let mut found = false;
            while cx > 0 {
                cx -= 1;
                let c = gb(di);
                di += 1;
                if c == lat2 {
                    found = true;
                    break;
                }
            }
            if !found {
                cx = saved?;
                break;
            }
            cx += 1;
            if dx <= gb(di + 0x63) {
                break;
            }
            saved = Some(cx);
            j += 1;
            di += 0xC7;
            if di >= COLS + 64 * 200 {
                return None;
            }
        }
        let mut k = 100 - cx;
        if b0 & 0x80 != 0 {
            k = -k;
        }
        let word = (b1 << 8) | (k & 0xFF);
        let m = (34..34 + 128).find(|&i| self.tt.get(i).copied() == Some(word))?;
        let mut v = m as i32 - 98;
        let vs = v < 0;
        v = v.abs();
        let (mut si, mut bxl, mut ch, mut best) = (0usize, 54i32, 0xFFi32, 0i32);
        loop {
            let al = (gb(si) + 1) & 0xFF;
            si += 1;
            if al == 0 {
                break;
            }
            let ln = ((-al) & 0xFF) as usize;
            if ln <= j {
                break;
            }
            let line = si;
            si += ln;
            let d = (gb(line + j) - v) & 0xFF;
            if d < ch {
                ch = d;
                best = bxl;
                if d == 0 {
                    break;
                }
            }
            bxl -= 1;
            if bxl == 0 {
                break;
            }
        }
        if ch > 2 {
            return None;
        }
        let mut y = best - 54;
        if vs {
            y = -y;
        }
        let x = if hs != 0 { -(j as i32) } else { j as i32 };
        Some((x + 0xA0, y + 0x4F))
    }

    /// sub_1BC64 hit test + sub_1BD25: screen point -> (lon16, lat), None outside the disc.
    pub fn pick(&self, x: i32, y: i32) -> Option<(u16, i16)> {
        let (dx0, by) = (x - 0x60, y - 0x19);
        if !((0..0x80).contains(&dx0) && (0..0x6D).contains(&by)) {
            return None;
        }
        let g = &self.g;
        let gb = |i: usize| *g.get(i).unwrap_or(&0xFF) as i32;
        let bx = by - 0x36;
        let mut cx = bx.abs() + 1;
        let (mut si, mut ax) = (0usize, 0usize);
        loop {
            si += ax;
            let al = gb(si);
            si += 1;
            ax = ((-(al + 1)) & 0xFF) as usize;
            cx -= 1;
            if cx == 0 || ax == 0 {
                break;
            }
        }
        let di = si + ax;
        let sgn = dx0 - 0x40;
        let dx = sgn.unsigned_abs() as usize;
        si += dx;
        if si >= di {
            return None;
        }
        let mut v = gb(si);
        if bx >= 0 {
            v = -v;
        }
        let w = self.tword(v);
        let k = s8(w);
        let kk = k.unsigned_abs() as usize;
        let col = COLS + 200 * dx;
        let lat2 = gb(col + kk);
        let mut a = gb(col + kk + 100);
        let e = ((lat2 / 2) as usize).min(98);
        let half = self.t[e][1];
        if k < 0 {
            a = half - a;
        }
        let l = 2 * half;
        let mut lon = self.t[e][2];
        if sgn < 0 {
            a = -a;
        }
        lon += a;
        if lon < 0 {
            lon += l;
        }
        if lon >= l {
            lon -= l;
        }
        let lat = if w & 0x8000 != 0 { -(lat2 / 2) } else { lat2 / 2 };
        if l == 0 {
            return None;
        }
        Some((((lon << 16) / l) as u16, lat as i16))
    }

    /// Full frame (DNVGA fn 0x145).
    pub fn render(&mut self, fb: &mut [u8], map: &[u8]) {
        self.line = None;
        while !self.render_line(fb, map) {}
    }
}
