//! Troop leaders in rooms, the map-screen troop layer and the orders
//! (docs/re/troops.md §1.6, §2, §4).
//!
//! Map objects follow the original object list ds:3CBE (`sub_16715`, `sub_1C5CF`, the
//! animation task 0x6B34 every 15 ticks, the C835 draw order), the contact window
//! (`sub_179EE`), the orders menus ds:210A..21A6 (`sub_1780A` / `sub_17847`), MOVE TROOP,
//! MODIFY EQUIPMENT and the troop card (`sub_178BC`).

use crate::{
    assets::Assets,
    audio::Audio,
    sim::{troop_addr, PROSPECTORS},
    talk::After,
    world::{fill, rect, Cmd, Line, Scr, World, VIEW},
};
use dune_formats::{planet, sprite::DrawOpts};

/// One entry of the object list ds:3CC0 (17 bytes in the original).
#[derive(Clone, Copy, Debug, Default)]
pub struct Obj {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub sprite: u8,
    /// Owner troop record.
    pub owner: usize,
    /// 1 animated, 2 one-shot, 0x40 overlay (selection), 0x80 removed.
    pub flags: u8,
    /// Current frame pointer (ds: address) and descriptor start.
    pub cur: usize,
    pub desc: usize,
}

/// Orders menus (ds:210A..21A6, 2122, 212E, 2136, 2012).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OrdMenu {
    /// 210A: the orders.
    Orders,
    /// 2122: out of range / captured.
    Far,
    /// 214A: moving troop.
    Moving,
    /// Occupation menus 215A / 216E / 2182 / 219A / 21A6.
    Occupation(u16),
    /// 212E: MOVE TROOP waiting for a click (prospectors: 2136).
    MoveWait { prospect: bool },
    /// 2012: equipment screen.
    Equipment,
}

#[derive(Default)]
pub struct TroopUi {
    pub objs: Vec<Obj>,
    /// ds:46F6 animation counter, tick accumulator of the 15-tick task.
    pub anim: u8,
    pub acc: u32,
    /// Orders menu stack (top last).
    pub menus: Vec<OrdMenu>,
    /// Contact window shown (the troop, ds:46EF).
    pub contact: Option<usize>,
    /// Contact face (FRM bank, frame 0) and its window rect.
    pub face: Option<Vec<u8>>,
    /// Troop card (right click), ds:46FA.
    pub card: Option<usize>,
    /// MOVE TROOP destinations picked for the prospectors (ds:4718).
    pub picks: Vec<usize>,
    /// Keyboard cycling of troop icons.
    pub kb: usize,
}

const NOWHERE_WHERE: u16 = 0x7F80;

impl World {
    // ------------------------------------------------------------------ rooms (§1.6)

    /// sub_13127: the Fremen troops standing in the current room become characters 0x0C
    /// (Harkonnen captive), 0x0E (an unrallied troop) and 0x0F (rallied leaders).
    pub(crate) fn place_troop_chars(&mut self) {
        self.mem[0x476B] = 0;
        self.mem[0x476A] = 0;
        for a in [0x10CA, 0x10BA, 0x10AA, 0x109A] {
            self.set16(a, NOWHERE_WHERE);
        }
        let bx = self.get16(6);
        if bx as u8 != 0x80 {
            return;
        }
        let di = self.get16(0x114E) as usize;
        if di < 0x100 {
            return;
        }
        let dx = self.get16(4);
        let dl = dx as u8;
        for si in self.sim().troop_ids(di) {
            let al = self.mem[si + 3];
            let mut ah = 2u8;
            if al & 0x20 != 0 {
                if self.mem[di + 8] >= 0x28 {
                    ah += 1;
                }
            } else {
                if self.mem[si + 0x10] & 0x80 != 0 {
                    continue;
                }
                if self.mem[0x2B] != 0 {
                    ah -= 1;
                }
            }
            if ah != dl {
                continue;
            }
            let (bp, dst) = if self.mem[si + 0x10] & 0x80 != 0 {
                let f = self.mem[si + 3] & 0x10;
                self.mem[0x1098 + 0x0F] = (self.mem[0x1098 + 0x0F] & 0xEF) | f;
                self.set16(0xEE, 0);
                self.mem[0xED] = if f != 0 { 0xFF } else { self.mem[si + 0x15] };
                (0x1098, 0x4768)
            } else if al >= 0x80 {
                (0x10B8, 0x4756)
            } else {
                self.mem[0x476A] &= 7;
                let k = self.mem[0x476A];
                self.mem[0x476A] += 1;
                if si == PROSPECTORS {
                    self.mem[0x476B] = k + 1;
                }
                (0x10C8, 0x4758 + 2 * k as usize)
            };
            self.set16(dst, si as u16);
            self.set16(bp, dx);
            self.set16(bp + 2, bx);
        }
        if self.mem[di + 8] == 0x21 {
            self.set16(0x10A8, dx);
            self.set16(0x10AA, bx);
            self.sim().smuggler_enter(di);
        }
        self.sim().analyse(di);
    }

    /// sub_13093: one menu line per present character (followers last), 0x88.. for the
    /// other leaders and 0x8F "Prospector Chief".
    pub(crate) fn char_lines(&mut self, present: &[usize]) -> Vec<Line> {
        let mut out = vec![];
        for pass in 0..2 {
            for &c in present {
                let follower = self.mem[0x0FD8 + 16 * c + 0x0F] & 0x40 != 0;
                if follower != (pass == 1) {
                    continue;
                }
                out.push(Line::new(0x78 + c as u16, Cmd::Character(c), true));
                if c == 0x0F {
                    let n = self.mem[0x476A] as usize;
                    let base = out.len() - 1;
                    for k in 1..n {
                        out.push(Line::new(0x87 + k as u16, Cmd::Leader(k), true));
                    }
                    out[base].cmd = Cmd::Leader(0);
                    if self.mem[0x2A] >= 5 && self.mem[0x476B] != 0 {
                        let i = base + self.mem[0x476B] as usize - 1;
                        if i < out.len() {
                            out[i] = Line::new(0x8F, Cmd::Leader(8), true);
                        }
                    }
                }
            }
        }
        out
    }

    /// sub_190BD: the talk menu's second line.
    pub(crate) fn talk_line2(&self, c: usize) -> Line {
        if c != 0x0C && self.get16(0x12) & 0x1000 != 0 && self.mem[0x10A7] & 0x10 == 0 {
            return Line::new(0x9C, Cmd::Overpower, true);
        }
        match c {
            0x0F => Line::new(0x93, Cmd::GiveOrdersRoom, true),
            0x0E => Line::new(if self.mem[0x0A] & 0x10 != 0 { 0x97 } else { 0x96 }, Cmd::WorkForMe, true),
            _ => {
                let f = self.mem[0x0FD8 + 16 * c + 0x0F];
                if f & 0x40 != 0 {
                    Line::new(0x92, Cmd::Stay(c), true)
                } else {
                    Line::new(0x91, Cmd::Come(c), f & 0x80 == 0)
                }
            }
        }
    }

    /// seg000:937E / 9373: a leader (or the unrallied troop) is selected for talking.
    pub(crate) fn select_leader(&mut self, k: usize) {
        let mut k = k;
        if k == 8 {
            k = (self.mem[0x476B] as usize).saturating_sub(1);
        }
        self.mem[0x476C] = k as u8;
        let si = self.get16(0x4758 + 2 * k) as usize;
        self.sim().contact_end_story(si);
        self.sim().troop_vars(si);
        self.talk_select(0x0F);
        self.talk_menu_for = Some(0x0F);
        self.build_menu();
    }
    pub(crate) fn select_fremen(&mut self) {
        let si = self.get16(0x4756) as usize;
        self.sim().troop_vars(si);
        self.talk_select(0x0E);
        self.talk_menu_for = Some(0x0E);
        self.build_menu();
    }

