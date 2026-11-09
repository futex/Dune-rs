//! The planet grid (TABLAT.BIN + MAP.HSQ), the location table and the 2-D map renderers.
//! See docs/re/travel.md §0-§2 and §4.3.
//!
//! Everything works on a mirror of the original data segment (`mem`, ds:0000..FFFF), so the
//! original addresses apply: ds:4948 TABLAT (byte-swapped, +4/+6 scratch centre columns),
//! ds:4880 longitude units per cell, ds:4C60 renderer scratch, ds:197C/197E map centre,
//! ds:DCF2..DCF8 view geometry, ds:46E3 view rect, ds:46EB map mode, ds:0100 locations.
//! The map buffer is MAP.HSQ (row 0 at offset 0x62FC).
//!
//! The terrain renderers are literal ports of the DNVGA driver entries k=29 (drv:1F4C, the
//! SEE DUNE MAP view, 4 pixels per cell with interpolation) and k=22 (drv:23EB, the flight
//! map, 1 pixel per cell), including their use of the ds:4C60 scratch area.

pub const TABLAT: usize = 0x4948;
pub const LON_UNITS: usize = 0x4880;
pub const SCRATCH: usize = 0x4C60;
pub const MAP_BASE: usize = 0x62FC;
pub const LOCATIONS: usize = 0x0100;
pub const LOC_SIZE: usize = 0x1C;

fn g16(m: &[u8], a: usize) -> u16 {
    u16::from_le_bytes([m[a & 0xFFFF], m[(a + 1) & 0xFFFF]])
}
fn s16(m: &mut [u8], a: usize, v: u16) {
    m[a & 0xFFFF] = v as u8;
    m[(a + 1) & 0xFFFF] = (v >> 8) as u8;
}

/// sub_100D1: TABLAT.BIN (big-endian words) -> ds:4948, and the ds:4880 table.
pub fn init_tables(mem: &mut [u8], tablat: &[u8]) {
    for i in 0..0x18C.min(tablat.len() / 2) {
        let v = u16::from_be_bytes([tablat[2 * i], tablat[2 * i + 1]]);
        s16(mem, TABLAT + 2 * i, v);
    }
    for r in 0..0x63 {
        let w = g16(mem, TABLAT + 2 + 8 * r) as u32;
        let bx = 2 * w;
        if bx == 0 {
            continue;
        }
        let (q, rem) = (0x10000 / bx, 0x10000 % bx);
        let v = q + (w < rem) as u32;
        s16(mem, LON_UNITS + 2 * r, v as u16);
    }
}

/// Half width of row |r| (TABLAT +2).
pub fn half(mem: &[u8], r: i32) -> u16 {
    g16(mem, TABLAT + 2 + 8 * (r.unsigned_abs() as usize).min(98))
}

/// sub_1B58B: map buffer index of the cell at (lon, row) and the row length 2w.
pub fn cell_ptr(mem: &[u8], lon: u16, row: i16) -> (usize, u16) {
    let r = (row as i32).unsigned_abs() as usize;
    let r = r.min(98);
    let mut off = g16(mem, TABLAT + 8 * r);
    if row < 0 {
        off = off.wrapping_neg();
    }
    let bp = g16(mem, TABLAT + 2 + 8 * r).wrapping_mul(2);
    let p = bp as u32 * lon as u32;
    let col = (p >> 16) as u16 + ((p >> 15) & 1) as u16;
    let di = (MAP_BASE as u16).wrapping_add(off).wrapping_add(col);
    (di as usize, bp)
}

/// sub_1B532: the cell byte.
pub fn cell(mem: &[u8], map: &[u8], lon: u16, row: i16) -> u8 {
    map.get(cell_ptr(mem, lon, row).0).copied().unwrap_or(0)
}

/// sub_1B5C5: snapped longitude (start of the cell column).
pub fn snap_lon(mem: &[u8], lon: u16, row: i16) -> u16 {
    let r = (row as i32).unsigned_abs() as usize;
    let bp = g16(mem, TABLAT + 2 + 8 * r.min(98)).wrapping_mul(2) as u32;
    let p = bp * lon as u32;
    let col = (p >> 16) + ((p >> 15) & 1);
    if bp == 0 {
        return lon;
    }
    ((col << 16) / bp).min(0xFFFF) as u16
}

/// Location record i (1-based) address.
pub fn loc_addr(i: usize) -> usize {
    0xE4 + LOC_SIZE * i
}

