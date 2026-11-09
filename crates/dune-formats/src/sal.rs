//! `.SAL` room descriptions (PALACE / SIET / VILG / HARK) and their renderer.
//!
//! Port of the game's `draw_room` (seg000:3B59) and its polygon/line routines;
//! see `docs/re/sal.md` for the byte-level format.

use crate::sprite::{DrawOpts, SpriteSheet};

pub const VIEW_W: usize = 320;
pub const VIEW_H: usize = 152;
const SCALE: [u16; 8] = [256, 288, 320, 352, 384, 448, 512, 640];

#[derive(Debug, Clone)]
pub enum Record {
    /// `id` is 1-based in the current bank; id 1 is a character slot marker.
    Sprite { id: u16, x: u16, y: u8, scale: u8, hflip: bool, vflip: bool, pal: u8 },
    Line { color: u8, x0: i16, y0: i16, x1: i16, y1: i16 },
    Polygon { color: u8, flags: u8, gx: i8, gy: i8, start: (i16, i16), chain_a: Vec<(i16, i16)>, chain_b: Vec<(i16, i16)> },
}

#[derive(Debug, Clone, Default)]
pub struct Room {
    pub nslots: u8,
    pub records: Vec<Record>,
}

/// A character standing on a slot marker.
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    pub x: u16,
    pub y: u8,
    pub scale: u8,
    pub hflip: bool,
    pub pal: u8,
}

impl Room {
    pub fn markers(&self) -> Vec<Marker> {
        self.records
            .iter()
            .filter_map(|r| match *r {
                Record::Sprite { id: 1, x, y, scale, hflip, pal, .. } => Some(Marker { x, y, scale, hflip, pal }),
                _ => None,
            })
            .collect()
    }
}