    /// 95C1: WORK FOR ME / RALLY ME.
    pub(crate) fn work_for_me(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let mut ah = 0u8;
        if self.get16(0xAC) >= 0x3E8 {
            let (al, c) = 0x64u8.overflowing_sub(self.mem[0x29]);
            if !c && (al >> 2) > self.mem[0x36] {
                ah = 2;
            }
        }
        self.mem[0x23] = ah;
        self.mem[0x47A5] = 0xFF;
        let c = self.get16(0x47C4) as usize;
        self.talk_sub(assets, audio, c, 5, After::Rally);
    }
    /// 95E2 tail for character 0x0E.
    pub(crate) fn rally_done(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.mem[0x1B] = self.mem[0x1B].wrapping_add(1);
        self.mem[0x23] = 0;
        if self.mem[0x47A5] != 0xFF {
            return;
        }
        let c = self.get16(0x47C4) as usize;
        if c != 0x0E {
            return;
        }
        let si = self.get16(0x4756) as usize;
        self.sim().rally(si);
        self.place_troop_chars();
        self.redraw_room(assets);
        if si == PROSPECTORS {
            self.mem[si + 0x15] = self.mem[si + 0x15].wrapping_add(0x18);
            self.mem[0x36] = self.mem[0x36].wrapping_add(0x18);
        }
        let k = (0..8).find(|&k| self.get16(0x4758 + 2 * k) as usize == si).unwrap_or(0);
        self.mem[0x476C] = k as u8;
        self.sim().troop_vars(si);
        self.talk_select(0x0F);
        self.talk_menu_for = Some(0x0F);
        self.build_menu();
        let _ = audio;
    }

    /// 5A03: GIVE ORDERS TO TROOP from the room.
    pub(crate) fn give_orders_room(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.mem[0x46F3] = 1;
        let k = self.mem[0x476C] as usize;
        let si = self.get16(0x4758 + 2 * k) as usize;
        self.mem[0x1954] = self.mem[si];
        self.talk_menu_for = None;
        self.mem[0xFB] = 0xFF;
        self.map_enter(assets, Some(audio), None);
        self.troop_select(assets, audio);
    }

    // ------------------------------------------------------------------ map objects (§4.3)

    /// sub_16770: animation descriptor of a troop (0 = not drawn).
    fn descriptor(&self, si: usize) -> usize {
        let m = &self.mem;
        let w = |a: usize| u16::from_le_bytes([m[a], m[a + 1]]) as usize;
        if m[si + 0x10] & 0x10 != 0 {
            return 0;
        }
        let job = (m[si + 3] & 0x0F) as usize;
        let eq = m[si + 0x19] & 0xC0;
        if m[si + 0x10] & 0x80 == 0 && m[si + 3] & 0x80 != 0 {
            let di = w(si + 4);
            return if m[di + 0x0A] & 0x10 != 0 { 0x181F } else { 0 };
        }
        if m[si + 3] & 0x40 != 0 {
            return self.walk_descriptor(si);
        }
        if m[si + 3] & 0x30 != 0 {
            if job == 0 && eq != 0 {
                return match eq {
                    0x80 => 0x1813,
                    0x40 => 0x1817,
                    _ => 0x181B,
                };
            }
            return w(0x16B6 + 2 * job);
        }
        if job == 0 && eq != 0 {
            return match eq {
                0x80 => 0x17BC,
                0x40 => 0x17C9,
                _ => 0x1806,
            };
        }
        let mut d = w(0x179C + 2 * job);
        let di = w(si + 4);
        if d == 0x1732 && m[di + 0x0A] & 2 != 0 {
            return 0x16AA;
        }
        if d == 0x1732 || d == 0x1774 {
            let v = (m[si + 2].wrapping_sub(1)) & 7;
            d += match v {
                0..=2 => 0,
                3 => 10,
                4 => 20,
                _ => 30,
            };
        }
        d
    }
    /// sub_16827.
    fn walk_descriptor(&self, si: usize) -> usize {
        let m = &self.mem;
        let w = |a: usize| u16::from_le_bytes([m[a], m[a + 1]]);
        let di = w(si + 4) as usize;
        let ax = w(di + 2).wrapping_sub(w(si + 6));
        let dx = (ax >> 8) as u8 as i8 as i16;
        let bx = (w(di + 4) as i16).wrapping_sub(w(si + 8) as i16);
        let mut bp = 2usize;
        let (adx, abx) = (dx.unsigned_abs(), bx.unsigned_abs());
        let mut v = bx;
        if adx >= abx {
            bp = 1;
            v = dx;
        }
        if v < 0 {
            bp ^= 2;
        }
        let class = ((m[si + 3] & 0x0F) >> 2) as usize;
        w(0x18BF + 2 * (class * 4 + bp)) as usize
    }
    /// sub_1686E: screen point of a troop (None = not drawn).
    fn troop_point(&self, si: usize) -> Option<(i32, i32)> {
        let m = &self.mem;
        if m[si + 3] & 0x40 != 0 {
            let (lon, row) = (self.get16(si + 6), self.get16(si + 8) as i16);
            let (x, y) = planet::map_to_screen(&self.mem, lon, row);
            return (x > -0x10 && y > -0x10 && x < 0x148 && y < 0xA0).then_some((x, y));
        }
        let di = self.get16(si + 4) as usize;
        let &(_, ix, iy) = self.mapst.icons.iter().find(|i| i.0 == di)?;
        let mut k = (m[si + 2].wrapping_sub(1)) as usize;
        if m[di + 0x0A] & 2 != 0 {
            k ^= 8;
        }
        k &= 15;
        let dx = m[0x1672 + 2 * k] as i8 as i32;
        let dy = m[0x1673 + 2 * k] as i8 as i32;
        Some((ix + dx, (iy & 0xFF) + dy))
    }
    /// sub_1C5CF: a new object centred on (x, y).
    fn new_obj(&mut self, assets: &mut Assets, bp: usize, x: i32, y: i32, owner: usize, flags: u8) -> Obj {
        let sprite = self.mem[bp];
        let (w, h) = assets.sheet("ONMAP.HSQ").and_then(|s| s.sprites.get(sprite as usize).map(|s| (s.width as i32, s.height as i32))).unwrap_or((8, 8));
        let (x0, y0) = (x - (w >> 1), y - (h >> 1));
        let mut o = Obj { x0, y0, x1: x0 + (w & 0xFFF), y1: y0 + h, sprite, owner, flags, cur: bp + 3, desc: bp };
        if self.mem[bp + 3] != 0 {
            let mut n = 0;
            while self.mem[bp + 3 * n] != 0 && n < 64 {
                n += 1;
            }
            let r = self.sim().rndr(n as u16 - 1) as usize;
            o.cur = bp + 3 * r;
            o.flags |= 1;
        }
        o
    }
    /// sub_16715: the troop objects of the map view.
    pub(crate) fn build_troop_objs(&mut self, assets: &mut Assets) {
        let mut objs = vec![];
        let places: Vec<usize> = self.mapst.icons.iter().map(|i| i.0).collect();
        let mut add = |w: &mut World, si: usize, objs: &mut Vec<Obj>| {
            let Some((x, y)) = w.troop_point(si) else { return };
            let bp = w.descriptor(si);
            if bp != 0 {
                let o = w.new_obj(assets, bp, x, y, si, 0);
                objs.push(o);
            }
        };
        for di in places {
            for si in self.sim().troop_ids(di) {
                add(self, si, &mut objs);
            }
        }
        let mut si = crate::sim::TROOPS;
        while si < crate::sim::TROOP_END {
            if self.mem[si + 0x10] & 0x10 == 0 && self.mem[si + 3] & 0x40 != 0 {
                add(self, si, &mut objs);
            }
            si += crate::sim::TROOP_SIZE;
        }
        self.tui.objs = objs;
        // sub_1697C: the selection highlight of the current troop.
        let sel = self.mem[0x1954];
        if sel != 0 && sel <= 0x43 {
            self.highlight(assets, troop_addr(sel));
        }
        self.dirty = true;
    }
    /// sub_1697C.
    fn highlight(&mut self, assets: &mut Assets, si: usize) {
        self.tui.objs.retain(|o| o.flags & 0x40 == 0);
        if !self.tui.objs.iter().any(|o| o.owner == si) {
            return;
        }
        if let Some((x, y)) = self.troop_point(si) {
            let o = self.new_obj(assets, 0x18FD, x, y, si, 0x40);
            self.tui.objs.push(o);
        }
    }
    fn unhighlight(&mut self) {
        self.tui.objs.retain(|o| o.flags & 0x40 == 0);
    }