/// Number of location records (terminator 0xFF).
pub fn loc_count(mem: &[u8]) -> usize {
    let mut n = 0;
    while mem[LOCATIONS + LOC_SIZE * n] != 0xFF && n < 100 {
        n += 1;
    }
    n
}

/// Place kind (sub_15E4F): 0 sietch, 1 palace, 2 village, 3 fort, 4 Harkonnen palace.
pub fn kind(app: u8) -> usize {
    match app {
        0..=0x1F => 0,
        0x20 => 1,
        0x21..=0x27 => 2,
        0x28..=0x2F => 3,
        _ => 4,
    }
}

/// sub_10169: snap the place longitudes, store their cell pointers, mark the map cells
/// (bit 6) and the MAP2 region statistics (+10, +11).
pub fn init_locations(mem: &mut [u8], map: &mut [u8], map2: &[u8], snap: bool) {
    let mut hist = [7u16; 256];
    for &b in map2.iter().take(0xC5F9) {
        hist[b as usize] = hist[b as usize].wrapping_add(1);
    }
    for i in 0..loc_count(mem) {
        let a = LOCATIONS + LOC_SIZE * i;
        let (lon, row) = (g16(mem, a + 2), g16(mem, a + 4) as i16);
        let lon = if snap { snap_lon(mem, lon, row) } else { lon };
        let (di, _) = cell_ptr(mem, lon, row);
        s16(mem, a + 2, lon);
        s16(mem, a + 6, di as u16);
        if let Some(c) = map.get_mut(di) {
            *c |= 0x40;
        }
        if snap {
            let v = map2.get(di).copied().unwrap_or(0);
            mem[a + 0x10] = v;
            mem[a + 0x11] = (hist[v as usize] >> 4) as u8;
        }
    }
}

/// sub_1409A: place whose cell pointer is `di`, as a 1-based index.
pub fn place_at_cell(mem: &[u8], di: usize) -> Option<usize> {
    (0..loc_count(mem)).find(|&i| g16(mem, LOCATIONS + LOC_SIZE * i + 6) as usize == di).map(|i| i + 1)
}

/// sub_1B647: map position -> screen point. `map_mode` = ds:46EB bit 7 (x4 scale).
pub fn map_to_screen(mem: &[u8], lon: u16, row: i16) -> (i32, i32) {
    let cl = if mem[0x46EB] & 0x80 != 0 { 2 } else { 0 };
    let row_c = g16(mem, 0x197E) as i16;
    let y = ((row.wrapping_sub(row_c) as i32) << cl) + g16(mem, 0xDCF8) as i16 as i32;
    let r = (row as i32).unsigned_abs() as usize;
    let l = g16(mem, TABLAT + 2 + 8 * r.min(98)).wrapping_mul(2) as i16 as i32;
    let d = lon.wrapping_sub(g16(mem, 0x197C)) as i16 as i32;
    let p = (d * l) << cl;
    let x = ((p >> 16) as i16).wrapping_add(g16(mem, 0xDCF6) as i16) as i32;
    (x, y as i16 as i32)
}

/// sub_1B5F9 in flight-map mode (cl = 0): screen point -> (lon, row), using the row centre
/// columns stored by the last flight-map render (TABLAT +6).
pub fn screen_to_map(mem: &[u8], x: i32, y: i32) -> (u16, i16) {
    let dx = (x as i16).wrapping_sub(g16(mem, 0xDCF6) as i16);
    let row = (y as i16).wrapping_sub(g16(mem, 0xDCF8) as i16).wrapping_add(g16(mem, 0x197E) as i16);
    let r = (row as i32).unsigned_abs() as usize;
    let si = TABLAT + 8 * r.min(98);
    let mut dx = (dx as u16).wrapping_add(g16(mem, si + 6));
    let cx = g16(mem, si + 2).wrapping_mul(2);
    if dx >= cx {
        if (dx as i16) < 0 {
            dx = dx.wrapping_add(cx);
        } else {
            dx = dx.wrapping_sub(cx);
        }
    }
    let lon = if cx == 0 { 0 } else { (((dx as u32) << 16) / cx as u32).min(0xFFFF) as u16 };
    (lon, row)
}

fn rd(map: &[u8], i: u16) -> u8 {
    map.get(i as usize).copied().unwrap_or(0)
}

/// Pole-mask tables of DNVGA: ramp cs:08EF (8 bytes), masks cs:08F7 / cs:092F, flight
/// margins cs:2384. Taken from the driver image (DNVGA.HSQ, org 0x100).
pub struct DriverTables {
    pub drv: Vec<u8>,
}