pub struct Sal {
    pub rooms: Vec<Room>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

impl Sal {
    pub fn parse(d: &[u8]) -> Option<Sal> {
        let n = u16_at(d, 0)? as usize / 2;
        let mut rooms = Vec::with_capacity(n);
        for i in 0..n {
            let mut p = u16_at(d, 2 * i)? as usize;
            let nslots = *d.get(p)?;
            p += 1;
            let mut records = Vec::new();
            loop {
                let w = u16_at(d, p)?;
                p += 2;
                if w == 0xFFFF {
                    break;
                }
                if w & 0x8000 == 0 {
                    let x = *d.get(p)? as u16 | ((w >> 1) & 0x100);
                    records.push(Record::Sprite {
                        id: w & 0x1FF,
                        x,
                        y: *d.get(p + 1)?,
                        scale: ((w >> 10) & 7) as u8,
                        vflip: w & 0x2000 != 0,
                        hflip: w & 0x4000 != 0,
                        pal: *d.get(p + 2)?,
                    });
                    p += 3;
                } else if w >> 8 == 0xC0 {
                    let r = |k: usize| u16_at(d, p + 2 * k).map(|v| v as i16);
                    records.push(Record::Line { color: w as u8, x0: r(0)?, y0: r(1)?, x1: r(2)?, y1: r(3)? });
                    p += 8;
                } else {
                    let gx = *d.get(p)? as i8;
                    let gy = *d.get(p + 1)? as i8;
                    let start = (u16_at(d, p + 2)? as i16, u16_at(d, p + 4)? as i16);
                    p += 6;
                    let mut chain_a = Vec::new();
                    let mut chain_b = Vec::new();
                    let last = loop {
                        let v = u16_at(d, p)?;
                        let vy = u16_at(d, p + 2)? as i16;
                        p += 4;
                        chain_a.push(((v & 0x3FFF) as i16, vy));
                        if v & 0x4000 != 0 {
                            break v;
                        }
                    };
                    if last & 0x8000 == 0 {
                        loop {
                            let v = u16_at(d, p)?;
                            let vy = u16_at(d, p + 2)? as i16;
                            p += 4;
                            chain_b.push(((v & 0x3FFF) as i16, vy));
                            if v & 0x8000 != 0 {
                                break;
                            }
                        }
                    }
                    records.push(Record::Polygon { color: w as u8, flags: (w >> 8) as u8, gx, gy, start, chain_a, chain_b });
                }
            }
            rooms.push(Room { nslots, records });
        }
        Some(Sal { rooms })
    }
}

/// The game's lopsided Bresenham that fills one polygon side buffer.
fn edge(buf: &mut Vec<i16>, (x0, y0): (i16, i16), (x1, y1): (i16, i16)) {
    let dx = x1 as i32 - x0 as i32;
    let dy = y1 as i32 - y0 as i32;
    if dy == 0 {
        buf.push(x0.min(x1));
        return;
    }
    if dx == 0 {
        for _ in 0..=dy.abs() {
            buf.push(x0);
        }
        return;
    }
    let (sx, sy) = (dx.signum(), dy.signum());
    let (major, minor, axial) = if dx.abs() > dy.abs() { (dx.abs(), dy.abs(), (sx, 0)) } else { (dy.abs(), dx.abs(), (0, sy)) };
    let mut err = major >> 1;
    let mut x = x0 as i32;
    for _ in 0..major {
        err += minor;
        let step = if err >= major {
            err -= major;
            (sx, sy)
        } else {
            axial
        };
        if step.1 == 1 {
            buf.push(x as i16);
        }
        x += step.0;
    }
}

fn put(fb: &mut [u8], x: i32, y: i32, c: u8) {
    if (0..VIEW_W as i32).contains(&x) && y >= 0 && (y as usize) < fb.len() / VIEW_W {
        fb[y as usize * VIEW_W + x as usize] = c;
    }
}

fn polygon(fb: &mut [u8], color: u8, flags: u8, gx: i8, gy: i8, start: (i16, i16), a: &[(i16, i16)], b: &[(i16, i16)]) {
    let mut ba = Vec::new();
    let mut bb = Vec::new();
    let mut cur = start;
    for &v in a {
        edge(&mut ba, cur, v);
        cur = v;
    }
    let end = cur;
    cur = start;
    for &v in b {
        edge(&mut bb, cur, v);
        cur = v;
    }
    edge(&mut bb, cur, end);
    let mask: u16 = ((flags as u16 & 0x3E) << 8) | 2;
    let mut lfsr: u16 = if flags & 0x3E != 0 { 1 } else { 0 };
    let mut acc: u16 = (color as u16) << 8;
    let (gx, gy) = ((gx as i16 as u16).wrapping_mul(16), (gy as i16 as u16).wrapping_mul(16));
    let lines = (end.1 - start.1).max(0) as usize;
    let mut y = start.1 as i32;
    for i in 0..lines {
        let (Some(&xa), Some(&xb)) = (ba.get(i), bb.get(i)) else { break };
        let (lo, hi) = (xa.min(xb) as i32, xa.max(xb) as i32);
        let mut c = acc;
        for k in 0..=(hi - lo) {
            lfsr = if lfsr & 1 != 0 { (lfsr >> 1) ^ mask } else { lfsr >> 1 };
            let pix = ((c >> 8) as u8).wrapping_add((lfsr & 3) as u8).wrapping_sub(1);
            let x = if flags & 1 != 0 { hi - k } else { lo + k };
            put(fb, x, y, pix);
            c = c.wrapping_add(gx);
        }
        acc = acc.wrapping_add(gy);
        y += 1;
    }
}

fn line(fb: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let (cx1, cy1) = (VIEW_W as i32, VIEW_H as i32);
    let ok = |x: i32, y: i32| (0..cx1).contains(&x) && (0..cy1).contains(&y);
    let (dx, dy) = (x1 - x0, y1 - y0);
    if dy == 0 {
        let s = x0.min(x1);
        for x in s..=s + dx.abs() {
            if ok(x, y0) {
                put(fb, x, y0, c);
            }
        }
        return;
    }
    if dx == 0 {
        let s = if dy > 0 { y0 } else { y0 - dy.abs() };
        for y in s..=s + dy.abs() {
            if ok(x0, y) {
                put(fb, x0, y, c);
            }
        }
        return;
    }
    let (sx, sy) = (dx.signum(), dy.signum());
    let (major, minor, axial) = if dx.abs() > dy.abs() { (dx.abs(), dy.abs(), (sx, 0)) } else { (dy.abs(), dx.abs(), (0, sy)) };
    let (mut err, mut x, mut y) = (major >> 1, x0, y0);
    for _ in 0..major {
        err += minor;
        let st = if err >= major {
            err -= major;
            (sx, sy)
        } else {
            axial
        };
        x += st.0;
        y += st.1;
        if ok(x, y) {
            put(fb, x, y, c);
        }
    }
}

/// Draws `room` into a 320-wide framebuffer (rows 0..152). Character markers are passed
/// to `character(marker_index, marker)` in file order, so the caller can draw people.
pub fn draw_room(fb: &mut [u8], room: &Room, bank: Option<&SpriteSheet>, mut character: impl FnMut(usize, &Marker, &mut [u8])) {
    let mut marker_idx = 0;
    for r in &room.records {
        match r {
            &Record::Sprite { id: 1, x, y, scale, hflip, pal, .. } => {
                character(marker_idx, &Marker { x, y, scale, hflip, pal }, fb);
                marker_idx += 1;
            }
            &Record::Sprite { id, x, y, scale, hflip, vflip, pal } => {
                let Some(s) = bank.and_then(|b| b.sprites.get(id as usize - 1)) else { continue };
                let opts = DrawOpts {
                    hflip,
                    vflip,
                    scale: if s.is_8bpp() { 256 } else { SCALE[scale as usize] },
                    pal_override: pal,
                    clip: Some((0, 0, VIEW_W as i32, VIEW_H as i32)),
                };
                s.draw(fb, VIEW_W, x as i32, y as i32, opts);
            }
            &Record::Line { color, x0, y0, x1, y1 } => line(fb, x0 as i32, y0 as i32, x1 as i32, y1 as i32, color),
            Record::Polygon { color, flags, gx, gy, start, chain_a, chain_b } => {
                polygon(fb, *color, *flags, *gx, *gy, *start, chain_a, chain_b)
            }
        }
    }
}

/// Sprite bank selected by a location "room byte" (`(v-1) >> 4`), `None` = keep the current one.
pub fn bank_for_room_byte(v: u8) -> Option<&'static str> {
    const BANKS: [&str; 16] = [
        "", "PROUGE.HSQ", "COMM.HSQ", "EQUI.HSQ", "BALCON.HSQ", "CORR.HSQ", "POR.HSQ", "SIET1.HSQ", "XPLAIN9.HSQ", "",
        "BUNK.HSQ", "FINAL.HSQ", "SERRE.HSQ", "BOTA.HSQ", "PALPLAN.HSQ", "SUN.HSQ",
    ];
    let b = BANKS[(v.wrapping_sub(1) >> 4) as usize];
    (!b.is_empty()).then_some(b)
}

/// SAL room index for a room byte.
pub fn sal_room(v: u8) -> usize {
    (v.wrapping_sub(1) & 0x0F) as usize
}

/// Palace room bytes for dl = 1..=12 (EXE table at ds:1225+, see docs/re/sal.md).
pub const PALACE_ROOM_BYTES: [u8; 12] = [0x4C, 0x3A, 0xCF, 0x62, 0x4B, 0x15, 0x5D, 0x26, 0x63, 0x61, 0x64, 0x5E];

/// Banks for every PALACE.SAL room (rooms 6/7/8 identified visually, not from the tables).
pub const PALACE_BANKS: [&str; 15] = [
    "POR.HSQ", "POR.HSQ", "POR.HSQ", "POR.HSQ", "PROUGE.HSQ", "COMM.HSQ", "COMM.HSQ", "COMM.HSQ", "EQUI.HSQ", "EQUI.HSQ",
    "BALCON.HSQ", "BALCON.HSQ", "CORR.HSQ", "CORR.HSQ", "SERRE.HSQ",
];