    /// Task 0x6B34 (every 15 ticks).
    pub(crate) fn troop_anim_tick(&mut self) {
        self.tui.acc += 1;
        if self.tui.acc < 15 {
            return;
        }
        self.tui.acc = 0;
        self.tui.anim = self.tui.anim.wrapping_add(1);
        let all = self.tui.anim & 3 == 0;
        let mut changed = false;
        for i in 0..self.tui.objs.len() {
            let o = self.tui.objs[i];
            if o.flags & 1 == 0 || (!all && o.flags & 0x40 == 0) {
                continue;
            }
            let mut f = o.cur;
            if self.mem[f] == 0 {
                f = o.desc;
                if o.flags & 2 != 0 {
                    continue;
                }
            }
            let sprite = self.mem[f];
            let dx = self.mem[f + 1] as i8 as i32;
            let dy = self.mem[f + 2] as i8 as i32;
            let ob = &mut self.tui.objs[i];
            ob.sprite = sprite;
            ob.cur = f + 3;
            ob.x0 += dx;
            ob.x1 += dx;
            ob.y0 += dy;
            ob.y1 += dy;
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
    }

    /// sub_1C6AD on the map: objects in the C835 order (smallest x1 + y1 first, the later
    /// entry first on ties, overlays last).
    pub(crate) fn draw_troop_objs(&self, assets: &mut Assets, fb: &mut [u8]) {
        let Some(on) = assets.sheet("ONMAP.HSQ") else { return };
        let mut order: Vec<usize> = (0..self.tui.objs.len()).filter(|&i| self.tui.objs[i].flags & 0x80 == 0).collect();
        let key = |i: usize| {
            let o = &self.tui.objs[i];
            if o.flags & 0x40 != 0 {
                0x7FFF
            } else {
                o.x1 + o.y1
            }
        };
        order.sort_by(|&a, &b| key(a).cmp(&key(b)).then(b.cmp(&a)));
        let clip = Some((4, 4, 316, 148));
        for i in order {
            let o = &self.tui.objs[i];
            if let Some(s) = on.sprites.get(o.sprite as usize) {
                s.draw(fb, 320, o.x0, o.y0, DrawOpts { clip, ..Default::default() });
            }
        }
    }

    /// sub_16946: own troop under the mouse (strict box test, last entry first).
    pub(crate) fn troop_hit(&self, x: i32, y: i32) -> Option<usize> {
        for o in self.tui.objs.iter().rev() {
            if o.flags & 0x40 != 0 {
                continue;
            }
            if o.x0 < x && o.y0 < y && x < o.x1 && y < o.y1 {
                return (self.mem[o.owner + 3] < 0x80).then_some(o.owner);
            }
        }
        None
    }

    /// sub_1633B: vegetation icons (ONMAP 0x78 / 0x79) into the map background.
    pub(crate) fn draw_vegetation(&self, assets: &mut Assets, fb: &mut [u8]) {
        let Some(on) = assets.sheet("ONMAP.HSQ") else { return };
        let (lon_c, row_c) = (self.get16(0x197C), self.get16(0x197E) as i16);
        let clip = Some((4, 4, 316, 148));
        let mut row = row_c - 18;
        loop {
            let (xc, y) = planet::map_to_screen(&self.mem, lon_c, row);
            if !(4..148).contains(&y) || row > 98 || row < -98 {
                if y >= 148 || row > 98 {
                    break;
                }
                row += 1;
                continue;
            }
            let (di, w2) = planet::cell_ptr(&self.mem, lon_c, row);
            let w2 = w2 as i32;
            if w2 == 0 {
                row += 1;
                continue;
            }
            let start = planet::cell_ptr(&self.mem, 0, row).0 as i32;
            let col_c = di as i32 - start;
            let cellp = |c: i32| (start + c.rem_euclid(w2)) as usize;
            let mut draw = |c: i32, x: i32| {
                let p = cellp(c);
                let cell = self.map.get(p).copied().unwrap_or(0);
                if cell & 0x30 != 0x10 {
                    return;
                }
                let next = self.map.get(cellp(c + 1)).copied().unwrap_or(0);
                let spr = if next & 0x30 == 0x10 { 0x79 } else { 0x78 };
                let jy = (p as i32) & 3;
                let jx = ((w2 + p as i32) >> 2) & 3;
                if let Some(s) = on.sprites.get(spr) {
                    let (w, h) = (s.width as i32, s.height as i32);
                    s.draw(fb, 320, x + jx - (w >> 1), y + jy - (h >> 1), DrawOpts { clip, ..Default::default() });
                }
            };
            let (mut c, mut x) = (col_c, xc);
            while x < 316 {
                draw(c, x);
                c += 1;
                x += 4;
            }
            let (mut c, mut x) = (col_c, xc);
            while x >= 4 {
                draw(c, x);
                c -= 1;
                x -= 4;
            }
            row += 1;
        }
    }

    // ------------------------------------------------------------------ contact (§2.3)

    /// sub_168EB: the current troop.
    pub(crate) fn cur_troop(&self) -> usize {
        if self.mem[0x46EB] >= 0x80 {
            troop_addr(self.mem[0x1954])
        } else {
            self.get16(0x4758 + 2 * self.mem[0x476C] as usize) as usize
        }
    }

    /// 86CC: CONTACT FREMEN TROOPS / GIVE ORDERS TO TROOP.
    pub(crate) fn troop_contact(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.mapst.msgbox = false;
        if self.mem[0x28] == 0 {
            return;
        }
        if self.get16(0x1176) < 2 {
            let p = self.get16(0x114E) as usize;
            self.mem[0x1954] = self.mem[p + 9];
            let (lon, row) = self.cur_pos();
            self.set16(0x197C, lon);
            self.set16(0x197E, row as u16);
            self.map_redraw(assets);
            return self.troop_select(assets, audio);
        }
        let last = self.mem[0x1955];
        if self.mem[0x1954] == 0 && last != 0 && self.tui.objs.iter().any(|o| o.owner == troop_addr(last) && o.flags & 0x40 == 0) {
            self.mem[0x1954] = last;
            return self.troop_select(assets, audio);
        }
        self.mem[0x1954] = 0;
        self.troop_next(assets, audio);
    }

    /// 86FA: NEXT TROOP.
    pub(crate) fn troop_next(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let cur = self.mem[0x1954];
        let here = self.get16(0x114E);
        let near = self.get16(0x1176) < 2;
        let mut best: Option<(u8, u8)> = None;
        for o in &self.tui.objs {
            if o.flags & 0x40 != 0 {
                continue;
            }
            let si = o.owner;
            if self.mem[si + 3] >= 0x80 {
                continue;
            }
            if near && self.get16(si + 4) != here {
                continue;
            }
            let id = self.mem[si];
            let d = id.wrapping_sub(cur);
            if d != 0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, id));
            }
        }
        match best {
            Some((_, id)) if id != cur => {
                self.mem[0x1954] = id;
                self.troop_select(assets, audio);
            }
            _ if cur != 0 => self.contact_talk(assets, audio),
            _ => {}
        }
    }

    /// 8685: SELECT.
    pub(crate) fn troop_select(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.unhighlight();
        self.contact_end();
        self.map_deselect();
        self.tui.card = None;
        let id = self.mem[0x1954];
        if id == 0 || id > 0x43 {
            return;
        }
        let si = troop_addr(id);
        if self.mem[si + 3] >= 0x80 {
            return;
        }
        self.mem[0x1955] = id;
        self.highlight(assets, si);
        self.orders_menu(si);
        self.contact_open(assets, si);
        self.contact_talk(assets, audio);
    }

    /// sub_179EE: the contact window and the leader's face.
    fn contact_open(&mut self, assets: &mut Assets, si: usize) {
        self.set16(0x46EF, si as u16);
        let icon_x = self.tui.objs.iter().find(|o| o.owner == si && o.flags & 0x40 == 0).map(|o| o.x0).unwrap_or(0x100);
        let (y0, a) = if icon_x < 0x4C { (0x50, 0x0E) } else { (5, 0x1E) };
        self.set16(0x18EB, y0);
        self.set16(0x18EF, y0 + 0x43);
        self.set16(0x4710, 0x5C);
        self.set16(0x4712, a);
        self.tui.contact = Some(si);
        // Face: the Harkonnen captain when the troop is captured at a hostile place.
        let di = self.get16(si + 4) as usize;
        let c = if self.mem[si + 3] & 0x20 != 0 && (self.mem[di + 0x0A] & 2 != 0 || !self.sim().friendly(di)) {
            0x0C
        } else {
            self.set16(0x4758, si as u16);
            self.mem[0x476C] = 0;
            0x0F
        };
        self.set16(0x47C4, c);
        self.tui.face = None;
        let (idx, variant) = crate::face::bank_and_variant(c as usize, &self.mem);
        if let Some(bank) = assets.resource_name(idx + 2).and_then(|n| assets.dat.read(&n).ok()).and_then(dune_formats::face::FaceBank::parse) {
            let (fx, fy, _, _) = bank.rect;
            // ds:22B9: the part of the face shown in the 59x59 window (FRM1..3, captain).
            const CROP: [(u16, u16); 4] = [(72, 61), (84, 29), (56, 26), (47, 8)];
            let k = if c == 0x0C { 3 } else { (idx as usize).saturating_sub(0x0E).min(2) };
            let (fx, fy) = (fx + CROP[k].0, fy + CROP[k].1);
            let mut pal = self.mapst.pal.clone();
            bank.sheet.apply_palette(&mut pal);
            self.mapst.pal = pal;
            let bg = vec![0u8; VIEW];
            let params = crate::face::FaceParams { character: c as usize, variant, f0: 0, shift: 0, mouth_rect: None, words: 1, story_phase: self.mem[0x2A], seed: self.rng_seed() };
            let anim = crate::face::FaceAnim::new(bank, params, &bg, 0);
            // Crop the face (59x59 from the bank's rect origin).
            let mut face = vec![0u8; 59 * 59];
            for y in 0..59 {
                for x in 0..59 {
                    let (sx, sy) = (fx as usize + x, fy as usize + y);
                    if sx < 320 && sy < 152 {
                        face[y * 59 + x] = anim.back[sy * 320 + sx];
                    }
                }
            }
            self.tui.face = Some(face);
        }
        self.dirty = true;
    }

    /// sub_17B58: end of the contact.
    pub(crate) fn contact_end(&mut self) {
        if let Some(si) = self.tui.contact.take() {
            if self.mem[0x4C] == 0 {
                self.sim().contact_end_story(si);
                let v = self.get16(si + 0x10) & 0x03F0;
                self.set16(si + 0x10, v);
                let v = self.get16(si + 0x12) & 0xE5FF;
                self.set16(si + 0x12, v);
                let d = (self.clock() >> 4) as u8;
                self.mem[si + 0x14] = d;
            }
        }
        self.mem[0x4C] = 0;
        self.tui.face = None;
        self.set16(0x46EF, 0);
        self.dirty = true;
    }

    /// 7C02: CONTACT TALK (sub-list 2 of character 0x0F, resumed from ds:47BA).
    pub(crate) fn contact_talk(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let si = self.get16(0x46EF) as usize;
        if si == 0 {
            return;
        }
        self.sim().troop_vars(si);
        let di = self.get16(si + 4) as usize;
        self.sim().place_name_vars(di);
        let pos = self.cur_pos();
        let d = self.sim().troop_distance(si, pos);
        if d > self.get16(0x1176) {
            self.mem[0x4C] = 0xFF;
        }
        // sub_19719
        if (self.mem[0x4C] as i8) >= 0 {
            let place = self.get16(si + 4);
            self.drop_events_from(0x0F, place);
        }
        self.set16(0x47C4, 0x0F);
        self.set16(0x47A2, 0x10C8);
        self.mem[0x47A5] = 0xFF;
        let mut p = self.get16(0x47BA) as usize;
        if p == 0 || p == 0xFFFF {
            p = self.sublist_start(0x7A).unwrap_or(0);
        }
        let save = self.mem[0x47C2];
        self.mem[0x47C2] = 0x20;
        let r = self.pick_from(p);
        self.mem[0x47C2] = save;
        match r {
            Some(r) => {
                let next = self.say_record(assets, audio, r, After::Contact);
                self.set16(0x47BA, next as u16);
            }
            None => self.set16(0x47BA, 0),
        }
    }
    /// End of a contact line: "more information" (ds:47A5 = 0x80) shows the equipment.
    pub(crate) fn contact_said(&mut self, _assets: &mut Assets) {
        self.mem[0x46F4] = (self.mem[0x47A5] == 0x80) as u8;
        self.dirty = true;
    }
    /// sub_12A51: the queued reports of that speaker/place are dropped.
    fn drop_events_from(&mut self, dl: u8, place: u16) {
        let n = self.mem[0x1190] as usize;
        let mut kept = vec![];
        for k in 0..n {
            let a = 0x1191 + 4 * k;
            let (ax, di) = (self.get16(a), self.get16(a + 2));
            if (ax >> 8) as u8 == dl && (dl != 0x0F || di == place) {
                continue;
            }
            kept.push((ax, di));
        }
        if kept.len() != n {
            for k in 0..10 {
                let (ax, di) = kept.get(k).copied().unwrap_or((0, 0));
                if k < kept.len() || k < n {
                    self.set16(0x1191 + 4 * k, ax);
                    self.set16(0x1193 + 4 * k, di);
                }
            }
            self.mem[0x1190] = kept.len() as u8;
        }
    }

    // ------------------------------------------------------------------ orders (§2.4)

    /// sub_1780A: which orders menu.
    pub(crate) fn orders_menu(&mut self, si: usize) {
        self.tui.menus.clear();
        let pos = self.cur_pos();
        let d = self.sim().troop_distance(si, pos);
        let st = self.mem[si + 3];
        let m = if d > self.get16(0x1176) || (st & 0x20 != 0 && st != 0x22) {
            OrdMenu::Far
        } else if st & 0x40 != 0 {
            OrdMenu::Moving
        } else {
            OrdMenu::Orders
        };
        self.tui.menus.push(m);
        self.build_menu();
    }

    /// The map menu when orders are in progress (or None).
    pub(crate) fn orders_lines(&mut self) -> Option<Vec<Line>> {
        let m = *self.tui.menus.last()?;
        let si = self.cur_troop();
        let mut v = vec![];
        match m {
            OrdMenu::Orders => {
                // sub_17847
                let job = self.mem[si + 3] & 0x0F;
                let locked = self.get16(si + 0x12) & 0x400 != 0;
                let occ = !locked && job != 1;
                let mut occ_text = 0x4F;
                let (mut equip, mut mv) = (false, false);
                if !locked && job != 2 {
                    mv = self.mem[0x2A] >= 5;
                    let di = self.get16(si + 4) as usize;
                    if self.mem[0x2A] >= 4 && self.get16(si + 0x10) & 0x200 == 0 && (self.mem[di + 8] < 0x28 || self.mem[di + 0x0A] & 8 != 0) {
                        self.sim().free_stock(di);
                        equip = (0..7).any(|k| self.mem[0x46FE + k] != 0) || self.mem[si + 0x19] != 0;
                    }
                } else if !locked && job == 2 {
                    occ_text = 0x56;
                }
                v.push(Line::new(0x4D, Cmd::Ord(1), true));
                v.push(Line::new(occ_text, Cmd::Ord(2), occ));
                v.push(Line::new(0x4E, Cmd::Ord(3), equip));
                v.push(Line::new(0x50, Cmd::Ord(4), mv));
                v.push(Line::new(if self.mem[0x46F3] != 0 { 0x52 } else { 0x53 }, Cmd::Ord(5), true));
            }
            OrdMenu::Far => {
                v.push(Line::new(0x51, Cmd::Ord(6), true));
                v.push(Line::new(0x52, Cmd::Ord(5), true));
            }
            OrdMenu::Moving => {
                v.push(Line::new(0x58, Cmd::Ord(4), true));
                v.push(Line::new(0x51, Cmd::Ord(6), true));
                v.push(Line::new(0xA3, Cmd::Ord(9), true));
            }
            OrdMenu::Occupation(menu) => {
                let ph = self.mem[0x2A] >= 0x10;
                let eco = self.mem[0x0A] & 0x20 != 0;
                let lines: &[(u16, u8)] = match menu {
                    0x215A => &[(0x75, 20), (0x76, 21), (0x77, 22)],
                    0x216E => &[(0x5D, 23), (0x76, 21), (0x77, 22)],
                    0x2182 => &[(0x5D, 24), (0x5F, 25), (0x75, 20), (0x77, 22)],
                    0x219A => &[(0x60, 26)],
                    _ => &[(0x5D, 27), (0x61, 28), (0x75, 20), (0x76, 21)],
                };
                for (i, &(id, c)) in lines.iter().enumerate() {
                    let mut en = true;
                    if i == 0 && menu != 0x219A && menu != 0x215A && !ph {
                        en = false;
                    }
                    if menu == 0x215A && i == 2 {
                        en = false;
                    }
                    if id == 0x5F && self.get16(0xE2) >= 0x1E {
                        en = false;
                    }
                    if id == 0x77 && !eco {
                        en = false;
                    }
                    v.push(Line::new(id, Cmd::Ord(c), en));
                }
                v.push(Line::new(0xA3, Cmd::Ord(9), true));
            }
            OrdMenu::MoveWait { prospect } => {
                if prospect {
                    v.push(Line::new(0x5B, Cmd::Ord(30), self.tui.picks.len() < 3));
                    v.push(Line::new(0x5C, Cmd::Ord(31), true));
                    v.push(Line::new(0xA1, Cmd::Ord(32), true));
                }
                v.push(Line::new(0xA3, Cmd::Ord(9), true));
            }
            OrdMenu::Equipment => v.push(Line::new(0xA1, Cmd::Ord(40), true)),
        }
        Some(v)
    }

    /// The orders handlers.
    pub(crate) fn order(&mut self, assets: &mut Assets, audio: &mut Audio, n: u8) {
        let si = self.cur_troop();
        match n {
            1 => {
                if self.mem[0x46F5] != 0 {
                    return;
                }
                self.contact_talk(assets, audio)
            }
            2 => self.change_occupation(si),
            3 => self.modify_equipment(assets, audio, si),
            4 => self.move_troop(si),
            5 => self.no_more_orders(assets, audio),
            6 => self.troop_next(assets, audio),
            9 => {
                self.tui.menus.pop();
                if self.tui.menus.is_empty() {
                    self.orders_exit();
                }
                self.build_menu();
            }
            20 => {
                let cl = if si == PROSPECTORS { 1 } else { 0 };
                self.setjob_confirm(assets, audio, si, cl);
            }
            21 => self.setjob_confirm(assets, audio, si, 4),
            22 => self.setjob_confirm(assets, audio, si, 8),
            23 | 24 | 27 => self.go_search(assets, audio, si),
            25 => self.espionage(assets, audio, si),
            26 => {
                let cl = (self.mem[si + 3] & 0x0C) | 2;
                self.setjob_confirm(assets, audio, si, cl)
            }
            28 => {
                let cl = (self.mem[si + 3] & 0x0C) | 1;
                self.setjob_confirm(assets, audio, si, cl)
            }
            30 => {}
            31 => {
                self.tui.picks.clear();
                self.build_menu();
            }
            32 => self.move_done(assets, audio, si),
            40 => self.equipment_done(assets, audio, si),
            _ => {}
        }
        self.dirty = true;
    }

    /// 8751: exit callback of the orders menu.
    fn orders_exit(&mut self) {
        if self.mem[0x1954] != 0 {
            self.unhighlight();
            self.mem[0x1954] = 0;
            self.contact_end();
        }
        self.tui.menus.clear();
    }

    /// 8763: NO MORE ORDERS / CUT CONTACT.
    pub(crate) fn no_more_orders(&mut self, assets: &mut Assets, _audio: &mut Audio) {
        let from_room = self.mem[0x46F3] != 0;
        self.orders_exit();
        self.mem[0x46F3] = 0;
        self.build_menu();
        if from_room {
            self.map_leave(assets);
            self.mem[0xFB] = 1;
        }
    }

    /// 69B3: CHANGE TROOP OCCUPATION.
    fn change_occupation(&mut self, si: usize) {
        let job = self.mem[si + 3] & 0x0F;
        let menu = if job == 2 {
            0x215A
        } else {
            match job >> 2 {
                0 => 0x216E,
                1 => {
                    if job == 5 {
                        0x219A
                    } else {
                        0x2182
                    }
                }
                _ => 0x21A6,
            }
        };
        self.tui.menus.push(OrdMenu::Occupation(menu));
        self.build_menu();
    }

    /// loc_16A89: SETJOB-CONFIRM.
    fn setjob_confirm(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize, cl: u8) {
        if self.mem[si + 3] & 0x0F == cl {
            self.tui.menus.pop();
            self.build_menu();
            return;
        }
        let saved3 = self.mem[si + 3];
        let saved12 = self.get16(si + 0x12);
        self.sim().set_job_if(si, cl);
        if si == PROSPECTORS && cl == 1 {
            // 6A80: the room menu is rebuilt (sub_12EBF).
        }
        self.mem[0x47A5] = 0xFF;
        self.confirm(assets, audio, 0x0A, After::SetJob { si, saved3, saved12 });
    }
    pub(crate) fn setjob_answer(&mut self, si: usize, saved3: u8, saved12: u16) {
        if self.mem[0x47A5] == 0xFF {
            if self.sim().class(si) != 0 {
                self.mem[si + 0x19] &= 0x7F;
            }
        } else {
            self.set16(si + 0x12, saved12);
            self.sim().set_job_if(si, saved3 & 0x0F);
        }
        self.tui.menus.pop();
        if let Some(OrdMenu::Occupation(_)) = self.tui.menus.last() {
            self.tui.menus.pop();
        }
        self.icons_dirty = true;
        self.build_menu();
    }

    /// 6A45: ESPIONAGE.
    fn espionage(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        let cl = (self.mem[si + 3] & 0x0C) | 1;
        self.sim().set_job_if(si, cl);
        self.mem[si + 0x10] &= 0xBF;
        let di = self.get16(si + 4) as usize;
        self.sim().analyse(di);
        let dest = self.get16(0xE4) as usize;
        self.confirm_move(assets, audio, si, dest, After::Espionage { si, di: dest });
    }
    pub(crate) fn espionage_answer(&mut self, assets: &mut Assets, si: usize, di: usize) {
        if self.mem[0x47A5] == 0xFF {
            self.end_orders(assets);
            self.sim().depart(si, di);
        } else {
            self.sim().job_base(si);
        }
        self.icons_dirty = true;
        self.build_menu();
    }

    /// sub_182DA: "go there?" confirmation.
    fn confirm_move(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize, di: usize, after: After) {
        let al = if di == self.get16(si + 4) as usize && si != PROSPECTORS {
            0x10
        } else {
            let old = self.get16(si + 4);
            self.set16(si + 4, di as u16);
            self.sim().troop_vars(si);
            self.sim().place_name_vars(di);
            self.set16(si + 4, old);
            0x0B
        };
        self.mem[0x47A5] = 0xFF;
        self.confirm(assets, audio, al, after);
    }

    /// 8770.
    fn end_orders(&mut self, _assets: &mut Assets) {
        if self.mem[0x1954] != 0 {
            self.mem[0x46F3] = 0;
            self.set16(0x1BEA, 0);
        }
        self.orders_exit();
        self.build_menu();
    }

    /// 776D / 7734 / 775C: GO & SEARCH FOR EQUIPMENT.
    fn go_search(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        let al = self.mem[si + 0x19];
        let k: Option<u8> = match self.sim().class(si) {
            0 => {
                if al & 0x80 == 0 {
                    Some(0)
                } else if al & 0x40 == 0 {
                    Some(1)
                } else {
                    None
                }
            }
            1 => (2u8..=5).find(|&k| al & (0x80 >> k) == 0),
            _ => (al & 2 == 0).then_some(6),
        };
        let Some(k) = k else {
            self.tui.menus.pop();
            self.build_menu();
            return self.confirm(assets, audio, 0x0F, After::None);
        };
        self.set16(0x1203, 0xE8 + k as u16);
        // sub_177D7: take it here first.
        let di = self.get16(si + 4) as usize;
        self.sim().free_stock(di);
        let need = if k == 1 && di == self.get16(0x1150) as usize { 2 } else { 1 };
        if self.mem[0x46FE + k as usize] >= need {
            self.mem[0x3D] = self.mem[si + 0x19];
            self.mem[si + 0x19] |= 0x80 >> k;
            self.tui.menus.pop();
            return self.equip_done_talk(assets, audio, si);
        }
        self.sim().find_item(si, k);
        if self.get16(0x470C) == 0xFFFF {
            self.tui.menus.pop();
            self.build_menu();
            return self.confirm(assets, audio, 0x0E, After::None);
        }
        let dest = self.get16(0x470E) as usize;
        let cl = (self.mem[si + 3] & 0x0C) | 3;
        let saved3 = self.mem[si + 3];
        let saved12 = self.get16(si + 0x12);
        let _ = (saved3, saved12);
        self.search_saved = Some((saved3, saved12));
        self.sim().set_job_if(si, cl);
        self.mem[0x47A5] = 0xFF;
        self.confirm(assets, audio, 0x0A, After::Search { si, k, di: dest });
    }
    pub(crate) fn search_answer(&mut self, assets: &mut Assets, si: usize, k: u8, di: usize) {
        let (saved3, saved12) = self.search_saved.take().unwrap_or((self.mem[si + 3], self.get16(si + 0x12)));
        if self.mem[0x47A5] != 0xFF {
            self.set16(si + 0x12, saved12);
            self.sim().set_job_if(si, saved3 & 0x0F);
            self.tui.menus.pop();
            self.build_menu();
            return;
        }
        if self.sim().class(si) != 0 {
            self.mem[si + 0x19] &= 0x7F;
        }
        let e = (k as u16) | (((0x80u16 >> k) & 0xFF) << 8);
        self.set16(si + 0x0E, e);
        let home = self.get16(si + 4);
        self.set16(si + 0x0C, home);
        self.sim().depart(si, di);
        self.end_orders(assets);
        self.icons_dirty = true;
    }

    /// 8064: MOVE TROOP / CHANGE DESTINATION.
    fn move_troop(&mut self, si: usize) {
        let prospect = si == PROSPECTORS;
        self.tui.picks.clear();
        if prospect {
            for k in 0..3 {
                let p = self.get16(0x11D3 + 2 * k) as usize;
                if p != 0 {
                    self.tui.picks.push(p);
                }
            }
        }
        let id = if prospect { 0x55 } else { 0x54 };
        self.say_prompt(id);
        self.tui.menus.push(OrdMenu::MoveWait { prospect });
        self.build_menu();
    }
    /// Click 81EC on the map while waiting for a destination.
    pub(crate) fn move_click(&mut self, assets: &mut Assets, audio: &mut Audio, rec: usize) {
        let Some(OrdMenu::MoveWait { prospect }) = self.tui.menus.last().copied() else { return };
        let si = self.cur_troop();
        if prospect {
            if !(self.mem[rec + 8] < 0x20 || self.mem[rec + 0x0A] & 8 != 0) {
                return;
            }
            if self.tui.picks.len() >= 3 {
                self.tui.picks.clear();
            }
            self.tui.picks.push(rec);
            self.build_menu();
            if self.tui.picks.len() == 3 {
                self.move_done(assets, audio, si);
            }
            return;
        }
        self.confirm_move(assets, audio, si, rec, After::Move { si, di: rec });
    }
    /// 8214: Done.
    fn move_done(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        if si != PROSPECTORS {
            return;
        }
        for k in 0..3 {
            let v = self.tui.picks.get(k).copied().unwrap_or(0) as u16;
            self.set16(0x11D3 + 2 * k, v);
        }
        let di = self.get16(0x11D3) as usize;
        if di == 0 {
            self.tui.menus.pop();
            self.build_menu();
            return;
        }
        self.confirm_move(assets, audio, si, di, After::Move { si, di });
    }
    pub(crate) fn move_answer(&mut self, assets: &mut Assets, si: usize, di: usize) {
        self.tui.menus.pop();
        if self.mem[0x47A5] == 0xFF {
            self.sim().depart(si, di);
            self.icons_dirty = true;
            if self.mem[si + 3] & 0x40 != 0 {
                self.end_orders(assets);
            }
        }
        self.build_menu();
    }

    /// Text shown in the contact subtitle box (prompt 0x54 / 0x55).
    fn say_prompt(&mut self, id: u16) {
        let t = self.cmd_t(id);
        self.prompt = Some(t);
        self.dirty = true;
    }

    // ------------------------------------------------------------------ equipment (§2.10)

    /// 7CBB: MODIFY EQUIPMENT.
    fn modify_equipment(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        let _ = (assets, audio);
        self.mem[0x46F5] = 1;
        self.tui.menus.push(OrdMenu::Equipment);
        self.mem[0x3D] = self.mem[si + 0x19];
        let e = self.mem[si + 0x19];
        for k in 0..7 {
            self.mem[0x4705 + k] = ((e << k) & 0x80 != 0) as u8;
        }
        let di = self.get16(si + 4) as usize;
        self.sim().free_stock(di);
        self.build_menu();
    }
    /// 7D68 + 7D81: Done.
    fn equipment_done(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        self.mem[0x46F5] = 0;
        self.tui.menus.pop();
        let mut v = 0u8;
        for k in 0..7 {
            if self.mem[0x4705 + k] != 0 {
                v |= 0x80 >> k;
            }
        }
        self.mem[si + 0x19] = v;
        self.equip_done_talk(assets, audio, si);
    }
    /// EQUIP-DONE 7D81.
    fn equip_done_talk(&mut self, assets: &mut Assets, audio: &mut Audio, si: usize) {
        self.icons_dirty = true;
        let old = self.mem[0x3D];
        let new = self.mem[si + 0x19];
        self.mem[0x3D] = new & !old;
        self.mem[0x3E] = old & !new;
        self.mem[0x3F] = if new & !old & 0x40 != 0 && self.get16(si + 4) == self.get16(0x1150) { 0x40 } else { 0 };
        if self.mem[si + 3] & 0x0F == 8 {
            self.sim().precondition(si);
        }
        self.build_menu();
        self.confirm(assets, audio, 0x0C, After::EquipDone);
    }
    /// Clicks in the equipment panels (7E97 / 7EB8).
    pub(crate) fn equipment_click(&mut self, x: i32, y: i32) -> bool {
        if self.mem[0x46F5] == 0 {
            return false;
        }
        let (t, p) = self.equip_rects();
        let inside = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.2 && y >= r.1 && y < r.3;
        let hit = |w: &World, base: usize| -> Option<usize> {
            for k in 0..7 {
                let (a, b) = (w.get16(base + 4 * k) as i32, w.get16(base + 4 * k + 2) as i32);
                if a != 0 && x >= a && x < b {
                    return Some(k);
                }
            }
            None
        };
        if inside(t) {
            if let Some(k) = hit(self, 0x4C7C) {
                if self.mem[0x4705 + k] != 0 {
                    self.mem[0x4705 + k] -= 1;
                    self.mem[0x46FE + k] += 1;
                }
            }
            self.dirty = true;
            return true;
        }
        if inside(p) {
            if let Some(k) = hit(self, 0x4C60) {
                if self.mem[0x4705 + k] == 0 && self.mem[0x46FE + k] != 0 {
                    self.mem[0x4705 + k] = 1;
                    self.mem[0x46FE + k] -= 1;
                }
            }
            self.dirty = true;
            return true;
        }
        false
    }
    /// 7DD9: the troop panel T (ds:1936) and the sietch panel P (ds:1940).
    fn equip_rects(&self) -> ((i32, i32, i32, i32), (i32, i32, i32, i32)) {
        let w = (self.get16(0x18E9) as i32, self.get16(0x18EB) as i32, self.get16(0x18ED) as i32, self.get16(0x18EF) as i32);
        let px0 = w.0 + 0x30;
        let mut py0 = w.3 + 1;
        if py0 >= 0x70 {
            py0 -= 0x6D;
        }
        let p = (px0, py0, w.2, py0 + 0x28);
        let t = (p.0 + 0x4D, w.1 + 0x2B, w.2, w.3);
        (t, p)
    }

    // ------------------------------------------------------------------ drawing

    /// Contact window, equipment panels, prompt, troop card.
    pub(crate) fn troop_overlays(&mut self, assets: &mut Assets, fb: &mut [u8]) {
        if let Some(si) = self.tui.contact {
            let (x0, y0, x1, y1) = (self.get16(0x18E9) as i32, self.get16(0x18EB) as i32, self.get16(0x18ED) as i32, self.get16(0x18EF) as i32);
            frame(fb, (x0, y0, x1, y1), self.mem[0x18F2], self.mem[0x18F1]);
            // Face rect ds:18F3.
            let (fx, fy) = (x0 + 4, y0 + 3);
            rect(fb, fx, fy, fx + 0x3D, fy + 0x3D, self.mem[0x18F1]);
            if let Some(face) = &self.tui.face {
                for y in 0..59 {
                    for x in 0..59 {
                        let c = face[y * 59 + x];
                        if c != 0 {
                            let p = (fy + 1 + y as i32) as usize * 320 + (fx + 1 + x as i32) as usize;
                            if p < fb.len() {
                                fb[p] = c;
                            }
                        }
                    }
                }
            }
            // Subtitle box ds:2244 (x0 + 0x49, y0 + 3).
            let text = self.prompt.clone().or_else(|| self.talk_page());
            if let Some(t) = text {
                let mut y = y0 + 3;
                for l in assets.font2.wrap(&t, (x1 - x0 - 0x4D).max(8) as usize) {
                    crate::world::text(fb, &assets.font2, x0 + 0x49, y, &l, 0xF0, 0);
                    y += 7;
                    if y > y1 - 7 {
                        break;
                    }
                }
            }
            if self.mem[0x46F4] != 0 || self.mem[0x46F5] != 0 {
                self.mem[0x46FA] = 0;
                let e = self.mem[si + 0x19];
                let items: Vec<u8> = if self.mem[0x46F5] != 0 { (0..7).map(|k| self.mem[0x4705 + k]).collect() } else { (0..7).map(|k| (e << k) & 0x80 != 0).map(|b| b as u8).collect() };
                let (t, p) = self.equip_rects();
                if self.mem[0x46F5] != 0 {
                    frame(fb, t, self.mem[0x1937 + 8], self.mem[0x1936 + 8]);
                    frame(fb, p, self.mem[0x1941 + 8], self.mem[0x1940 + 8]);
                    let stock: Vec<u8> = (0..7).map(|k| self.mem[0x46FE + k]).collect();
                    let t6f = self.cmd_t(0x6F);
                    crate::world::text_lines(fb, &assets.font2, p.0 + 8, p.1 + 8, &t6f, 0xFB, 0);
                    let xr = self.equip_columns(assets, fb, &stock, p.0 + 0x50, p.1 + 4, p.3, self.mem[0x1948]);
                    for (k, r) in xr.iter().enumerate() {
                        self.set16(0x4C60 + 4 * k, r.0 as u16);
                        self.set16(0x4C62 + 4 * k, r.1 as u16);
                    }
                }
                let xr = self.equip_columns(assets, fb, &items, x0 + 0x80, y0 + 0x2D, y1, 0xF0);
                for (k, r) in xr.iter().enumerate() {
                    self.set16(0x4C7C + 4 * k, r.0 as u16);
                    self.set16(0x4C7E + 4 * k, r.1 as u16);
                }
            }
        }
        if let Some(si) = self.tui.card {
            self.draw_card(assets, fb, si);
        }
    }

    pub(crate) fn talk_page(&self) -> Option<Vec<u8>> {
        self.talk.as_ref().and_then(|t| t.pages.get(t.page).cloned())
    }

    /// sub_17E3D: item columns (count copies stacked downward, ONMAP ds:192F[k]); returns
    /// the x ranges of the items.
    fn equip_columns(&self, assets: &mut Assets, fb: &mut [u8], counts: &[u8], x: i32, y: i32, bottom: i32, _col: u8) -> Vec<(i32, i32)> {
        let mut ranges = vec![(0, 0); 7];
        if counts.iter().all(|&c| c == 0) {
            let t = self.cmd_t(0x69);
            crate::world::text(fb, &assets.font2, x + 0x0C, y + 5, &t, 0xF0, 0);
            return ranges;
        }
        let Some(on) = assets.sheet("ONMAP.HSQ") else { return ranges };
        let mut dx = x;
        for (k, &n) in counts.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let spr = self.mem[0x192F + k] as usize;
            let Some(s) = on.sprites.get(spr) else { continue };
            let (w, h) = (s.width as i32 & 0xFFF, s.height as i32);
            let x0 = dx;
            // sub_161D3: vertical packing.
            let avail = bottom - y;
            let mut n = n as i32;
            let mut step = h + 2;
            if avail / n.max(1) < step {
                step = (avail - h) / n.max(1);
                if step < 2 {
                    step = 2;
                    n = (avail - h) / 2;
                }
            }
            for i in 0..n.max(0) {
                s.draw(fb, 320, dx, y + i * step, DrawOpts::default());
            }
            dx += w;
            ranges[k] = (x0, dx);
            dx += 1;
        }
        ranges
    }

    /// sub_178E9: the troop card.
    fn draw_card(&mut self, assets: &mut Assets, fb: &mut [u8], si: usize) {
        self.sim().troop_vars(si);
        let v = self.get16(0x11F3) + 0x0C;
        self.set16(0x11F3, v);
        let (x0, y0) = (self.get16(0x18DF) as i32, self.get16(0x18E1) as i32);
        let (x1, y1) = (self.get16(0x18E3) as i32, self.get16(0x18E5) as i32);
        frame(fb, (x0, y0, x1, y1), self.mem[0x18E8], self.mem[0x18E7]);
        let font = &assets.font2;
        let put = |fb: &mut [u8], w: &World, id: u16, x: i32, y: i32, c: u8| {
            let raw = w.cmd_t(id);
            let t = crate::talk::expand(&w.mem, assets, &raw);
            crate::world::text_lines(fb, font, x, y, &t, c, 0).1
        };
        let st = self.mem[0x30];
        let mut y = y0 + 4;
        put(fb, self, if st & 0x40 != 0 { 0x3B } else { 0x3A }, x0 + 12, y, 0x9A);
        y += 9;
        let di = self.get16(0x2C) as usize;
        let name = self.place_name_raw(di);
        crate::world::text(fb, font, x0 + 4, y, &name, 0x96, 0);
        y += 10;
        if st & 0x20 != 0 {
            put(fb, self, if st == 0x22 { 0x42 } else { 0x41 }, x0 + 4, y, 0x9A);
            y += 0x11;
        } else {
            put(fb, self, 0x3C, x0 + 4, y, 0x9A);
            y += 0x0F;
            if st != 2 {
                let a = 0x11F7 + ((self.mem[0x2F] >> 1) & 6) as usize;
                let id = self.get16(a);
                put(fb, self, id, x0 + 4, y, 0x9A);
                y += 10;
            }
            if st & 0x40 == 0 {
                let id = if self.get16(0x32) & 0x200 != 0 {
                    Some(0x3F)
                } else if self.get16(0x32) & 0x100 == 0 || self.get16(0x34) & 0x30 != 0 {
                    Some(0x40)
                } else if st == 0 {
                    Some(0x3D)
                } else if self.mem[0x2F] == 1 {
                    Some(0x43)
                } else if st == 6 {
                    Some(0x3E)
                } else {
                    None
                };
                if let Some(id) = id {
                    put(fb, self, id, x0 + 4, y, 0x9A);
                    y += 0x11;
                }
            }
        }
        y += 4;
        put(fb, self, 0x6E, x0 + 4, y, 0x96);
        y += 8;
        let e = self.mem[si + 0x19];
        let items: Vec<u8> = (0..7).map(|k| ((e << k) & 0x80 != 0) as u8).collect();
        self.equip_columns(assets, fb, &items, x0 + 4, y, y1, 0xF0);
    }

    /// sub_178BC: right click on a troop icon.
    pub(crate) fn troop_card(&mut self, si: usize) {
        if self.tui.card == Some(si) {
            self.tui.card = None;
            self.dirty = true;
            return;
        }
        let pos = self.cur_pos();
        let d = self.sim().troop_distance(si, pos);
        if d >= self.get16(0x1176) {
            return;
        }
        let Some(o) = self.tui.objs.iter().find(|o| o.owner == si && o.flags & 0x40 == 0).copied() else { return };
        // sub_15F25 with height 0x64.
        let cx = 0x64;
        let mut by = o.y0 - cx / 2;
        if by < 4 {
            by = 4;
        }
        if by >= 0x94 - cx {
            by = 0x94 - cx;
        }
        let mut dx = o.x0 + 0x0F;
        if dx >= 0xD2 {
            dx -= 0x82;
        }
        self.set16(0x18DF, dx as u16);
        self.set16(0x18E1, by as u16);
        self.set16(0x18E3, (dx + 0x6A) as u16);
        self.set16(0x18E5, (by + cx) as u16);
        self.set16(0x46FA, si as u16);
        self.mem[0x1955] = self.mem[si];
        self.tui.card = Some(si);
        self.dirty = true;
    }

    /// 9584: OVERPOWER THE PRISONER (once per character).
    pub(crate) fn overpower(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let rec = self.get16(0x47A2) as usize;
        let cl = self.mem[rec + 0x0E] as u32;
        let bit = 1u16.wrapping_shl(cl);
        if self.get16(0xEE) & bit == 0 {
            let v = self.get16(0xEE) | bit;
            self.set16(0xEE, v);
            let si = self.get16(0x4768) as usize;
            self.mem[si + 0x15] = self.mem[si + 0x15].wrapping_sub(0x29);
            let (v, _) = self.mem[0xED].overflowing_sub(0x29);
            self.mem[0xED] = v;
            if (v as i8) < 0 {
                self.mem[si + 3] |= 0x10;
                self.mem[0x10A7] |= 0x10;
            }
        }
        let c = self.get16(0x47C4) as usize;
        self.talk_sub(assets, audio, 0x10, 5, After::None);
        self.set16(0x47C4, c as u16);
    }

    /// 5B1E: FIND PROSPECTORS.
    pub(crate) fn find_prospectors(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.mem[0x1955] = 3;
        let si = PROSPECTORS;
        self.set16(0x197C, self.get16(si + 6));
        self.set16(0x197E, self.get16(si + 8));
        self.map_redraw(assets);
        if self.get16(0x1176) <= 1 && (self.mem[si + 3] & 0x40 != 0 || self.get16(si + 4) != self.get16(0x114E)) {
            return;
        }
        if self.mem[0x46EB] & 0x40 != 0 {
            return;
        }
        self.troop_contact(assets, audio);
    }

    pub(crate) fn place_name_raw(&self, rec: usize) -> Vec<u8> {
        if rec < 0x100 || rec >= 0x8A8 {
            return vec![];
        }
        let (b0, b1) = (self.mem[rec] as u16, self.mem[rec + 1] as u16);
        let mut s = self.cmd_t(b0);
        s.push(if b1 < 3 { b' ' } else { b'-' });
        s.extend(self.cmd_t(b1 + 0x0C));
        s
    }
}