impl DriverTables {
    pub fn new(dnvga: Vec<u8>) -> DriverTables {
        DriverTables { drv: dnvga }
    }
    fn b(&self, addr: usize) -> u8 {
        self.drv.get(addr.wrapping_sub(0x100)).copied().unwrap_or(0)
    }
}

/// sub_1B6C3 with ds:46EB bit 7 set: the SEE DUNE MAP terrain (driver k=29), into the
/// 320x200 buffer `fb` at (4,4)-(316,148). Clamps |ds:197E| <= 75.
pub fn render_map(mem: &mut [u8], map: &[u8], fb: &mut [u8], drv: &DriverTables) {
    s16(mem, 0xDCF6, 0xA0);
    s16(mem, 0xDCF8, 0x4C);
    let r = g16(mem, 0x197E) as i16;
    if (r as i32).unsigned_abs() >= 0x4B {
        s16(mem, 0x197E, if r < 0 { (-0x4Bi16) as u16 } else { 0x4B });
    }
    let lon = g16(mem, 0x197C);
    let row0 = (g16(mem, 0x197E) as i16).wrapping_sub(0x12);
    K29 { mem, map, fb, drv, lon, mask_tab: 0, lim: 0, out: 0x504 }.run(row0);
}

struct K29<'a> {
    mem: &'a mut [u8],
    map: &'a [u8],
    fb: &'a mut [u8],
    drv: &'a DriverTables,
    lon: u16,
    mask_tab: usize,
    lim: usize,
    out: usize,
}