/// sub_17B1B: a framed box (fill, outline).
pub(crate) fn frame(fb: &mut [u8], r: (i32, i32, i32, i32), fill_c: u8, outline: u8) {
    fill(fb, r.0, r.1, r.2, r.3, fill_c);
    rect(fb, r.0, r.1, r.2 - 1, r.3 - 1, outline);
}

/// MOIS.HSQ (bank 0x31) animation (troops.md §4.6): parts {sprite (1-based), x, y}.
const MOIS_PARTS: [&[(u8, i32, i32)]; 13] = [
    &[],
    &[],
    &[(4, 39, 8)],
    &[(13, 8, 46), (10, 86, 48)],
    &[(2, 39, 8)],
    &[(13, 8, 46), (9, 73, 61), (3, 11, 84), (8, 86, 48)],
    &[(1, 39, 8)],
    &[(5, 39, 8)],
    &[(12, 39, 8)],
    &[(14, 39, 8)],
    &[(7, 39, 8)],
    &[(11, 39, 8)],
    &[(6, 39, 8)],
];
const MOIS_BODY: [usize; 9] = [2, 4, 6, 7, 8, 9, 10, 11, 12];

pub struct Harvester {
    pub x: i32,
    pub y: i32,
    pub frame: usize,
    pub acc: u32,
    pub clean: Vec<u8>,
}