impl K29<'_> {
    fn off(&self, bp: usize) -> u16 {
        g16(self.mem, bp)
    }
    fn w(&self, bp: usize) -> u16 {
        g16(self.mem, bp + 2)
    }

    fn run(&mut self, row0: i16) {
        let s = SCRATCH;
        let mut cnt = 0x24;
        let ax = (row0 as u16).wrapping_shl(3);
        let mut bp;
        if (ax as i16) < 0 {
            bp = TABLAT + ax.wrapping_neg() as usize;
            self.mask_tab = 0x8F7;
            self.lim = TABLAT + 0x2B0;
            let (cx, bx) = (self.off(bp).wrapping_neg(), self.w(bp));
            self.fill(s + 0x8C, cx, bx, bp);
            loop {
                bp -= 8;
                let (cx, bx) = (self.off(bp).wrapping_neg(), self.w(bp));
                if cx == 0 {
                    self.mask_tab = 0x92F;
                    self.lim += 8;
                    self.fill(s + 0x6CC, cx, bx, bp);
                    self.interp(bp, true);
                    cnt -= 1;
                    if cnt == 0 {
                        return;
                    }
                    break;
                }
                self.fill(s + 0x6CC, cx, bx, bp);
                self.interp(bp, true);
                cnt -= 1;
                if cnt == 0 {
                    return;
                }
            }
        } else {
            bp = TABLAT + ax as usize;
            self.mask_tab = 0x92F;
            self.lim = TABLAT + 0x2B8;
            let (cx, bx) = (self.off(bp), self.w(bp));
            self.fill(s + 0x8C, cx, bx, bp);
        }
        loop {
            bp += 8;
            let (cx, bx) = (self.off(bp), self.w(bp));
            self.fill(s + 0x6CC, cx, bx, bp);
            self.interp(bp, false);
            cnt -= 1;
            if cnt == 0 {
                return;
            }
        }
    }

    /// drv:2025: one map row -> one scratch line, 4 pixels per cell.
    fn fill(&mut self, di: usize, cx: u16, bx: u16, bp: usize) {
        let mut si = (MAP_BASE as u16).wrapping_add(cx);
        let l = bx.wrapping_add(bx);
        let p = self.lon as u32 * l as u32;
        let centre = (p >> 16) as u16;
        s16(self.mem, bp + 4, centre);
        let sub = ((p as u16) >> 14) & 3;
        let mut di = di - sub as usize;
        let t = |c: u8| (c << 4) | 8;
        let mut al;
        let (mut cx, rest): (u16, Option<u16>);
        if l < 88 {
            di += (88 - l as usize) * 2;
            let mut s = centre.wrapping_sub(l >> 1);
            if (s as i16) < 0 {
                s = s.wrapping_add(l);
            }
            let bx2 = l.wrapping_sub(s);
            let c = l.wrapping_add(1).wrapping_sub(bx2);
            // jns 209E (always taken here)
            if bx2 == 0 {
                return;
            }
            cx = bx2;
            rest = Some(c);
            let s0 = si;
            si = si.wrapping_add(s);
            al = t(rd(self.map, si.wrapping_sub(1)));
            self.cells(&mut di, &mut si, &mut al, cx);
            si = s0;
            cx = rest.unwrap();
        } else {
            let mut s = centre.wrapping_sub(44);
            if (s as i16) < 0 {
                s = s.wrapping_add(l);
            }
            let bx2 = l.wrapping_sub(s);
            let c = 89u16.wrapping_sub(bx2);
            if (c as i16) >= 0 {
                if bx2 == 0 {
                    return;
                }
                let s0 = si;
                si = si.wrapping_add(s);
                al = t(rd(self.map, si.wrapping_sub(1)));
                self.cells(&mut di, &mut si, &mut al, bx2);
                si = s0;
                cx = c;
            } else {
                cx = 89;
                si = si.wrapping_add(s);
                al = t(rd(self.map, si.wrapping_sub(1)));
            }
        }
        if cx != 0 {
            self.cells(&mut di, &mut si, &mut al, cx);
        }
    }

    fn cells(&mut self, di: &mut usize, si: &mut u16, al: &mut u8, n: u16) {
        for _ in 0..n {
            let dh = (rd(self.map, *si) << 4) | 8;
            *si = si.wrapping_add(1);
            let d = ((dh.wrapping_sub(*al) as i8) >> 2) as u8;
            let a = *al;
            let px = [a, a.wrapping_add(d), a.wrapping_add(d.wrapping_mul(2)), a.wrapping_add(d.wrapping_mul(3))];
            for (k, v) in px.into_iter().enumerate() {
                self.mem[(*di + k) & 0xFFFF] = v;
            }
            *di += 4;
            *al = dh;
        }
    }

    /// drv:2123 (up = southern rows toward the equator) / drv:2153, then the pole mask,
    /// the output of 4 lines and the copy of the new row into line 0 (drv:22A0..2342).
    fn interp(&mut self, bp: usize, up: bool) {
        let s = SCRATCH;
        let (nar, o1, o2, o3, wid, wn, ww) = if up {
            (s + 0x13C, s + 0x2CC, s + 0x45C, s + 0x5EC, s + 0x77C, self.w(bp + 8), self.w(bp))
        } else {
            (s + 0x77C, s + 0x5EC, s + 0x45C, s + 0x2CC, s + 0x13C, self.w(bp), self.w(bp - 8))
        };
        let m = &mut *self.mem;
        let frac: u16 = if wn == 0 {
            0
        } else {
            ((((ww.wrapping_sub(wn)) as u32) << 16) / wn as u32) as u16
        };
        let t = frac >> 1;
        let u = t >> 1;
        let c3 = t.wrapping_add(u).wrapping_add(0x80);
        let step1 = (u.wrapping_add(0x80) >> 8) as u8;
        let step3 = (c3 >> 8) as u8;
        let step2 = (t.wrapping_add(0x80) >> 8) as u8;
        // Forward pass.
        let (mut si, mut di, mut p1, mut p2, mut p3) = (nar, wid, o1, o2, o3);
        let (mut a1, mut a2, mut a3, mut acc) = (step1, step2, step3, frac);
        for _ in 0..0xB0 {
            let mut al = m[si & 0xFFFF];
            si += 1;
            let ah = (((m[di & 0xFFFF].wrapping_sub(al)) as i8) >> 2) as u8;
            di += 1;
            al = al.wrapping_add(ah);
            m[p1 & 0xFFFF] = al;
            p1 += 1;
            let (v, c) = a1.overflowing_add(step1);
            a1 = v;
            if c {
                m[p1 & 0xFFFF] = al;
                p1 += 1;
            }
            al = al.wrapping_add(ah);
            m[p2 & 0xFFFF] = al;
            p2 += 1;
            let (v, c) = a2.overflowing_add(step2);
            a2 = v;
            if c {
                m[p2 & 0xFFFF] = al;
                p2 += 1;
            }
            al = al.wrapping_add(ah);
            m[p3 & 0xFFFF] = al;
            p3 += 1;
            let (v, c) = a3.overflowing_add(step3);
            a3 = v;
            if c {
                m[p3 & 0xFFFF] = al;
                p3 += 1;
            }
            let (v, c) = acc.overflowing_add(frac);
            acc = v;
            if c {
                di += 1;
            }
        }
        // Backward pass.
        let (mut si, mut di, mut p1, mut p2, mut p3) = (nar - 1, wid - 1, o1, o2, o3 - 1);
        let (mut a1, mut a2, mut a3, mut acc) = (step1, step2, step3, frac);
        for _ in 0..0xB0 {
            let mut al = m[si & 0xFFFF];
            si -= 1;
            let ah = (((m[di & 0xFFFF].wrapping_sub(al)) as i8) >> 2) as u8;
            di -= 1;
            al = al.wrapping_add(ah);
            p1 -= 1;
            m[p1 & 0xFFFF] = al;
            let (v, c) = a1.overflowing_add(step1);
            a1 = v;
            if c {
                p1 -= 1;
                m[p1 & 0xFFFF] = al;
            }
            al = al.wrapping_add(ah);
            p2 -= 1;
            m[p2 & 0xFFFF] = al;
            let (v, c) = a2.overflowing_add(step2);
            a2 = v;
            if c {
                p2 -= 1;
                m[p2 & 0xFFFF] = al;
            }
            al = al.wrapping_add(ah);
            m[p3 & 0xFFFF] = al;
            p3 -= 1;
            let (v, c) = a3.overflowing_add(step3);
            a3 = v;
            if c {
                m[p3 & 0xFFFF] = al;
                p3 -= 1;
            }
            let (v, c) = acc.overflowing_add(frac);
            acc = v;
            if c {
                di -= 1;
            }
        }
        // Pole mask.
        if bp >= self.lim {
            let mut idx = self.mask_tab + (bp - self.lim);
            let mut di = s + 0xA0;
            for _ in 0..4 {
                let lc = self.drv.b(idx) as usize;
                let mid = self.drv.b(idx + 1) as usize;
                if lc > 4 {
                    for _ in 0..lc - 4 {
                        self.mem[di & 0xFFFF] = 0;
                        di += 1;
                    }
                    for k in 0..4 {
                        self.mem[di & 0xFFFF] = self.drv.b(0x8EF + k);
                        di += 1;
                    }
                } else {
                    for k in 4 - lc..4 {
                        self.mem[di & 0xFFFF] = self.drv.b(0x8EF + k);
                        di += 1;
                    }
                }
                di += 2 * mid;
                if lc > 4 {
                    for k in 0..4 {
                        self.mem[di & 0xFFFF] = self.drv.b(0x8F3 + k);
                        di += 1;
                    }
                    for _ in 0..lc - 4 {
                        self.mem[di & 0xFFFF] = 0;
                        di += 1;
                    }
                } else {
                    for k in 0..lc {
                        self.mem[di & 0xFFFF] = self.drv.b(0x8F3 + k);
                        di += 1;
                    }
                }
                di += 0x58;
                idx += 2;
            }
        }
        // Output 4 lines of 312 pixels (drv:2343).
        for k in 0..4 {
            let src = s + 0xA0 + k * 0x190;
            let dst = self.out + k * 320;
            for x in 0..0x138 {
                if let Some(p) = self.fb.get_mut(dst + x) {
                    *p = ((self.mem[(src + x) & 0xFFFF] >> 4) & 0x0F) + 0x10;
                }
            }
        }
        self.out += 4 * 320;
        // Line 4 -> line 0 (700 bytes).
        self.mem.copy_within(s + 0x6CC..s + 0x6CC + 700, s + 0x8C);
    }
}