impl World {
    /// loc_14D06: the harvesters of the last place seen from the desert.
    pub(crate) fn desert_harvester(&mut self, assets: &mut Assets, pal: &mut dune_formats::palette::Palette) {
        self.mem[0xF6] = 0;
        let p = self.get16(0x1150) as usize;
        if p < 0x100 || p >= 0x8A8 {
            return;
        }
        // sub_14DED: ds:4733 = (place harvesters << 8) | mining troops with a harvester.
        let mut n = 0u8;
        for si in self.sim().troop_ids(p) {
            if self.mem[si + 3] == 0 && self.mem[si + 0x19] & 0x80 != 0 {
                n += 1;
            }
        }
        let ah = self.mem[p + 0x14];
        if self.get16(4) != self.get16(p + 2) || ah == 0 || self.mem[6] != self.mem[p + 4] {
            return;
        }
        let bh = self.mem[7];
        if bh > ah {
            return;
        }
        self.mem[0xF6] = 1;
        let Some(mois) = assets.sheet("MOIS.HSQ") else { return };
        mois.apply_palette(pal);
        let (x, y) = ((5 + 4 * bh as i32) & 0xFF, 0x29);
        draw_parts(&mut self.view, &mois, 3, x, y);
        if bh <= n {
            self.pending_voc = Some("SN5.VOC");
            self.harvester = Some(Harvester { x, y, frame: 0, acc: 0, clean: self.view.clone() });
        }
    }