/// sub_1B6C3 with ds:46EB bit 7 clear: the flight destination map (sub_1B7D2 + driver
/// k=22) in the rect ds:46E3, into `fb`. Clamps |ds:197E| <= 0x56 - h/2.
pub fn render_flight(mem: &mut [u8], map: &[u8], fb: &mut [u8], drv: &DriverTables) {
    let (x0, y0, x1, y1) = (g16(mem, 0x46E3), g16(mem, 0x46E5), g16(mem, 0x46E7), g16(mem, 0x46E9));
    let wdt = x1.wrapping_sub(x0);
    s16(mem, 0xDCF6, (wdt >> 1).wrapping_add(x0));
    s16(mem, 0xDCF2, wdt);
    let a = y1.wrapping_sub(y0).wrapping_sub(1);
    s16(mem, 0xDCF8, (a >> 1).wrapping_add(y0));
    let h = a.wrapping_add(1);
    s16(mem, 0xDCF4, h);
    let half_h = (h.wrapping_sub(1)) >> 1;
    let lim = 0x56u16.wrapping_sub(half_h);
    let r = g16(mem, 0x197E) as i16;
    if (r as i32).unsigned_abs() as u16 >= lim {
        s16(mem, 0x197E, if r < 0 { lim.wrapping_neg() } else { lim });
    }
    let lon = g16(mem, 0x197C);
    let top = (g16(mem, 0x197E) as i16).wrapping_sub(half_h as i16);
    let mut di = SCRATCH;
    let ax = (top as u16).wrapping_shl(3);
    let mut n = h as usize;
    let mut bp;
    let mut neg = (ax as i16) < 0;
    bp = if neg { TABLAT + ax.wrapping_neg() as usize } else { TABLAT + ax as usize };
    while n > 0 {
        let mut cx = g16(mem, bp);
        let bx = g16(mem, bp + 2);
        if neg {
            cx = cx.wrapping_neg();
            if cx == 0 {
                neg = false;
            }
        }
        copy_row(mem, map, di, cx, bx, bp, lon);
        di += 200;
        if neg {
            bp -= 8;
        } else {
            bp += 8;
        }
        n -= 1;
    }
    if mem[0x46EB] & 0x40 != 0 {
        return;
    }
    // Driver k=22: bottom-up, 1 pixel per cell.
    let w = wdt as usize;
    let hh = h as usize;
    let mut row = top.wrapping_add(h as i16 - 1);
    let mut si = SCRATCH + 200 * (hh - 1);
    let mut out = (y0 as usize + hh - 1) * 320 + x0 as usize;
    for _ in 0..hh {
        flight_line(mem, fb, drv, row, w, si, out);
        si -= 200;
        out = out.wrapping_sub(320);
        row = row.wrapping_sub(1);
    }
}

/// sub_1B7D2: copy min(2w, width) cells centred on lon into the scratch line `di`.
fn copy_row(mem: &mut [u8], map: &[u8], di: usize, cx: u16, bx: u16, bp: usize, lon: u16) {
    let mut si = (MAP_BASE as u16).wrapping_add(cx);
    let l = bx.wrapping_add(bx);
    let centre = ((lon as u32 * l as u32) >> 16) as u16;
    s16(mem, bp + 6, centre);
    let wdt = g16(mem, 0xDCF2);
    let mut di = di;
    let mut n = wdt;
    if l < wdt {
        di += ((wdt - l) >> 1) as usize;
        n = l;
    }
    let mut s = centre.wrapping_sub(n >> 1);
    if (s as i16) < 0 {
        s = s.wrapping_add(l);
    }
    let bx2 = l.wrapping_sub(s);
    let c = n.wrapping_sub(bx2);
    let mut put = |di: &mut usize, si: u16, k: u16| {
        for i in 0..k {
            mem[*di & 0xFFFF] = rd(map, si.wrapping_add(i));
            *di += 1;
        }
    };
    if (c as i16) >= 0 {
        put(&mut di, si.wrapping_add(s), bx2);
        put(&mut di, si, c);
    } else {
        si = si.wrapping_add(s);
        put(&mut di, si, n);
    }
}

/// drv:2413: one flight-map line with the pole margins (drv:2396 / drv:23D7).
fn flight_line(mem: &[u8], fb: &mut [u8], drv: &DriverTables, row: i16, w: usize, si: usize, out: usize) {
    let mut o = out;
    let mut put = |o: &mut usize, v: u8| {
        if let Some(p) = fb.get_mut(*o) {
            *p = v;
        }
        *o += 1;
    };
    let a = (row as i32).unsigned_abs() as i32 - 0x46;
    let (mut si, mut cx, mut m) = (si, w, 0usize);
    if a >= 0 {
        m = drv.b(0x2384 + a as usize) as usize;
        si += m;
        if cx < 2 * m {
            for _ in 0..w {
                put(&mut o, 0);
            }
            return;
        }
        cx -= 2 * m;
        if m >= 4 {
            for _ in 0..m - 4 {
                put(&mut o, 0);
            }
            put(&mut o, 0x1C);
            put(&mut o, 0x19);
        }
        put(&mut o, 0x18);
        put(&mut o, 0x17);
    }
    for i in 0..cx {
        put(&mut o, (mem[(si + i) & 0xFFFF] & 0x0F) + 0x10);
    }
    if m != 0 {
        put(&mut o, 0x17);
        put(&mut o, 0x18);
        if m >= 4 {
            put(&mut o, 0x19);
            put(&mut o, 0x1C);
            for _ in 0..m - 4 {
                put(&mut o, 0);
            }
        }
    }
}