    /// Task 0x4BB9 (16 ticks): group 0, body parts cycling while parts 3 / 5 alternate.
    pub(crate) fn harvester_tick(&mut self, assets: &mut Assets) {
        if self.closeup.is_some() || self.talk_menu_for.is_some() || !self.menu_is_room() {
            return;
        }
        let Some(h) = &mut self.harvester else { return };
        h.acc += 1;
        if h.acc < 16 {
            return;
        }
        h.acc = 0;
        let Some(mois) = assets.sheet("MOIS.HSQ") else { return };
        let f = h.frame;
        h.frame = (h.frame + 1) % 36;
        let (x, y) = (h.x, h.y);
        self.view.copy_from_slice(&h.clean);
        draw_parts(&mut self.view, &mois, MOIS_BODY[f % 9], x, y);
        draw_parts(&mut self.view, &mois, if f & 1 == 0 { 3 } else { 5 }, x, y);
        self.ripple_fb.copy_from_slice(&self.view);
        self.dirty = true;
    }

    fn menu_is_room(&self) -> bool {
        self.scr == Scr::Room && self.argue.is_none() && self.msg_menu.is_none() && self.talk.is_none()
    }
}

fn draw_parts(fb: &mut [u8], sh: &dune_formats::sprite::SpriteSheet, part: usize, x: i32, y: i32) {
    for &(s, dx, dy) in MOIS_PARTS[part] {
        if let Some(sp) = sh.sprites.get(s as usize - 1) {
            sp.draw(fb, 320, x + dx, y + dy, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
        }
    }
}
