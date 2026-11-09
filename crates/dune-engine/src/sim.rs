//! World simulation: troops, the spice economy and the war, ported routine by routine from
//! DNCDPRG.EXE (docs/re/troops.md, docs/re/war.md). Everything works on the mirror of the
//! original data segment (`m`), so addresses are the original ds: offsets and saved games
//! stay compatible.
//!
//! Routine names in comments are the original seg000 addresses (`sub_1XXXX`).
//! UI side effects of the original routines (sprite refreshes, popups) are reduced to the
//! request flags the original also uses (ds:46EC map redraw, ds:473B room refresh) plus
//! `Sim::icons` (the map troop objects must be rebuilt).

use dune_formats::planet;

pub const TROOPS: usize = 0x08AA;
pub const TROOP_END: usize = 0x0FBB;
pub const TROOP_SIZE: usize = 0x1B;
/// ds:0FBC: dummy place of destroyed troops.
pub const NOWHERE: usize = 0x0FBC;
/// Troop 3: the prospectors.
pub const PROSPECTORS: usize = 0x08E0;
/// Location record 2: the Harkonnen palace.
pub const HARK_PALACE: usize = 0x011C;
pub const MAP_CELLS: usize = 0xC5F9;

pub fn troop_addr(id: u8) -> usize {
    TROOPS + TROOP_SIZE * (id as usize).wrapping_sub(1)
}

pub struct Sim<'a> {
    pub m: &'a mut [u8],
    pub map: &'a mut [u8],
    /// cs:65B4, the vegetation-decay LFSR (code segment: not saved).
    pub lfsr: &'a mut u16,
    /// Some troop object on the map changed (sub_18461 / sub_15D50 / sub_1C58A requests).
    pub icons: bool,
}

impl<'a> Sim<'a> {
    pub fn new(m: &'a mut [u8], map: &'a mut [u8], lfsr: &'a mut u16) -> Sim<'a> {
        Sim { m, map, lfsr, icons: false }
    }

    // ------------------------------------------------------------------ memory helpers
    #[inline]
    pub fn b(&self, a: usize) -> u8 {
        self.m[a & 0xFFFF]
    }
    #[inline]
    pub fn sb(&mut self, a: usize, v: u8) {
        self.m[a & 0xFFFF] = v;
    }
    #[inline]
    pub fn w(&self, a: usize) -> u16 {
        u16::from_le_bytes([self.m[a & 0xFFFF], self.m[(a + 1) & 0xFFFF]])
    }
    #[inline]
    pub fn sw(&mut self, a: usize, v: u16) {
        self.m[a & 0xFFFF] = v as u8;
        self.m[(a + 1) & 0xFFFF] = (v >> 8) as u8;
    }
    fn or_b(&mut self, a: usize, v: u8) {
        let x = self.b(a) | v;
        self.sb(a, x);
    }
    fn and_b(&mut self, a: usize, v: u8) {
        let x = self.b(a) & v;
        self.sb(a, x);
    }
    fn or_w(&mut self, a: usize, v: u16) {
        let x = self.w(a) | v;
        self.sw(a, x);
    }
    fn and_w(&mut self, a: usize, v: u16) {
        let x = self.w(a) & v;
        self.sw(a, x);
    }
    fn inc_b(&mut self, a: usize) {
        let x = self.b(a).wrapping_add(1);
        self.sb(a, x);
    }
    fn dec_b(&mut self, a: usize) {
        let x = self.b(a).wrapping_sub(1);
        self.sb(a, x);
    }

    // ------------------------------------------------------------------ RNGs (§3.0)
    /// sub_1E3CC (seed ds:D826).
    pub fn rnd(&mut self) -> u16 {
        let p = self.w(0xD826) as u32 * 0xCBD1;
        let s = (p as u16).wrapping_add(1);
        self.sw(0xD826, s);
        (s >> 8) | ((((p >> 16) & 0xFF) as u16) << 8)
    }
    /// sub_1E3B7 (seed ds:D824).
    pub fn rnd2(&mut self, mask: u16) -> u16 {
        let p = self.w(0xD824) as u32 * 0xE56D;
        let s = (p as u16).wrapping_add(1);
        self.sw(0xD824, s);
        ((s >> 8) | ((((p >> 16) & 0xFF) as u16) << 8)) & mask
    }
    /// sub_1E3DF: uniform 0..=max by rejection (seed ds:D828).
    pub fn rndr(&mut self, max: u16) -> u16 {
        if max == 0 {
            return 0;
        }
        let mut cx: u16 = 0xFFFF;
        let mut ax = max;
        loop {
            cx <<= 1;
            ax >>= 1;
            if ax == 0 {
                break;
            }
        }
        let cx = !cx;
        loop {
            let p = self.w(0xD828) as u32 * 0xCBD1;
            let s = (p as u16).wrapping_add(1);
            self.sw(0xD828, s);
            let v = ((s >> 8) | ((((p >> 16) & 0xFF) as u16) << 8)) & cx;
            if v <= max {
                return v;
            }
        }
    }
    /// `rol word ds:0,1`: the carry.
    pub fn rol0(&mut self) -> bool {
        let v = self.w(0);
        self.sw(0, v.rotate_left(1));
        v & 0x8000 != 0
    }

    pub fn clock(&self) -> u16 {
        self.w(2)
    }
    /// sub_11AC5: the raw day.
    pub fn day(&self) -> u16 {
        self.w(2) >> 4
    }

    // ------------------------------------------------------------------ troop lists (§1.1)
    /// sub_16906: CF = 1 ⇔ ours (+3 < 0x80).
    pub fn ours(&self, si: usize) -> bool {
        self.b(si + 3) < 0x80
    }
    /// sub_1693B: job class.
    pub fn class(&self, si: usize) -> u8 {
        (self.b(si + 3) & 0x0F) >> 2
    }

    /// sub_16603: every troop of a place (the next id is read after the callback).
    pub fn walk_all(&mut self, di: usize, mut f: impl FnMut(&mut Sim, usize)) {
        let mut al = self.b(di + 9);
        let mut guard = 0;
        while al != 0 && guard < 200 {
            let si = troop_addr(al);
            f(self, si);
            al = self.b(si + 1);
            guard += 1;
        }
    }
    /// sub_1661D: our troops of a place.
    pub fn walk_ours(&mut self, di: usize, mut f: impl FnMut(&mut Sim, usize)) {
        self.walk_all(di, |s, si| {
            if s.ours(si) {
                f(s, si)
            }
        });
    }
    /// sub_16639: the place's troops, then every moving troop heading there.
    pub fn walk_with_moving(&mut self, di: usize, mut f: impl FnMut(&mut Sim, usize)) {
        self.walk_all(di, &mut f);
        let mut si = TROOPS;
        while si < TROOP_END {
            let al = self.b(si + 3);
            if al & 0x40 != 0 {
                let dest = if al & 3 == 3 { self.w(si + 0x0C) } else { self.w(si + 4) };
                if dest as usize == di {
                    f(self, si);
                }
            }
            si += TROOP_SIZE;
        }
    }
    pub fn troop_ids(&self, di: usize) -> Vec<usize> {
        let mut v = vec![];
        let mut al = self.b(di + 9);
        while al != 0 && v.len() < 100 {
            let si = troop_addr(al);
            v.push(si);
            al = self.b(si + 1);
        }
        v
    }

    /// sub_1851F: append to the place list with the lowest free slot number.
    pub fn link(&mut self, si: usize, di: usize) {
        let ah = self.b(si);
        let al = self.b(di + 9);
        if al == 0 {
            self.sb(di + 9, ah);
            let mut cl = 1u8;
            if self.w(si + 0x10) & 0x80 != 0 {
                cl += 8;
            }
            self.sb(si + 2, cl);
            return;
        }
        let mut used = [0u8; 30];
        let mut al = al;
        let mut last = 0;
        let mut guard = 0;
        while al != 0 && guard < 100 {
            let t = troop_addr(al);
            let bl = self.b(t + 2) as usize;
            if (1..=30).contains(&bl) {
                used[bl - 1] = 0xFF;
            }
            last = t;
            al = self.b(t + 1);
            guard += 1;
        }
        self.sb(last + 1, ah);
        let start = if self.w(si + 0x10) & 0x80 != 0 { 8 } else { 0 };
        let mut cl = 0u8;
        for (j, &u) in used[start..].iter().enumerate() {
            if u == 0 {
                cl = (start + j + 1) as u8;
                break;
            }
        }
        if cl == 0 {
            cl = 30;
        }
        self.sb(si + 2, cl);
    }

    /// sub_1858C: unlink from its place list (a moving troop is in no list).
    pub fn unlink(&mut self, si: usize) {
        let al = self.b(si);
        let di = self.w(si + 4) as usize;
        if self.b(si + 3) & 0x40 == 0 {
            if self.b(di + 9) == al {
                let next = self.b(si + 1);
                self.sb(si + 1, 0);
                self.sb(di + 9, next);
                return;
            }
            let mut cur = self.b(di + 9);
            let mut guard = 0;
            while cur != 0 && guard < 100 {
                let t = troop_addr(cur);
                let nx = self.b(t + 1);
                if nx == 0 {
                    break;
                }
                if nx == al {
                    let after = self.b(si + 1);
                    self.sb(t + 1, after);
                    break;
                }
                cur = nx;
                guard += 1;
            }
        }
        self.sb(si + 1, 0);
    }

    /// sub_166B1: the troop is destroyed.
    pub fn destroy(&mut self, si: usize) {
        self.unlink(si);
        self.icons = true;
        self.sw(si + 4, NOWHERE as u16);
        self.sb(si + 3, 0xA0);
        self.sb(si + 0x1A, 0);
    }

    /// sub_185CC: a Fremen troop beyond slot 8 makes room by destroying a captured one.
    fn make_room(&mut self, si: usize) {
        if self.w(si + 0x10) & 0x80 != 0 || self.b(si + 2) <= 8 {
            return;
        }
        self.unlink(si);
        let di = self.w(si + 4) as usize;
        let mut al = self.b(di + 9);
        while al != 0 {
            let t = troop_addr(al);
            al = self.b(t + 1);
            if self.b(t + 0x10) & 0x80 != 0 || self.b(t + 3) & 0x20 == 0 {
                continue;
            }
            self.destroy(t);
            break;
        }
        self.link(si, di);
    }

    // ------------------------------------------------------------------ jobs (§2.5)
    /// sub_16B25.
    pub fn job_reset(&mut self, si: usize) {
        let t = self.clock();
        self.sw(si + 0x0A, t);
        self.sw(si + 0x0C, 0);
        self.sw(si + 0x0E, 0);
    }

    /// loc_16BB6: toggle the inactive bit to `cf`; returns cf.
    fn toggle_inactive(&mut self, si: usize, cf: bool) -> bool {
        let al = self.b(si + 3) & 0x10;
        let ah = if cf { 0x10 } else { 0 };
        if al != ah {
            self.sb(si + 3, self.b(si + 3) ^ 0x10);
            self.icons = true;
            if al != 0 {
                self.job_reset(si);
            }
        }
        cf
    }
    /// sub_16B96 (mining precondition): true = cannot work.
    pub fn pre_mine(&mut self, si: usize, di: usize) -> bool {
        let cf = self.w(si + 0x10) & 0x200 != 0
            || self.w(si + 0x12) & 0x30 != 0
            || self.b(di + 0x12) < 1
            || (self.b(di + 0x0A) ^ 0x40) & 0x41 != 0;
        self.toggle_inactive(si, cf)
    }
    /// sub_16B8A (prospecting).
    pub fn pre_prospect(&mut self, si: usize, di: usize) -> bool {
        let cf = self.b(di + 0x0A) & 0x41 != 0;
        self.toggle_inactive(si, cf)
    }
    /// sub_16BD7 (irrigation).
    pub fn pre_plant(&mut self, si: usize, di: usize) -> bool {
        let cf = self.w(si + 0x12) & 0x30 != 0 || self.b(di + 0x1B) < 1 || self.b(di + 0x0A) & 0x20 == 0 || self.b(si + 0x19) & 2 == 0;
        self.toggle_inactive(si, cf)
    }
    /// sub_16C15: precondition of the troop's job (CF).
    pub fn precondition(&mut self, si: usize) -> bool {
        let di = self.w(si + 4) as usize;
        match self.b(si + 3) & 0x0F {
            0 => self.pre_mine(si, di),
            1 => self.pre_prospect(si, di),
            8 => self.pre_plant(si, di),
            _ => false,
        }
    }

    /// sub_16AD4: set job cl.
    pub fn set_job(&mut self, si: usize, mut cl: u8) {
        let di = self.w(si + 4) as usize;
        if cl == 8 && di == 0x07C8 && self.b(di + 0x1A) == 0 {
            cl = 0x0A;
        }
        self.sb(si + 3, cl);
        self.and_b(si + 0x12, 0xCF);
        self.and_w(si + 0x10, 0xFEFF);
        if !self.precondition(si) {
            self.or_w(si + 0x10, 0x100);
        }
        self.icons = true;
        self.job_reset(si);
        if self.b(si + 3) != 2 {
            let c = self.class(si);
            let v = 0x2000u16 << c;
            self.or_w(si + 0x12, v);
        }
        // sub_17847 (menu enables) is refreshed by the orders UI.
    }
    /// loc_16ACB: set job cl unless it is already the job.
    pub fn set_job_if(&mut self, si: usize, cl: u8) {
        if self.b(si + 3) & 0x0F == cl {
            return;
        }
        self.set_job(si, cl);
    }
    /// sub_16AC5: back to the base job of the class.
    pub fn job_base(&mut self, si: usize) {
        let cl = self.b(si + 3) & 0xFC;
        self.set_job_if(si, cl);
    }
    /// sub_17085.
    pub fn inactive(&mut self, si: usize) {
        self.or_b(si + 3, 0x10);
    }

    // ------------------------------------------------------------------ motivation (§3.6)
    /// sub_16EFD.
    pub fn motivation(&self, si: usize) -> u8 {
        let ah = self.b(si + 3) & 0x0F;
        let mut al = self.b(si + 0x15);
        if self.b(0xFA) != 0 {
            al = al.wrapping_add(0x14);
        }
        let mut cap = true;
        if ah == 6 {
            if self.w(si + 4) == self.w(0x114E) {
                al = al.wrapping_add(0x1E);
            } else {
                cap = false;
            }
        } else if ah & 0xFE == 8 {
            al = 0x64;
            cap = false;
        }
        if cap && al >= 0x64 {
            al = 0x64;
        }
        let ph = self.b(0x2A);
        if (0x64..0x68).contains(&ph) {
            al = al.wrapping_sub(0x28);
            if (al as i8) < 10 {
                al = 10;
            }
        }
        al
    }
    /// sub_16F48.
    pub fn motiv_add(&mut self, si: usize, al: u8) {
        let v = self.b(si + 0x15).wrapping_add(al);
        self.sb(si + 0x15, if v > 0x64 { 0x64 } else { v });
    }
    /// sub_16F56: every active troop.
    pub fn motiv_add_all(&mut self, al: u8) {
        let mut si = TROOPS;
        while si < TROOP_END {
            if self.b(si + 3) & 0xA0 == 0 {
                self.motiv_add(si, al);
            }
            si += TROOP_SIZE;
        }
    }
    /// sub_16F93.
    pub fn motiv_sub(&mut self, si: usize, al: u8) {
        let (v, c) = self.b(si + 0x15).overflowing_sub(al);
        self.sb(si + 0x15, if c { 0 } else { v });
        if self.b(si + 0x15) < 5 {
            self.sb(si + 0x15, 4);
            self.inactive(si);
            self.or_w(si + 0x12, 0x20);
        }
    }
    /// sub_16F78: charisma +al.
    pub fn charisma_up(&mut self, al: u8) {
        let ah = self.b(0x29);
        let mut v = al.wrapping_add(ah);
        if v > 0xC8 {
            v = 0xC8;
        }
        self.sb(0x29, v);
        let d = ((v & 0xFC).wrapping_sub(ah & 0xFC)) >> 2;
        if d != 0 {
            self.motiv_add_all(d);
        }
    }
    /// sub_16FB0: charisma −al.
    pub fn charisma_down(&mut self, al: u8) {
        let ch = self.b(0x29);
        let (v, c) = ch.overflowing_sub(al);
        let v = if c || v == 0 { 1 } else { v };
        self.sb(0x29, v);
        let d = ((ch & 0xFC).wrapping_sub(v & 0xFC)) >> 2;
        if d == 0 {
            return;
        }
        let mut si = TROOPS;
        while si < TROOP_END {
            if self.b(si + 3) & 0xA0 == 0 {
                self.motiv_sub(si, d);
            }
            si += TROOP_SIZE;
        }
    }
    /// sub_16EDD: skill bx += al (cap 0x5F), level-up notice.
    pub fn skill_up(&mut self, si: usize, bx: usize, al: u8) {
        let dl = self.b(si + 0x16 + bx);
        let mut v = al.wrapping_add(dl);
        if v > 0x5F {
            v = 0x5F;
        }
        self.sb(si + 0x16 + bx, v);
        if (v ^ dl) & 0xF0 != 0 {
            let x = (self.w(si + 0x10) & 0xFFFC) | (bx as u16 + 1);
            self.sw(si + 0x10, x);
        }
    }

    // ------------------------------------------------------------------ rally / capture
    /// sub_166CE: the troop joins Paul.
    pub fn rally(&mut self, si: usize) {
        if self.b(si + 3) & 0x80 == 0 || self.b(si + 0x10) & 0x80 != 0 {
            return;
        }
        self.inc_b(0x28);
        if self.b(0x28) >= self.b(0x1178) {
            self.raise_phase(0x4C);
        }
        self.charisma_up(1);
        let v = (self.b(si + 3) & 0x20) | 2;
        self.sb(si + 3, v);
        self.job_reset(si);
        let d = self.day() as u8;
        self.sb(si + 0x14, d);
        let di = self.w(si + 4) as usize;
        if self.b(di + 0x0B) == 0 {
            self.sb(di + 0x0B, 2);
            self.paint_ours(di);
        }
        self.icons = true;
    }
    /// sub_1668F: captured (spy caught / defeated).
    pub fn caught(&mut self, si: usize) {
        if self.b(si + 3) & 0x20 != 0 || self.b(si + 0x10) & 0x80 != 0 {
            return;
        }
        self.or_b(si + 3, 0x20);
        self.sb(si + 0x19, 0);
        let t = self.clock();
        self.sw(si + 0x0A, t);
        self.charisma_down(4);
    }

    // ------------------------------------------------------------------ equipment (§4.10)
    /// sub_17F27: free stock of a place into ds:46FE..4704.
    pub fn free_stock(&mut self, di: usize) {
        self.free_stock_into(di, 0x46FE);
    }
    pub fn free_stock_into(&mut self, di: usize, bx: usize) {
        for k in 0..7 {
            let v = self.b(di + 0x14 + k);
            self.sb(bx + k, v);
        }
        let mut al = self.b(di + 9);
        let mut guard = 0;
        while al != 0 && guard < 100 {
            let si = troop_addr(al);
            let mut e = self.b(si + 0x19);
            let mut k = 0;
            while e != 0 && k < 8 {
                if e & 0x80 != 0 {
                    let v = self.b(bx + k);
                    self.sb(bx + k, v.saturating_sub(1));
                }
                e <<= 1;
                k += 1;
            }
            al = self.b(si + 1);
            guard += 1;
        }
    }
    /// sub_17F5F: arrival, the place stock gets the troop's items.
    pub fn stock_add(&mut self, si: usize, di: usize) {
        let mut e = self.b(si + 0x19);
        let mut k = 0;
        while e != 0 && k < 8 {
            if e & 0x80 != 0 {
                self.inc_b(di + 0x14 + k);
            }
            e <<= 1;
            k += 1;
        }
    }
    /// sub_17F75: departure (not below 0).
    pub fn stock_sub(&mut self, si: usize, di: usize) {
        let mut e = self.b(si + 0x19);
        let mut k = 0;
        while e != 0 && k < 8 {
            if e & 0x80 != 0 {
                let v = self.b(di + 0x14 + k);
                self.sb(di + 0x14 + k, v.saturating_sub(1));
            }
            e <<= 1;
            k += 1;
        }
    }

    /// sub_17F90: nearest place with a free item bl for troop si. ds:470C/470E.
    pub fn find_item(&mut self, si: usize, bl: u8) {
        let home = self.w(si + 4) as usize;
        let (hl, hr) = (self.w(home + 2), self.w(home + 4));
        self.sw(0x470C, 0xFFFF);
        let mut di = 0x138;
        while self.w(di) != 0xFFFF {
            if self.b(di + 0x0A) & 0x80 == 0 && self.b(di + 8) < 0x28 && di != home {
                let mut dx = self.w(di + 2).wrapping_sub(hl);
                if (dx as i16) < 0 {
                    dx = dx.wrapping_neg();
                }
                let mut ax = self.w(di + 4).wrapping_sub(hr);
                if (ax as i16) < 0 {
                    ax = ax.wrapping_neg();
                }
                let mut d = dx >> 8;
                if (d as u8) < (ax as u8) {
                    d = ax;
                }
                if (d as u8) < 0x32 {
                    if self.b(di + 8) >= 0x21 {
                        d >>= 2;
                    }
                    if d < self.w(0x470C) {
                        self.free_stock_into(di, 0x4C60);
                        if self.b(0x4C60 + bl as usize) != 0 {
                            self.reserve_items(di, bl);
                            if self.b(0x4C60 + bl as usize) != 0 {
                                self.sw(0x470C, d);
                                self.sw(0x470E, di as u16);
                            }
                        }
                    }
                }
            }
            di += 0x1C;
        }
    }
    /// sub_18018: subtract the items already being fetched there, and Paul's orni.
    fn reserve_items(&mut self, di: usize, bl: u8) {
        let mut si = 0x088F + TROOP_SIZE;
        while si < TROOP_END {
            let al = self.b(si + 3);
            if al & 0x40 != 0 && al & 3 == 3 && self.w(si + 4) as usize == di && self.w(si + 0x0C) as usize != di && self.b(si + 0x0E) == bl {
                let a = 0x4C60 + bl as usize;
                let v = self.b(a);
                self.sb(a, v.saturating_sub(1));
            }
            si += TROOP_SIZE;
        }
        if di == self.w(0x1150) as usize && self.b(0x2A) < 0x50 {
            let v = self.b(0x4C61);
            self.sb(0x4C61, v.saturating_sub(1));
        }
    }

    // ------------------------------------------------------------------ place analysis
    /// sub_15D36: CF = 1 ⇔ not a Harkonnen-held fort.
    pub fn friendly(&self, di: usize) -> bool {
        self.b(di + 8) < 0x28 || self.b(di + 0x0A) & 0x08 != 0
    }
    /// sub_15098: (cx, dx) = (Harkonnen troops alive, Fremen job-6 troops alive).
    pub fn sides(&mut self, di: usize) -> (u16, u16) {
        let (mut cx, mut dx) = (0u16, 0u16);
        self.walk_all(di, |s, si| {
            if s.b(si + 3) & 0x20 != 0 {
                return;
            }
            if s.b(si + 0x10) & 0x80 != 0 {
                cx += 1;
            } else if s.b(si + 3) == 6 {
                dx += 1;
            }
        });
        (cx, dx)
    }

    /// sub_1342D: troop strength.
    pub fn strength(&self, si: usize) -> u16 {
        let mut al = self.motivation(si).wrapping_add(self.motivation(si));
        let (v, c) = al.overflowing_add(self.b(si + 0x17));
        al = if c { 0xFF } else { v };
        let mut ax: u32 = (al as u32 * self.b(si + 0x1A) as u32) >> 4;
        let mut dx = ax;
        let mut bl = self.b(si + 0x19) << 2;
        for _ in 0..4 {
            dx = (dx << 1) & 0xFFFF;
            let bit = bl & 0x80 != 0;
            bl <<= 1;
            if bit {
                ax += dx;
                if ax > 0xFFFF {
                    ax = 0xFFFF;
                    break;
                }
            }
        }
        let r = (ax >> 8) as u16;
        if r == 0 {
            (self.b(si + 0x1A) >= 1) as u16
        } else {
            r
        }
    }
    /// sub_1348A: wind-trap progress per slot.
    pub fn windtrap_rate(&self, si: usize) -> u8 {
        let m = self.motivation(si);
        let al = m.wrapping_add(m);
        let (v, c) = al.overflowing_add(self.b(si + 0x16));
        let al = if c { 0xFF } else { v };
        let ax = (al as u32 * self.b(si + 0x1A) as u32) >> 4;
        let r = (ax >> 8) as u8;
        if r == 0 {
            (self.b(si + 0x1A) >= 1) as u8
        } else {
            r
        }
    }
    /// sub_133D9.
    pub fn threshold(h: u16, f: u16) -> u8 {
        let (hi, lo) = if f >= h { (f, h) } else { (h, f) };
        let mut v: u32 = 0xFC;
        if lo != 0 {
            let n = (hi as u32) << 8;
            if (n >> 16) < lo as u32 {
                v = (n / lo as u32) >> 1;
                if v >= 0xFC {
                    v = 0xFC;
                }
            }
        }
        let v = v as u8;
        if f < h {
            v.wrapping_neg()
        } else {
            v
        }
    }
    /// sub_133BE: side sums ds:94/96, ds:5C/5E, threshold ds:9C.
    pub fn side_sums(&mut self, di: usize) {
        self.sw(0x94, 0);
        self.sw(0x96, 0);
        self.sw(0x5C, 0);
        self.sw(0x5E, 0);
        self.walk_all(di, |s, si| {
            if s.b(si + 3) & 0x20 != 0 {
                return;
            }
            let ax = s.strength(si);
            if s.b(si + 0x10) & 0x80 != 0 {
                let v = s.w(0x94).wrapping_add(ax);
                s.sw(0x94, v);
            } else {
                let v = s.w(0x96).wrapping_add(ax);
                s.sw(0x96, v);
                let a = s.w(si + 0x10);
                s.or_w(0x5C, a);
                let a = s.w(si + 0x12);
                s.or_w(0x5E, a);
            }
        });
        let t = Self::threshold(self.w(0x94), self.w(0x96));
        self.sb(0x9C, t);
    }
    /// sub_134A5: troop counts ds:60..92 of a place.
    pub fn counts(&mut self, di: usize) {
        for a in 0x60..0x93 {
            self.sb(a, 0);
        }
        self.walk_with_moving(di, |s, si| {
            if s.b(si + 3) & 0x20 != 0 {
                return;
            }
            let al = s.b(si + 3);
            let dx = if al & 0x40 != 0 { 0x7F } else { 0x61 };
            let mut bx = dx;
            if s.b(si + 0x10) & 0x80 == 0 {
                bx -= 1;
                if al == 0x80 {
                    s.inc_b(0x90);
                    return;
                }
            }
            s.inc_b(bx);
            let mut job = al & 0x0F;
            if al & 3 == 3 {
                job &= 0xFC;
            }
            let bx = dx + job as usize;
            s.inc_b(bx + 1);
            if bx < 0x7F {
                let r = (s.w(si + 0x12) & 0x0F) as usize;
                s.inc_b(0x71 + r);
            }
        });
        let v = self.b(0x60).wrapping_add(self.b(0x7E));
        self.sb(0x91, v);
        let v = self.b(0x61).wrapping_add(self.b(0x7F));
        self.sb(0x92, v);
    }

    /// sub_1331E: the place variables used by the dialogue conditions.
    pub fn analyse(&mut self, di: usize) {
        self.sw(0x11CE, di as u16);
        let (b0, b1) = (self.b(di), self.b(di + 1));
        self.sw(0x4E, ((b0 as u16) << 8) | b1 as u16);
        let v = self.b(0x1141 + b1 as usize);
        self.sb(0x50, v);
        let v = self.b(di + 0x0A);
        self.sb(0x51, v);
        let v = self.b(di + 0x12);
        self.sb(0x52, v);
        let v = self.b(di + 0x1B);
        self.sb(0x54, v);
        let v = self.b(di + 8);
        self.sb(0x4D, v);
        for k in 0..7 {
            let v = self.b(di + 0x14 + k);
            self.sb(0x55 + k, v);
        }
        self.side_sums(di);
        self.counts(di);
        self.free_stock(di);
        let mut al: u8 = 0xFF;
        for k in 0..7 {
            let cf = self.b(0x46FE + k) < 1;
            al = (al << 1) | cf as u8;
        }
        al = !al;
        al <<= 1;
        self.sb(0x53, al);
        self.present_helpers(di);
        self.nearest(di);
    }
    /// sub_13385: ds:F7 = Gurney/Stilgar/Chani at that place.
    fn present_helpers(&mut self, di: usize) {
        self.sb(0xF7, 0);
        if di == self.w(0x1150) as usize {
            return;
        }
        let bx = ((((di - 0x100) / 0x1C + 1) as u16) << 8) | 0x80;
        for si in [0x1018, 0x1028, 0x1048] {
            if self.w(si + 2) == bx {
                let cl = self.b(si + 0x0E);
                let v = self.b(0xF7) | 1u8.wrapping_shl(cl as u32);
                self.sb(0xF7, v);
            }
        }
    }
    /// sub_15274: nearest places (ds:CA..E4).
    pub fn nearest(&mut self, di: usize) {
        let (dx, bx) = (self.w(di + 2), self.w(di + 4));
        for a in [0xCA, 0xD0, 0xD6, 0xDC, 0xE2] {
            self.sw(a, 0xFFFF);
        }
        let mut si = 0x100;
        while self.w(si) != 0xFFFF {
            if si != di {
                let mut cx = self.w(si + 2).wrapping_sub(dx);
                if (cx as i16) < 0 {
                    cx = cx.wrapping_neg();
                }
                let mut ax = self.w(si + 4).wrapping_sub(bx);
                if (ax as i16) < 0 {
                    ax = ax.wrapping_neg();
                }
                let mut d = cx >> 8;
                if (d as u8) < (ax as u8) {
                    d = ax;
                }
                let bp = if self.b(si + 8) >= 0x28 {
                    Some(if self.b(si + 0x0A) & 0x80 != 0 { 0xE2 } else { 0xDC })
                } else if self.b(si + 0x0A) & 0x80 == 0 {
                    Some(0xD0)
                } else if self.b(0x2A) < self.b(si + 0x0B) {
                    None
                } else {
                    Some(0xD6)
                };
                if let Some(bp) = bp {
                    if d < self.w(bp) {
                        self.sw(bp, d);
                        self.sw(bp + 2, si as u16);
                    }
                    if d < self.w(0xCA) {
                        self.sw(0xCA, d);
                        self.sw(0xCC, si as u16);
                    }
                }
            }
            si += 0x1C;
        }
        let o = (dx, bx);
        self.direction(0xDE, o);
        self.direction(0xE4, o);
        let al = self.direction(0xD8, o);
        self.sw(0x11FD, al as u16 + 0xDA);
        self.direction(0xCC, o);
        self.direction(0xD2, o);
    }
    /// sub_15323.
    fn direction(&mut self, bp: usize, (dx, bx): (u16, u16)) -> u8 {
        let di = self.w(bp) as usize;
        let (tl, tr) = (self.w(di + 2), self.w(di + 4));
        let al = Self::angle(dx, bx, tl, tr);
        let v = (al.wrapping_add(0x10) >> 5) & 7;
        self.sb(bp + 2, v);
        v
    }
    /// sub_15133: heading 0..255 from (dx, bx) to (di, cx).
    pub fn angle(dx: u16, bx: u16, di: u16, cx: u16) -> u8 {
        let mut bx = cx.wrapping_sub(bx) as i16 as i32;
        let mut dx = di.wrapping_sub(dx) as i16 as i32;
        if !(-0x80..0x80).contains(&bx) {
            bx >>= 1;
            dx >>= 1;
        }
        let bx = ((bx as i16 as i32 & 0xFF) << 8) as u16 as i16 as i32;
        let ax = bx.abs();
        let cx = (dx as i16 as i32).abs();
        let dx = dx as i16 as i32;
        if cx >= ax {
            if cx < 1 {
                return 0;
            }
            let q = (0x20 * bx) / dx;
            let al = q as u8;
            if dx >= 0 {
                al.wrapping_add(0x40)
            } else {
                al.wrapping_add(0xC0)
            }
        } else {
            if ax < 1 {
                return 0;
            }
            let q = (0x20 * dx) / bx;
            let mut al = q as u8;
            if bx >= 0 {
                al = al.wrapping_sub(0x80);
            }
            al.wrapping_neg()
        }
    }

    // ------------------------------------------------------------------ territory (§3.5)
    /// sub_1B58B: (map index, column, row width) of (lon, row).
    fn cell_col(&self, lon: u16, row: i16) -> (usize, i32, i32) {
        let (di, bp) = planet::cell_ptr(self.m, lon, row);
        let p = bp as u32 * lon as u32;
        let col = (p >> 16) + ((p >> 15) & 1);
        (di, col as i32, bp as i32)
    }
    /// seg000:64B2: filled midpoint circle, calls `span(row, start, count)` per row pair.
    fn circle(r: u16, mut span: impl FnMut(i32, i32, i32)) {
        let cx0 = r as i32;
        if cx0 == 0 {
            return;
        }
        // Literal register model of the routine.
        let mut ax: i32 = cx0;
        let mut bx: i32 = -cx0;
        let mut si: i32 = cx0 - 1;
        let mut dx: i32 = 0;
        let mut di: i32 = 0;
        let mut bp: i32 = 0;
        let mut cx: i32 = cx0;
        let mut emit = |bx: i32, si: i32, dx: i32, di: i32| {
            let n = dx - di + 1;
            span(bx, di, n);
            span(si, di, n);
        };
        // 64C6
        loop {
            ax -= bp;
            if ax >= 0 {
                // 64C3
                bp += 1;
                dx += 1;
                di -= 1;
                continue;
            }
            emit(bx, si, dx, di);
            bx += 1;
            si -= 1;
            ax += cx;
            if ax < 0 {
                break;
            }
            cx -= 1;
            if cx == 0 {
                break;
            }
            bp += 1;
            dx += 1;
            di -= 1;
        }
        // 64D5
        bp += 1;
        cx -= 1;
        let mut first = true;
        loop {
            if !first {
                // 64D9
                dx += 1;
                di -= 1;
                ax -= bp;
                bp += 1;
            }
            first = false;
            // 64DE
            loop {
                emit(bx, si, dx, di);
                bx += 1;
                si -= 1;
                let (s, c) = (ax as u16).overflowing_add(cx as u16);
                ax = s as i16 as i32;
                if c {
                    break;
                }
                cx -= 1;
                if cx == 0 {
                    break;
                }
            }
            // 64E9
            cx -= 1;
            if cx < 0 {
                return;
            }
        }
    }

    /// sub_1644E (ours, radius +B) / sub_16447 (Harkonnen, radius cx).
    pub fn paint_ours(&mut self, di: usize) {
        let r = self.b(di + 0x0B) as u16;
        self.paint_zone(di, r, 0x20);
    }
    pub fn paint_hark(&mut self, di: usize, r: u16) {
        self.paint_zone(di, r, 0x30);
    }
    fn paint_zone(&mut self, di: usize, r: u16, val: u8) {
        self.sb(0x4739, val);
        let (lon, row0) = (self.w(di + 2), self.w(di + 4) as i16 as i32);
        let mut spans = vec![];
        Self::circle(r, |bx, start, n| spans.push((bx, start, n)));
        for (bx, start, n) in spans {
            let row = bx + row0;
            if !(-0x5D..=0x5D).contains(&row) {
                continue;
            }
            let (base, col, bp) = self.cell_col(lon, row as i16);
            if bp <= 0 {
                continue;
            }
            let mut p = base as i32 + start;
            let mut c = col + start;
            while c < 0 {
                p += bp;
                c += bp;
            }
            for _ in 0..n.max(0) {
                if let Some(cell) = self.map.get_mut(p as usize) {
                    let ah = *cell & 0x30;
                    if ah != 0x10 {
                        *cell = (*cell ^ ah) | val;
                    }
                }
                c += 1;
                p += 1;
                if c >= bp {
                    c -= bp;
                    p -= bp;
                }
            }
        }
    }

    /// sub_16515: greening disc of a planting place.
    pub fn green_disc(&mut self, di: usize) {
        self.icons = true;
        self.sb(0x4736, 0x44);
        let r = self.b(di + 0x0B) as u16;
        let lon = self.w(di + 0x0C);
        let row0 = self.b(di + 0x0E) as i8 as i32;
        let mut spans = vec![];
        Self::circle(r, |bx, start, n| spans.push((bx, start, n)));
        for (bx, start, n) in spans {
            let row = bx + row0;
            if !(-0x56..=0x56).contains(&row) {
                continue;
            }
            let (base, col, bp) = self.cell_col(lon, row as i16);
            if bp <= 0 {
                continue;
            }
            let mut p = base as i32 + start;
            let mut c = col + start;
            while c < 0 {
                p += bp;
                c += bp;
            }
            for _ in 0..n.max(0) {
                let pi = p as usize;
                if pi < self.map.len() {
                    let orig = self.map[pi];
                    let mut al = orig;
                    if orig & 0x30 != 0x10 {
                        if orig & 0x40 != 0 {
                            if let Some(si) = self.place_at(pi) {
                                self.sb(si + 0x12, 0);
                                if !self.friendly(si) && si >= 0x138 {
                                    self.and_b(si + 0x0A, 0x7F);
                                    self.liberate(si);
                                }
                            }
                        }
                        let ah = orig & 0x0E;
                        al = (orig & 0xCF) | 0x20;
                        if ah < 8 {
                            let v = self.b(0x4736);
                            self.sb(0x4736, v.rotate_left(1));
                            if v & 0x80 != 0 {
                                al = (al & 0xCF) | 0x10;
                            }
                        }
                    }
                    self.map[pi] = al;
                }
                c += 1;
                p += 1;
                if c >= bp {
                    c -= bp;
                    p -= bp;
                }
            }
        }
    }
    /// sub_1409A.
    fn place_at(&self, di: usize) -> Option<usize> {
        let mut si = 0x100;
        while self.w(si) != 0xFFFF {
            if self.w(si + 6) as usize == di {
                return Some(si);
            }
            si += 0x1C;
        }
        None
    }

    /// sub_163F0: wind-trap water and vegetation decay (new day only).
    pub fn ecology(&mut self) {
        if self.b(0x46DE) == 0 {
            return;
        }
        let mut di = 0x100;
        while self.w(di) != 0xFFFF {
            if self.b(di + 0x0A) & 0x20 != 0 {
                let bl = self.b(di + 0x1B);
                if bl < 0xFA {
                    let si = self.w(di + 6) as usize;
                    let mut dx = 0u8;
                    for k in 0..6 {
                        let c = self.map.get((si + k).wrapping_sub(1)).copied().unwrap_or(0);
                        if c & 0x30 == 0x10 {
                            dx += 1;
                        }
                    }
                    let v = bl.wrapping_add((dx >> 1) + 1);
                    self.sb(di + 0x1B, if v < 0xFA { v } else { 0xFA });
                }
            }
            di += 0x1C;
        }
        // seg000:65B6
        let mut si = *self.lfsr;
        let mut n = 0;
        for _ in 0..0x46 {
            let c = si & 1;
            si >>= 1;
            if c != 0 {
                si ^= 0x402;
            }
            let mut d = si as u32;
            while d < MAP_CELLS as u32 {
                let p = d as usize;
                if self.map[p] & 0x30 == 0x10 {
                    self.map[p] = (self.map[p] & 0xCF) | 0x20;
                    n += 1;
                }
                d += 0x7FF;
            }
        }
        *self.lfsr = si;
        if n != 0 && (self.b(0x46EB) as i8) < 0 {
            self.inc_b(0x46EC);
        }
    }

    // ------------------------------------------------------------------ events (§3.10)
    /// sub_129F0: post report event ax for place di.
    pub fn post_event(&mut self, ax: u16, di: u16) {
        if self.b(0x0A) & 1 == 0 {
            return;
        }
        let n = self.b(0x1190) as usize;
        for k in 0..n {
            let a = 0x1191 + 4 * k;
            if self.w(a) == ax && self.w(a + 2) == di {
                return;
            }
        }
        let mut cx = self.b(0x1190) as usize;
        if cx >= 10 {
            self.pop_event();
            cx = 9;
        }
        self.inc_b(0x1190);
        let a = 0x1191 + 4 * cx;
        self.sw(a, ax);
        self.sw(a + 2, di);
    }
    /// sub_171B2: an event told by the place's Fremen chief.
    pub fn troop_event(&mut self, al: u8, di: usize) {
        self.post_event(0x0F00 | al as u16, di as u16);
    }
    /// sub_12A34: drop the oldest event.
    pub fn pop_event(&mut self) {
        self.dec_b(0x1190);
        if self.b(0x1190) == 0 {
            self.sb(0x118F, 0);
        }
        for k in 0..0x24 {
            let v = self.b(0x1195 + k);
            self.sb(0x1191 + k, v);
        }
        for k in 0..4 {
            self.sb(0x1191 + 0x24 + k, 0);
        }
    }
    /// loc_126DA: comm-room message ax = sender | param << 8.
    pub fn post_message(&mut self, ax: u16) {
        let n = self.b(0xC8) as usize;
        for k in 0..n {
            if self.w(0x1179 + 2 * k) == ax {
                return;
            }
        }
        let mut cl = self.b(0xC8) as usize;
        if cl >= 10 {
            self.drop_message();
            cl = 9;
        }
        self.sw(0x1179 + 2 * cl, ax);
        self.inc_b(0xC8);
        self.inc_b(0xC9);
        if self.b(0x2A) >= 0x38 && self.b(0x0B) != 8 {
            self.post_event(0x201, 0);
        }
    }
    /// sub_1272F.
    fn drop_message(&mut self) {
        if self.b(0xC8) == 0 {
            return;
        }
        self.dec_b(0xC8);
        for k in 0..18 {
            let v = self.b(0x117B + k);
            self.sb(0x1179 + k, v);
        }
        self.sw(0x1179 + 18, 0);
    }

    // ------------------------------------------------------------------ story phases (§3.9)
    /// sub_1425B: discover a place.
    pub fn discover(&mut self, di: usize) {
        if self.b(di + 0x0A) & 0x80 == 0 {
            return;
        }
        self.and_b(di + 0x0A, 0x7F);
        self.sb(di + 0x0B, 0);
        if self.b(di + 8) < 0x20 {
            self.inc_b(0x27);
            if self.w(di) == 0x0603 {
                self.raise_phase(0x10);
            }
        }
    }
    fn discover_list(&mut self, l: &[usize]) {
        for &d in l {
            self.discover(d);
        }
    }
    /// sub_1121F: raise the story phase to `al` and run its handler.
    pub fn raise_phase(&mut self, al: u8) {
        if al <= self.b(0x2A) {
            return;
        }
        self.sb(0x2A, al);
        self.sb(0xFF, 0);
        if al > 0x6C {
            return;
        }
        self.phase_handler();
    }
    /// The handler table cs:11E7 (word index = al >> 2 for multiples of 4; odd phases read
    /// the word at byte offset al >> 1, kept as in the original).
    fn phase_handler(&mut self) {
        let al = self.b(0x2A);
        let off = (al >> 1) as usize;
        // Word at cs:11E7 + off. Even offsets are table entries; odd ones straddle two.
        const TAB: [u16; 28] = [
            0x0D2A, 0x1011, 0x1027, 0x102F, 0x1045, 0x1053, 0x10B7, 0x10A4, 0x10B2, 0x10B7, 0x10B8, 0x10BE, 0x1103, 0x110F, 0x1127, 0x10B7,
            0x112D, 0x1133, 0x1139, 0x1166, 0x117B, 0x1188, 0x1196, 0x11B3, 0x11CB, 0x11E6, 0x10B7, 0x10B7,
        ];
        let h = if off & 1 == 0 {
            TAB[off / 2]
        } else {
            // Misaligned word: high byte of entry k, low byte of entry k+1.
            let k = off / 2;
            ((TAB[k] >> 8) | ((TAB.get(k + 1).copied().unwrap_or(0) & 0xFF) << 8)) as u16
        };
        match h {
            0x1011 => {
                // Phase 4: palace room table change, two places known, Jessica to room 9.
                self.dec_b(0x122A);
                self.discover_list(&[0x0218, 0x02DC]);
                self.sb(0x0FE8, 9);
            }
            0x1027 => self.and_b(0x122E, 0x7F),
            0x102F => {
                self.and_b(0x124A, 0x7F);
                self.and_b(0x1247, 0x7F);
                self.sw(0x121D, 0xFFFF);
            }
            0x1045 => {
                self.sb(0x0FD8, 5);
                self.sb(0x0FE8, 9);
                self.post_message(0x10B);
            }
            0x1053 => {
                self.sb(0x0FD8, 0x0A);
                self.discover_list(&[0x034C, 0x0368, 0x0384]);
            }
            0x10A4 => {
                self.and_b(0x1245, 0x7F);
                self.sw(0x1217, 0xFFFF);
            }
            0x10B2 => self.sb(0x0FFB, 1),
            0x10B8 => self.discover(0x0800),
            0x10BE => {
                let t = self.clock();
                self.sw(0x1154, t);
                self.sw(0x1018, 0x2006);
                self.sw(0x0FF8, 0x2008);
                self.sw(0x0FFA, 0x0180);
                self.sb(0x0FE8, 0x0A);
                self.sw(0x0FEA, 0x0180);
                self.sw(0x1201, 0x109);
                self.charisma_up(0x14);
                self.or_b(0x0A, 0x10);
                self.discover_list(&[0x05EC, 0x05D0, 0x0608, 0x0640, 0x065C]);
            }
            0x1103 => {
                self.post_event(4, 0);
                self.post_message(0x1409);
            }
            0x110F => {
                if self.b(0x0FE8) != 8 {
                    self.sb(0x0FE8, 0x0A);
                    self.sw(0x0FEA, 0x0180);
                }
                self.post_message(0x280A);
            }
            0x1127 => self.sb(0x0FDB, 0xFF),
            0x112D => self.or_b(0x1067, 2),
            0x1133 => self.discover(0x03D8),
            0x1139 => {
                self.charisma_up(0x0A);
                let al = (self.b(0x1057) | 0x10) & 0xFD;
                self.sb(0x1057, al);
                let v = self.b(0x28).wrapping_add(2);
                self.sb(0x1178, v);
                self.discover_list(&[0x03F4, 0x0410, 0x03BC, 0x088C]);
            }
            0x1166 => {
                self.inc_b(0x1141);
                self.sb(0x0FE8, 2);
                self.sw(0x0FEA, 0x0180);
                self.post_event(0x105, 0);
            }
            0x117B => {
                self.or_b(0x0A, 0x40);
                self.charisma_up(0x28);
                self.sb(0x0FE8, 9);
            }
            0x1188 => {
                self.and_b(0x1259, 0x7F);
                self.sw(0x1211, 0xFFFF);
            }
            0x1196 => {
                self.or_b(0x0A, 0x20);
                self.discover_list(&[0x07E4, 0x0790, 0x07AC, 0x0854, 0x081C]);
            }
            0x11B3 => {
                self.sb(0x1038, 5);
                let v = self.b(0x11D0).wrapping_add(0x0C);
                self.sb(0x11D0, v);
                let d = self.day().wrapping_add(3);
                self.sw(0x1156, d);
                self.inc_b(0x1141);
            }
            0x11CB => self.chani_kidnapped(),
            0x11E6 => self.chani_to_fort(),
            _ => {}
        }
    }
    /// sub_111CB: phase 0x60.
    pub fn chani_kidnapped(&mut self) {
        self.sb(0xFF, 0);
        self.sb(0x2A, 0x60);
        let (bx, dx) = Self::where_of(HARK_PALACE, self.b(HARK_PALACE + 8));
        self.sw(0x1048, (dx & 0xFF00) | 2);
        self.sw(0x104A, bx);
    }
    /// sub_140AE: (where word, place word with room 1).
    pub fn where_of(di: usize, app: u8) -> (u16, u16) {
        let idx = ((di - 0x100) / 0x1C + 1) as u16;
        ((idx << 8) | 0x80, ((app as u16) << 8) | 1)
    }
    /// sub_11F13: Chani moved to the southernmost Harkonnen fort without attackers.
    fn chani_to_fort(&mut self) {
        let mut di = 0x138;
        let mut bx: i16 = -100;
        let mut si = 0x100;
        while self.b(di) < 8 {
            let row = self.w(di + 4) as i16;
            if row > bx && !self.friendly(di) {
                let (_, dx) = self.sides(di);
                if dx == 0 {
                    let row = self.w(di + 4) as i16;
                    if row > bx {
                        bx = row;
                        si = di;
                    }
                }
            }
            di += 0x1C;
        }
        if bx == -100 {
            si = 0x100;
        }
        let (wb, dx) = Self::where_of(si, self.b(si + 8));
        self.sw(0x1048, (dx & 0xFF00) | 3);
        self.sw(0x104A, wb);
        let ax = ((self.b(si) as u16) << 8) | self.b(si + 1) as u16;
        self.sw(0xF2, ax);
        self.post_message(0x2B0A);
    }

    // ------------------------------------------------------------------ per slot (§3.1)
    /// sub_11B23 head: F4/F5 eye-exposure decay (before the sky and the clock display).
    pub fn eye_decay(&mut self) {
        let al = self.b(0xF4).wrapping_sub(1);
        if (al as i8) < 0x10 {
            self.sb(0xF5, 0);
            self.sb(0xF4, 0);
        } else {
            self.sb(0xF4, al);
        }
    }

    /// sub_11B23 from seg000:1B46 (the caller has run `eye_decay`, the sky and the clock).
    pub fn slot(&mut self) {
        let t = self.clock();
        let old = self.w(0x1174);
        self.sw(0x1174, t);
        let d = ((old & 0xF0) as u8).wrapping_sub((t & 0xF0) as u8);
        self.sb(0x46DE, d);
        if d != 0 {
            self.daily();
        }
        if self.b(0xC2) >= 7 {
            self.sb(0x473B, 0);
            return;
        }
        self.chani_cure_check();
        let save_11f7 = self.w(0x11F7);
        let save_11ce = self.w(0x11CE);
        self.troops_pass();
        self.ecology();
        match t & 15 {
            3 => self.emperor_check(),
            4 => {
                self.harkonnen_attack();
            }
            8 => {
                if self.b(0xBF) & 0x10 != 0 && self.w(0x10) & 8 == 0 && self.b(0x0B) != 8 && self.b(0xC2) == 0 {
                    self.post_event(0x30B, 0);
                }
            }
            15 => self.reinforce(),
            _ => {}
        }
        if save_11ce >= 0x100 {
            self.analyse(save_11ce as usize);
        }
        self.sw(0x11F7, save_11f7);
        self.war_recheck();
    }

    /// sub_11BEC: the per-slot war-mode re-check.
    pub fn war_recheck(&mut self) {
        if self.b(0x2B) == 0 {
            return;
        }
        let di = self.w(0x114E) as usize;
        if di >= 0x100 {
            self.hostility(di);
        }
        if self.b(0x46D9) != 0 {
            self.sb(0x46D9, 6);
        }
        if self.b(0x2B) == 0 {
            self.or_b(0x473B, 1);
        }
    }

    /// sub_11C46: first slot of a day.
    pub fn daily(&mut self) {
        let al = self.b(0x2A);
        let old = self.b(0xFE);
        self.sb(0xFE, al);
        if old != al {
            self.sb(0xFF, 0);
        }
        self.inc_b(0xFF);
        self.chars_daily();
        self.disease();
        let v = self.b(0xD5).wrapping_add(1);
        if v >= 2 {
            self.sb(0xD5, v);
        }
        let shipped = self.w(0x1172);
        self.sw(0x1172, 0);
        let a0 = self.w(0xA0);
        let prev = self.w(0x1170);
        self.sw(0x1170, a0);
        let (v, c) = shipped.wrapping_add(a0).overflowing_sub(prev);
        let a6 = if c { 0 } else { v };
        self.sw(0xA6, a6);
        let ae = self.w(0xAE);
        self.sw(0xAE, a6);
        if ae >= a6 {
            self.sw(0xB2, ae - a6);
            self.sw(0xB0, 0);
        } else {
            self.sw(0xB2, 0);
            self.sw(0xB0, a6 - ae);
        }
        self.hark_estimate();
        self.stats();
        self.trends();
        let mut bx = self.rnd();
        let mut si = 0x10D8;
        while self.b(si) < 0x14 {
            if self.b(si + 2) & 8 != 0 {
                for bp in (0..=4).rev() {
                    if self.b(si + 4 + bp) == 0 && self.b(si + 9 + bp) & 0x80 != 0 {
                        bx = bx.rotate_left(2);
                        self.sb(si + 4 + bp, (bx & 3) as u8);
                    }
                }
            }
            si += 0x11;
        }
    }
    /// sub_11D66: characters indoors somewhere get their room fixed.
    fn chars_daily(&mut self) {
        // cs:1D35: highest valid room per appearance byte.
        const MIN_ROOM: [u8; 0x31] = [
            2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 2, 2, 2, 2, 2, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 0x0C, 1, 1, 1, 1, 1, 1, 1, 3, 3, 3,
            3, 3, 3, 3, 3, 2,
        ];
        let mut si = 0x0FD8;
        for _ in 0..12 {
            let ax = self.w(si + 2);
            if ax as u8 == 0x80 && (ax >> 8) != 0xFF {
                let di = 0xE4 + 0x1C * (ax >> 8) as usize;
                let place = self.w(si);
                let bl = self.b(di + 8);
                let ok = (place >> 8) as u8 == bl && (place as u8) <= MIN_ROOM.get(bl as usize).copied().unwrap_or(0);
                if !ok {
                    self.sw(si, ((bl as u16) << 8) | 1);
                }
            }
            si += 0x10;
        }
    }
    /// sub_11CDA: Harkonnen spice estimate; returns the number of Harkonnen forts.
    pub fn hark_estimate(&mut self) -> u16 {
        let (mut cx, mut dx) = (0u16, 0u16);
        let mut di = 0x100;
        loop {
            if !self.friendly(di) {
                dx += 1;
                cx = cx.wrapping_add((self.b(di + 0x12) >> 3) as u16);
            }
            di += 0x1C;
            if self.b(di) == 0xFF {
                break;
            }
        }
        let r = self.rndr(cx >> 4);
        self.sw(0xA8, cx.wrapping_add(r));
        dx
    }
    /// sub_1C02E: SEE RESULTS statistics.
    pub fn stats(&mut self) {
        let (mut bx, mut dx) = (0u32, 0u32);
        for &c in self.map.iter().take(MAP_CELLS) {
            let a = c & 0x30;
            if a != 0 {
                dx += 1;
                if a != 0x30 {
                    bx += 1;
                }
            }
        }
        let h = dx - bx;
        let pct = |n: u32| {
            let q = (n << 16) / 0xC472;
            let p = q * 100;
            ((p >> 16) + ((p >> 15) & 1)) as u16
        };
        let a4 = pct(h);
        let a2 = pct(bx).wrapping_add(1);
        self.sw(0xA2, a2);
        self.sw(0xA4, a4);
        let (v, c) = self.w(0xA0).wrapping_add(self.w(0x1172)).overflowing_sub(self.w(0x1170));
        let ax = if c { 0 } else { v };
        if ax >= self.w(0xA6) {
            self.sw(0xA6, ax);
        }
        let (mut aa, mut ac) = (0u16, 0u16);
        let mut si = TROOPS;
        while self.b(si) != 0 {
            let men = self.b(si + 0x1A) as u16;
            if self.b(si + 3) & 0x20 == 0 {
                if self.b(si + 0x10) & 0x80 != 0 {
                    ac = ac.wrapping_add(men);
                } else if self.b(si + 3) & 0x80 == 0 {
                    aa = aa.wrapping_add(men);
                }
            }
            si += TROOP_SIZE;
        }
        self.sw(0xAA, aa);
        self.sw(0xAC, ac);
    }

    /// sub_1BF26 (data part; the strings are formatted by the globe screen): trends of
    /// the six SEE RESULTS values (ds:115E values, ds:116A trend codes).
    pub fn trends(&mut self) {
        let vals = [self.w(0xA4), self.w(0xA2), self.w(0xA8), self.w(0xA6), self.w(0xAC), self.w(0xAA)];
        let today = self.clock() & 0xFFF0;
        for (k, &v) in vals.iter().enumerate() {
            let old = self.w(0x115E + 2 * k);
            self.sw(0x115E + 2 * k, v);
            if v != old {
                self.sb(0x116A + k, if v < old { 2 } else { 1 });
            } else if self.w(0x115C) != today {
                self.sb(0x116A + k, 3);
            }
        }
    }

    /// sub_10169 (callback seg000:01E0): troops placed at their places, region and clan.
    pub fn init_troops(&mut self) {
        let mut di = 0x100;
        while self.b(di) != 0xFF {
            let (dx, bx) = (self.w(di + 2), self.w(di + 4));
            let place = di;
            self.walk_all(place, |s, si| {
                s.sw(si + 4, place as u16);
                s.sw(si + 6, dx);
                s.sw(si + 8, bx);
                let al = s.b(place) & 0x0F;
                let mut ah = s.b(si + 0x12) & 0x70;
                if al > 3 {
                    ah ^= 0x80;
                    if al > 5 {
                        ah ^= 0x80;
                        if al > 9 {
                            ah ^= 0x80;
                        }
                    }
                }
                s.sb(si + 0x12, al | ah);
            });
            di += 0x1C;
        }
    }

    /// sub_11D10: Harkonnen reinforcements (slot 15).
    pub fn reinforce(&mut self) {
        if !self.rol0() {
            return;
        }
        let mut si = TROOPS;
        while si < 0x0FA0 {
            if self.b(si + 0x10) & 0x80 != 0 {
                let al = self.b(si + 0x1A).wrapping_sub(1);
                if al < 0xC7 {
                    self.inc_b(si + 0x1A);
                }
            }
            si += TROOP_SIZE;
        }
    }

    // ------------------------------------------------------------------ disease / Chani (§3.9)
    /// sub_11E43 (daily).
    fn disease(&mut self) {
        if self.day() < self.w(0x1156) || self.b(0x2A) != 0x5C || self.w(0x114E) == 0x07C8 {
            return;
        }
        let mut di = 0x100;
        let (mut cx, mut bx) = (0u16, 0usize);
        while self.b(di) != 0xFF {
            if self.b(di + 8) < 0x28 && self.b(di + 0x0A) & 0x80 == 0 && di != 0x2C0 {
                let mut dx = 0u16;
                self.walk_ours(di, |s, si| {
                    if s.b(si + 3) < 8 {
                        dx += 1;
                    }
                });
                if dx > cx {
                    cx = dx;
                    bx = di;
                }
            }
            di += 0x1C;
        }
        if cx == 0 {
            return;
        }
        self.sw(0x11DB, bx as u16);
        self.inc_b(0xF8);
        self.walk_ours(bx, |s, si| {
            s.or_w(si + 0x12, 0x400);
            s.inactive(si);
        });
        self.troop_event(8, bx);
    }
    /// sub_11D9F.
    fn chani_cure_check(&mut self) {
        if self.w(0x12) & 0x80 != 0 {
            return;
        }
        if self.chani_at_ill() {
            self.chani_cure();
        }
    }
    /// sub_11E01 (si = Chani's record ds:1048).
    fn chani_at_ill(&mut self) -> bool {
        let si = 0x1048;
        if self.b(0x2A) != 0x5D || self.b(si + 0x0E) != 7 {
            return false;
        }
        let bx = self.w(si + 2);
        if bx as u8 != 0x80 {
            return false;
        }
        self.sb(si, 2);
        let di = 0xE4 + 0x1C * (bx >> 8) as usize;
        self.ill_at(di)
    }
    /// sub_11E24: a held (ill) troop at di.
    fn ill_at(&self, di: usize) -> bool {
        let mut al = self.b(di + 9);
        while al != 0 {
            let s = troop_addr(al);
            if self.w(s + 0x12) & 0x400 != 0 {
                return true;
            }
            al = self.b(s + 1);
        }
        false
    }
    /// sub_11EDA.
    fn chani_cure(&mut self) {
        let v = self.b(0xF9).wrapping_add(8);
        self.sb(0xF9, v);
        if v != 0 {
            return;
        }
        let di = (0xE4 + 0x1C * (self.w(0x104A) >> 8) as usize).max(0x100);
        self.walk_all(di, |s, si| {
            let ax = (s.w(si + 0x12) & 0xFBFF) | 0x800;
            s.sw(si + 0x12, ax);
        });
        self.post_event(0x709, di as u16);
        self.dec_b(0xF8);
        let mut d = 0x100;
        let mut found = 0;
        while self.w(d) != 0xFFFF {
            if self.ill_at(d) {
                found = d;
                break;
            }
            d += 0x1C;
        }
        self.sw(0x11DB, found as u16);
        if found == 0 {
            self.chani_kidnapped();
        }
    }
    /// sub_11EBE: end of contact with a troop cured by Chani.
    pub fn contact_end_story(&mut self, si: usize) {
        if self.w(si + 0x12) & 0x800 != 0 && self.b(0x2A).wrapping_sub(0x60) < 4 {
            self.raise_phase(0x64);
        }
    }

    // ------------------------------------------------------------------ troop pass (§3.2)
    /// sub_16C6F.
    pub fn troops_pass(&mut self) {
        let ax = self.w(0x101A);
        let g = if ax as u8 == 0x80 { 0x100 + 0x1C * ((ax >> 8) as usize).wrapping_sub(1) } else { 0 };
        self.sw(0x473C, g as u16);
        // sub_16C46
        let ph = self.b(0x2A).wrapping_sub(0x2D);
        if ph < 3 && self.w(0x10) & 0x10 == 0 && g != 0 {
            self.analyse(g);
            if self.b(0x66) != 0 {
                self.raise_phase(0x30);
            }
        }
        self.sb(0x4737, 0);
        let mut si = TROOPS;
        while si < TROOP_END {
            'again: loop {
                if self.w(si + 0x12) & 0x430 != 0 {
                    if self.b(si + 3) & 0x40 != 0 {
                        self.move_slot(si);
                        break 'again;
                    }
                    if self.b(0xFA) == 0 {
                        break 'again;
                    }
                    self.and_b(si + 0x12, 0xCF);
                    if self.w(si + 0x12) & 0x400 != 0 {
                        break 'again;
                    }
                    continue 'again;
                }
                if self.b(si + 0x1A) < 0x14 && self.merge(si) {
                    break 'again;
                }
                let al = self.b(si + 3);
                if al & 0xA0 != 0 {
                    break 'again;
                }
                if al & 0x40 != 0 {
                    self.move_slot(si);
                    break 'again;
                }
                let di = self.w(si + 4) as usize;
                match al & 0x0F {
                    0 => self.mine(si, di),
                    1 => self.prospect(si, di),
                    4 => self.train(si, di),
                    5 => self.spy(si, di),
                    6 => self.battle(si, di),
                    8 => self.plant(si, di),
                    9 => self.windtrap(si, di),
                    10 => self.bulbs(si, di),
                    _ => {}
                }
                self.decay(si);
                break 'again;
            }
            si += TROOP_SIZE;
        }
        let v = self.b(0x4737);
        self.sb(0xFA, v);
    }
    fn move_slot(&mut self, si: usize) {
        if self.b(si) != self.b(0x1954) {
            self.march(si, 4);
        }
    }

    /// sub_16D19: a small troop merges into another troop of the place.
    fn merge(&mut self, si: usize) -> bool {
        if self.b(si + 3) & 0xE3 != 0 || self.b(si + 0x10) & 0x80 != 0 || si == PROSPECTORS {
            return false;
        }
        let di = self.w(si + 4) as usize;
        let mut bx = 0usize;
        let mut cl = !self.b(si + 0x1A);
        self.walk_ours(di, |s, t| {
            if s.b(t + 3) & 0xA0 != 0 || t == PROSPECTORS || t == si {
                return;
            }
            let al = s.b(t + 0x1A);
            if cl >= al {
                bx = t;
                cl = al;
            }
        });
        if bx == 0 {
            return false;
        }
        let v = self.b(bx + 0x1A).wrapping_add(self.b(si + 0x1A));
        self.sb(bx + 0x1A, v);
        let al = self.b(si + 0x19);
        self.sb(si + 0x19, al & self.b(bx + 0x19));
        self.or_b(bx + 0x19, al);
        self.or_w(bx + 0x12, 0x200);
        self.destroy(si);
        true
    }
    /// sub_16D7B: skill decay every 64 slots.
    fn decay(&mut self, si: usize) {
        if self.clock() & 0x3F != 0 {
            return;
        }
        let cl = self.class(si) as u32;
        let ax = 0xC000u16.rotate_left(cl) & self.w(si + 0x12);
        if ax == 0 {
            return;
        }
        if ax & 0x8000 != 0 && self.b(si + 0x18) != 0 {
            self.dec_b(si + 0x18);
        }
        if ax & 0x4000 != 0 && self.b(si + 0x17) != 0 {
            self.dec_b(si + 0x17);
        }
        if ax & 0x2000 != 0 && self.b(si + 0x16) != 0 {
            self.dec_b(si + 0x16);
        }
    }

    /// sub_16CFC: daily growth of a sietch zone.
    fn sietch_grow(&mut self, di: usize) {
        if self.b(di + 8) >= 0x20 || self.b(di + 0x0B) >= 0x0C || self.b(di + 0x0A) & 1 != 0 {
            return;
        }
        self.inc_b(di + 0x0B);
        self.paint_ours(di);
    }
    /// sub_16E20: daily place events (mining, training, planting troops).
    fn place_daily(&mut self, si: usize, di: usize) {
        if self.b(0x46DE) == 0 {
            return;
        }
        if self.b(di + 0x0A) & 8 != 0 {
            let al = (self.day() as u8).wrapping_sub(self.b(di + 0x0B));
            if al >= 0xFE {
                return;
            }
            {
                self.and_b(di + 0x0A, 0xF7);
                self.and_b(di + 8, 7);
                self.inc_b(0x27);
                self.fort_converted(di);
                self.sb(di + 0x0B, 5);
            }
        }
        self.sietch_grow(di);
        let al = (self.day() as u8).wrapping_sub(self.b(si + 0x14));
        if al > 8 {
            self.motiv_sub(si, 1);
        }
        if self.b(si) != self.b(di + 9) {
            return;
        }
        let mut dl = 0u8;
        let paul = self.w(0x114E) as usize;
        self.walk_ours(di, |s, t| {
            if di == paul || s.b(di + 8) >= 0x21 || s.b(t + 0x15) >= 0x28 || s.b(t + 3) & 0x2F != 0 {
                return;
            }
            dl |= if s.b(t + 0x12) & 0x80 != 0 { 2 } else { 1 };
        });
        if dl != 3 {
            return;
        }
        self.walk_ours(di, |s, t| {
            if s.b(t + 0x15) >= 0x28 || s.b(t + 3) & 0x2B != 0 {
                return;
            }
            s.inactive(t);
            s.or_b(t + 0x12, 0x10);
        });
        self.post_event(0x302, di as u16);
    }
    /// sub_16DBB: a liberated fort becomes a sietch.
    fn fort_converted(&mut self, di: usize) {
        let (bx, dx) = Self::where_of(di, self.b(di + 8));
        let mut si = 0x0FD8;
        for _ in 0..12 {
            if self.w(si + 2) == bx {
                let ax = self.w(si);
                let al = if ax as u8 == 1 { 1 } else { 2 };
                self.sw(si, (dx & 0xFF00) | al);
            }
            si += 0x10;
        }
        if bx == self.w(6) {
            self.or_b(0x473B, 0x80);
            let ax = self.w(4);
            let al = if ax as u8 == 1 { 1 } else { 2 };
            let v = (dx & 0xFF00) | al;
            self.sw(4, v);
            self.sb(0x0B, al as u8);
            self.sb(8, (dx >> 8) as u8);
        }
        self.walk_all(di, |s, t| {
            if s.w(t + 0x10) & 0x20 != 0 {
                s.and_w(t + 0x10, !0x20);
                s.or_w(t + 0x12, 0x1000);
            }
        });
        self.remove_harkonnens(di);
    }
    /// sub_16E02: every Harkonnen record of a place is removed.
    pub fn remove_harkonnens(&mut self, di: usize) {
        loop {
            let mut cx = 0;
            self.walk_all(di, |s, t| {
                if !s.ours(t) && s.b(t + 0x10) & 0x80 != 0 {
                    s.destroy(t);
                    cx += 1;
                }
            });
            if cx == 0 {
                break;
            }
        }
    }

    /// sub_16FE5: spice mining.
    fn mine(&mut self, si: usize, di: usize) {
        self.place_daily(si, di);
        if self.w(si + 0x10) & 0x200 != 0 {
            if (self.clock() & 0x0F) as u8 == self.b(si) & 0x0F {
                self.and_w(si + 0x10, 0xFDFF);
                if self.pre_mine(si, di) {
                    return;
                }
                self.produce(si, di);
                return;
            }
            self.sw(si + 0x0C, 0);
            self.inactive(si);
            return;
        }
        if self.pre_mine(si, di) {
            self.sw(si + 0x0C, 0);
            self.sw(si + 0x0E, 0);
            self.inactive(si);
            return;
        }
        self.accidents(si, di);
        self.produce(si, di);
    }
    fn produce(&mut self, si: usize, di: usize) {
        self.or_w(si + 0x10, 0x100);
        let r = self.rate(si, di);
        let dx = self.w(si + 0x0E);
        let e = r.wrapping_add(dx);
        self.sw(si + 0x0E, e);
        if (e ^ dx) & 0xFF80 != 0 {
            self.skill_up(si, 0, 1);
        }
        let s = r as u32 + self.w(0x46E1) as u32;
        self.sw(0x46E1, (s % 10) as u16);
        let a0 = self.w(0xA0).wrapping_add((s / 10) as u16);
        self.sw(0xA0, a0);
        let ax = (r as u8 as u16 + self.b(di + 0x13) as u16) + (r & 0xFF00);
        let cl = self.b(di + 0x11) as u16;
        let (q, rem) = if cl == 0 { (0xFF, 0) } else { ((ax / cl).min(0xFF), ax % cl) };
        self.sb(di + 0x13, rem as u8);
        let q = q as u8;
        if q > self.b(di + 0x12) & 0x0F && self.b(0x46EB) & 0x40 != 0 {
            self.inc_b(0x46EC);
        }
        let v = self.b(di + 0x12);
        self.sb(di + 0x12, v.saturating_sub(q));
    }
    /// sub_1708A: production rate per slot (and +0C, trend bits).
    pub fn rate(&mut self, si: usize, di: usize) -> u16 {
        let al = self.motivation(si).wrapping_add(self.b(si + 0x16) & 0xF0);
        let mut p = al as u16 * self.b(si + 0x1A) as u16;
        if self.b(si + 0x19) & 0x80 == 0 {
            p >>= 2;
        }
        let v = ((self.b(di + 0x12) & 0xF0) as u16 + 1) * (p >> 8);
        let r = (v >> 7) & 0x1FF;
        let old = self.w(si + 0x0C);
        self.sw(si + 0x0C, r);
        if old != r {
            let bl = if old < r { 8 } else { 4 };
            let cx = (self.w(si + 0x10) & 0xFFF3) | bl;
            self.sw(si + 0x10, cx);
        }
        r
    }
    /// sub_1714C: harvester accidents.
    fn accidents(&mut self, si: usize, di: usize) {
        if self.b(0x0A) & 1 == 0 || self.b(si + 0x19) & 0x80 == 0 || (self.clock() & 0x0F) as u8 != self.b(si) & 0x0F {
            return;
        }
        // sub_171BC: saboteurs.
        if self.b(0x2A) >= 0x35 && self.w(si + 0x12) & 0x40 != 0 {
            self.rol0();
            self.rol0();
            self.rol0();
            if self.w(0) & 7 == 0 {
                self.or_w(si + 0x10, 0x200);
                self.inactive(si);
                self.or_w(si + 0x10, 0x8000);
                self.or_b(di + 0x0A, 4);
                self.troop_event(3, di);
            }
        }
        let ax = self.rnd();
        let ah = (ax >> 8) as u8;
        let danger = self.b(0x1141 + self.b(di) as usize);
        if danger < ah {
            return;
        }
        self.or_w(si + 0x10, 0x4000);
        if self.b(si + 0x19) & 0x40 != 0 {
            return;
        }
        match ah & 3 {
            0 => {}
            1 => {
                self.or_w(si + 0x10, 0x2000);
                let v = self.b(si + 0x1A);
                if v > 2 {
                    self.sb(si + 0x1A, v - 2);
                }
            }
            2 => {
                self.or_w(si + 0x10, 0x200);
                self.inactive(si);
            }
            _ => {
                self.or_w(si + 0x10, 0x1000);
                self.and_b(si + 0x19, 0x7F);
                self.dec_b(di + 0x14);
                self.troop_event(6, di);
                self.icons = true;
            }
        }
    }
    /// sub_170CC: prospecting.
    fn prospect(&mut self, si: usize, di: usize) {
        let al = self.b(di + 0x0A);
        if al & 2 != 0 {
            return;
        }
        if al & 0x40 == 0 {
            if self.pre_prospect(si, di) {
                return;
            }
            let mut ax = self.w(si + 0x0C);
            if ax == 0 {
                let num = (self.b(di + 0x11) as u16) << 4;
                let cl = self.b(si + 0x15).wrapping_add(self.b(si + 0x16)) as u16;
                ax = if cl == 0 { 0xFF } else { (num / cl).min(0xFF) };
                self.sw(si + 0x0C, ax);
            }
            let cx = self.clock().wrapping_sub(self.w(si + 0x0A));
            if ax > cx {
                if cx != 0 {
                    let v = (cx as u32 * 100 / ax as u32) as u16;
                    self.sw(si + 0x0E, v);
                }
                return;
            }
            self.skill_up(si, 0, 2);
            self.or_b(di + 0x0A, 0x40);
            if self.b(0x46EB) & 0x40 != 0 {
                self.inc_b(0x46EC);
            }
        }
        let was = self.b(si + 3);
        self.pre_prospect(si, di);
        self.sw(si + 0x0E, 100);
        if si == self.w(0x46EF) as usize {
            return;
        }
        if self.w(0x11D3) != 0 {
            self.depart(si, di);
            return;
        }
        if was & 0x10 == 0 {
            self.troop_event(0x0E, di);
        }
    }
    /// sub_171EF: military training (and "repairing").
    fn train(&mut self, si: usize, di: usize) {
        self.place_daily(si, di);
        self.and_w(si + 0x10, 0xFDFF);
        if self.b(di + 0x0A) & 4 != 0 {
            let mut ax = self.w(si + 0x0E);
            if ax >> 8 != 0xFF {
                ax = 0xFF00 | (0x40u8.wrapping_sub(self.b(si + 0x17))) as u16;
            }
            let al = (ax as u8).wrapping_sub(1);
            if (al as i8) >= 0 {
                self.sw(si + 0x0E, (ax & 0xFF00) | al as u16);
                return;
            }
            self.and_b(di + 0x0A, 0xFB);
            let mut cx = 0u16;
            self.walk_ours(di, |s, t| {
                if s.w(t + 0x12) & 0x40 != 0 {
                    s.and_w(t + 0x12, !0x40);
                    cx = 0x100;
                }
            });
            self.or_w(si + 0x12, cx);
            self.sw(si + 0x0E, 0);
            return;
        }
        let c = self.w(si + 0x0C).wrapping_sub(1);
        self.sw(si + 0x0C, c);
        if (c as i16) >= 0 {
            return;
        }
        let avg: u8 = if di == self.w(0x473C) as usize {
            self.or_w(si + 0x10, 0x800);
            0xA0
        } else {
            let (mut cx, mut dx) = (0u16, 0u16);
            self.walk_ours(di, |s, t| {
                if s.b(t + 3) == 4 {
                    cx += 1;
                    dx = dx.wrapping_add(s.b(t + 0x17) as u16);
                }
            });
            if cx == 0 {
                dx as u8
            } else {
                ((dx / (cx & 0xFF).max(1)) & 0xFF) as u8
            }
        };
        let d = avg.saturating_sub(self.b(si + 0x17)) as u16 * 2;
        let mut m = self.motivation(si);
        if m < 0x1E {
            m = 0x1E;
        }
        let cx = d + m as u16;
        let bl = self.b(si + 0x19);
        let base: u16 = if bl & 0x0C != 0 {
            200
        } else if bl & 0x10 != 0 {
            250
        } else if bl & 0x20 != 0 {
            300
        } else {
            400
        };
        self.sw(si + 0x0C, base / cx);
        self.skill_up(si, 1, 1);
    }
    /// sub_172B0: espionage.
    fn spy(&mut self, si: usize, di: usize) {
        let bx = self.clock().wrapping_sub(self.w(si + 0x0A)) as i16;
        if self.b(si + 0x10) & 0x40 == 0 {
            let al = 0x50u8.wrapping_sub(self.b(si + 0x17));
            let bp = (al as i8 as i16) >> 1;
            let ax = bp >> 1;
            if ax < bx {
                let mut done = false;
                if self.w(si + 0x0C) == 0 {
                    let mut cx = 0u16;
                    self.walk_all(di, |s, t| {
                        if s.b(t + 0x10) & 0x10 != 0 {
                            s.and_b(t + 0x10, 0xEF);
                            cx += 1;
                            s.icons = true;
                        }
                    });
                    if cx == 0 {
                        self.or_b(si + 0x10, 0x40);
                        done = true;
                    } else {
                        self.sw(si + 0x0C, cx);
                    }
                }
                if !done && bp < bx {
                    self.side_sums(di);
                    let c = self.w(si + 0x0C).max(1);
                    let v = self.w(0x94) / c;
                    self.sw(si + 0x0E, v);
                    self.or_b(si + 0x10, 0x40);
                }
            }
        }
        if self.b(si) == self.b(di + 9) {
            return;
        }
        let half = (self.b(si + 0x17) >> 1) as u16;
        if half >= bx as u16 {
            return;
        }
        let r = self.rnd() as u8 & 0x3F;
        if r < self.b(si + 0x17) {
            return;
        }
        self.caught(si);
    }
    /// sub_17693: irrigation & tree care.
    fn plant(&mut self, si: usize, di: usize) {
        if self.b(si + 3) & 0x10 != 0 || self.pre_plant(si, di) {
            return;
        }
        self.sb(0x4737, 0xFF);
        self.or_w(si + 0x10, 0x100);
        if self.b(di + 0x0A) & 1 == 0 {
            self.or_b(di + 0x0A, 1);
            self.sb(di + 0x12, 0);
            let (dx, bx) = (self.w(di + 2), self.w(di + 4));
            self.sw(di + 0x0C, dx);
            self.sw(di + 0x0E, bx & 0xFF);
            self.sb(di + 0x0B, 4);
            self.green_disc(di);
            return;
        }
        self.place_daily(si, di);
        let mut al = self.b(si + 0x18) >> 2;
        if al == 0 {
            al = 1;
        }
        let (v, c) = self.b(di + 0x0F).overflowing_add(al);
        self.sb(di + 0x0F, v);
        if !c {
            return;
        }
        self.skill_up(si, 2, 1);
        let (v, c) = self.b(di + 0x1B).overflowing_sub(0x0C);
        self.sb(di + 0x1B, v);
        if c {
            self.sb(di + 0x1B, 0);
            self.inactive(si);
            self.icons = true;
            return;
        }
        if self.b(di + 0x0B) < 0x0C {
            self.inc_b(di + 0x0B);
        }
        let ax = (self.b(di + 0x0E) as i8 as i16) - 2;
        if ax >= -82 {
            self.sb(di + 0x0E, ax as u8);
        }
        self.green_disc(di);
    }
    /// sub_17711: wind-trap assembly.
    fn windtrap(&mut self, si: usize, di: usize) {
        if self.b(di + 0x0A) & 0x20 == 0 {
            self.motiv_add(si, 1);
            let p = self.windtrap_rate(si);
            let (v, c) = self.b(di + 0x1B).overflowing_add(p);
            self.sb(di + 0x1B, v);
            if !c {
                return;
            }
            self.or_b(di + 0x0A, 0x20);
            self.or_b(di + 8, 8);
            self.sb(di + 0x1B, 5);
        }
        self.job_base(si);
    }
    /// sub_1767D: bulb growing.
    fn bulbs(&mut self, si: usize, di: usize) {
        if self.b(di + 0x1A) == 0 {
            self.inc_b(0xEC);
            if self.b(0xEC) != 0 {
                return;
            }
            self.sb(di + 0x1A, 0x10);
        }
        self.set_job_if(si, 8);
    }

    // ------------------------------------------------------------------ battles (§4)
    /// sub_1758D.
    pub fn damage(&self, si: usize, x: u16) -> u8 {
        let m = 0xFFu8.wrapping_sub(self.b(si + 0x17)).wrapping_sub(self.b(si + 0x17));
        let p = x as u32 * m as u32;
        let d = if p >> 16 != 0 { 0xFF } else { (p >> 8) as u8 };
        d.min(self.b(si + 0x1A))
    }
    /// sub_1739E: one battle round.
    pub fn battle(&mut self, si: usize, di: usize) {
        self.or_b(0x11BC, 1);
        if di == HARK_PALACE {
            self.inc_b(0xC2);
            self.walk_ours(di, |s, t| s.sb(t + 3, 4));
            self.remove_harkonnens(di);
            for c in self.map.iter_mut().take(MAP_CELLS) {
                if *c & 0x30 == 0x30 {
                    *c &= 0xEF;
                }
            }
            self.troop_event(0x0A, HARK_PALACE);
            self.icons = true;
            return;
        }
        self.side_sums(di);
        if self.w(0x94) == 0 {
            self.victory(di);
            return;
        }
        let r = self.rnd() as u8;
        if r < self.b(0x9C) {
            self.fremen_strike(si, di);
        } else {
            self.harkonnen_strike(si, di);
        }
    }
    /// loc_173EF.
    fn fremen_strike(&mut self, si: usize, di: usize) {
        let (cx, _) = self.sides(di);
        let mut s = self.strength(si);
        if cx != 0 {
            s /= cx;
        }
        let mut dl = (s as u8).wrapping_add(1);
        if dl == 0 {
            dl = 0xFF;
        }
        let x = dl as u16;
        let (mut killed, mut alive) = (0u16, 0u16);
        self.walk_all(di, |s, t| {
            if s.b(t + 0x10) & 0x80 == 0 {
                return;
            }
            alive += 1;
            let d = s.damage(t, x);
            killed = killed.wrapping_add(d as u16);
            let men = s.b(t + 0x1A);
            s.sb(t + 0x1A, men.wrapping_sub(d));
            if men <= d {
                s.or_b(t + 3, 0x20);
                s.or_b(t + 0x10, 0x10);
                alive -= 1;
                if s.rnd2(3) == 0 {
                    let place = s.w(t + 4) as usize;
                    s.and_b(t + 0x19, 0xFB);
                    s.stock_sub(t, place);
                }
                s.icons = true;
            }
        });
        let v = self.w(si + 0x0C).wrapping_add(killed);
        self.sw(si + 0x0C, v);
        if alive == 0 {
            self.victory(di);
        }
    }
    /// loc_1751D.
    fn harkonnen_strike(&mut self, si: usize, di: usize) {
        let mut x = self.w(0x94);
        let cl = self.b(0x60) as u16;
        if cl != 0 {
            x /= cl;
        }
        let d = self.damage(si, x);
        let v = self.w(si + 0x0E).wrapping_add(d as u16);
        self.sw(si + 0x0E, v);
        let men = self.b(si + 0x1A);
        self.sb(si + 0x1A, men.wrapping_sub(d));
        if men <= d {
            let r = self.rnd2(0x7F) as u8;
            self.sb(si + 0x1A, r.wrapping_add(0x1E));
            self.caught(si);
            let (_, dx) = self.sides(di);
            if dx == 0 {
                self.capture(di);
            }
        }
    }
    /// loc_17429: the place is won.
    pub fn victory(&mut self, di: usize) {
        if di != self.w(0x114E) as usize {
            self.troop_event(7, di);
        }
        if self.b(di + 8) < 0x28 {
            self.and_b(di + 0x0A, 0xFD);
            self.walk_all(di, |s, t| s.victory_fremen(t, di));
        } else {
            self.liberate_core(di);
            return;
        }
        self.victory_tail(di);
    }
    /// loc_17443 (also reached by greening a fort).
    pub fn liberate(&mut self, di: usize) {
        self.liberate_core(di);
    }
    fn liberate_core(&mut self, di: usize) {
        self.sb(di + 0x0B, 5);
        self.paint_ours(di);
        let d = (self.day() as u8).wrapping_add(2);
        self.sb(di + 0x0B, d);
        self.charisma_up(4);
        self.motiv_add_all(1);
        self.or_b(di + 0x0A, 8);
        let cl = self.b(di) as u32;
        self.sw(0x115A, 0x8000u16.rotate_left(cl));
        self.walk_all(di, |s, t| s.victory_fremen(t, di));
        self.walk_all(di, |s, t| s.victory_convert(t));
        self.victory_tail(di);
    }
    fn victory_tail(&mut self, di: usize) {
        let dx = (self.w(0) & 3 == 0) as u16;
        loop {
            let mut cx = 0u16;
            self.walk_all(di, |s, t| {
                if s.ours(t) || s.b(t + 0x10) & 0x80 == 0 {
                    return;
                }
                if cx >= dx {
                    s.destroy(t);
                    cx += 1;
                    return;
                }
                s.sb(t + 3, 0xAC);
                s.or_b(t + 0x10, 0x10);
                s.sb(t + 0x1A, 0);
                s.sw(t + 0x0C, 0);
                s.sb(t + 0x19, 0);
                cx += 1;
            });
            if cx <= dx {
                break;
            }
        }
        let n = self.hark_estimate();
        if n as u8 <= 1 {
            self.sb(0xC2, 1);
            self.and_b(0x0FF7, 0xFD);
            self.and_b(0x1007, 0xFD);
            self.atomics_gift(di);
        }
        self.icons = true;
    }
    /// Walker 75AF (ours only).
    fn victory_fremen(&mut self, t: usize, di: usize) {
        if !self.ours(t) {
            return;
        }
        if t == PROSPECTORS {
            self.and_b(t + 3, 0xDF);
            self.pre_prospect(t, self.w(t + 4) as usize);
            return;
        }
        if self.b(t + 3) & 0x20 != 0 {
            self.sb(t + 3, 0x22);
            return;
        }
        self.or_w(t + 0x10, 0x400);
        if self.b(di + 8) >= 0x28 {
            self.or_w(t + 0x10, 0x20);
        }
        self.motiv_add(t, 4);
        self.skill_up(t, 1, 3);
        self.set_job_if(t, 4);
    }
    /// Walker 75EA (not ours, Harkonnen): a prisoner freed, or removed.
    fn victory_convert(&mut self, t: usize) {
        if self.ours(t) || self.b(t + 0x10) & 0x80 == 0 {
            return;
        }
        self.unlink(t);
        self.and_b(t + 0x10, 0x7F);
        let di = self.w(t + 4) as usize;
        self.link(t, di);
        if self.b(t + 2) >= 8 {
            self.destroy(t);
            return;
        }
        self.sb(t + 3, 0xA0);
        let ax = self.rnd2(0x0F7F).wrapping_add(0x1464);
        self.sb(t + 0x1A, ax as u8);
        self.sb(t + 0x15, (ax >> 8) as u8);
        let ax = self.rnd2(0x1F1F).wrapping_add(0x0A0A);
        self.sb(t + 0x16, ax as u8);
        self.sb(t + 0x17, (ax >> 8) as u8);
        self.sb(t + 0x19, 0);
        self.and_b(t + 0x10, 0xEF);
        self.icons = true;
    }
    /// sub_1765E: the planet's spare atomics go to the last place won.
    fn atomics_gift(&mut self, di: usize) {
        let mut cl = 0u8;
        let mut si = 0x100;
        loop {
            cl = cl.wrapping_add(self.b(si + 0x19));
            si += 0x1C;
            if self.b(si) == 0xFF {
                break;
            }
        }
        let (v, c) = cl.overflowing_sub(0x0A);
        if !c {
            let x = self.b(di + 0x19).wrapping_add(v);
            self.sb(di + 0x19, x);
        }
    }
    /// sub_174B6: the place falls to the Harkonnens.
    pub fn capture(&mut self, di: usize) {
        self.and_b(di + 0x0A, 0xFD);
        if di == self.w(0x114E) as usize {
            self.sb(0x46D9, 6);
            return;
        }
        let al = self.b(di + 8);
        if al < 0x28 {
            self.sb(di + 8, (al & 7) + 0x28);
            self.dec_b(0x27);
            let (bx, dx) = Self::where_of(di, self.b(di + 8));
            let mut si = 0x0FD8;
            for _ in 0..9 {
                if self.w(si + 2) == bx {
                    self.sw(si, (dx & 0xFF00) | 3);
                }
                si += 0x10;
            }
        }
        self.walk_all(di, |s, t| {
            if s.b(t + 0x10) & 0x80 != 0 {
                s.or_b(t + 0x10, 0x10);
            } else {
                s.or_b(t + 3, 0x20);
            }
        });
        self.paint_hark(di, 5);
        self.and_b(di + 0x0A, 0xF6);
        self.icons = true;
    }

    /// sub_160F8: battle gauge of a place.
    pub fn gauge(&mut self, di: usize) -> u8 {
        let (mut f, mut c, mut k, mut h) = (0u32, 0u32, 0u32, 0u32);
        self.walk_all(di, |s, t| {
            let men = s.b(t + 0x1A) as u32;
            if s.b(t + 0x10) & 0x80 != 0 {
                h += men;
                return;
            }
            if s.b(t + 3) != 6 {
                return;
            }
            if s.b(t + 3) & 0x20 == 0 {
                f += men;
            }
            c += s.w(t + 0x0C) as u32;
            k += s.w(t + 0x0E) as u32;
        });
        let (f, c, k, h) = (f as u16, c as u16, k as u16, h as u16);
        let bx = f.wrapping_add(k);
        let a: u16 = if bx == 0 { 0 } else { ((((f & 0xFF) as u32) << 8 | ((f >> 8) as u32) << 16) / bx as u32) as u16 };
        let cx = h.wrapping_add(c);
        let b: u16 = if cx == 0 { 0 } else { ((((h & 0xFF) as u32) << 8 | ((h >> 8) as u32) << 16) / cx as u32) as u16 };
        let m = a.max(b);
        let mut ax = a.wrapping_sub(b);
        if m != 0 {
            // cwd; xchg al,ah; xchg al,dl: dx:ax = (ax << 8) with the sign byte below.
            let sx = ax as i16 as i32;
            let dxv: u16 = if sx < 0 { 0xFFFF } else { 0 };
            let ah_new = ax as u8;
            let al_new = dxv as u8;
            let dl_new = (ax >> 8) as u8;
            let num = (((dxv & 0xFF00) | dl_new as u16) as u32) << 16 | ((ah_new as u32) << 8) | al_new as u32;
            let q = (num as i32) / (m as i16 as i32);
            ax = q as u16;
        }
        let ax = ((ax as i16) >> 1) as u16;
        (ax as u8).wrapping_add(0x80)
    }
    /// sub_16144: ds:FD.
    pub fn set_fd(&mut self, di: usize) {
        let g = self.gauge(di) | 1;
        self.sb(0xFD, g);
    }

    /// sub_1503C: hostility on entering place di (and the war re-check).
    pub fn hostility(&mut self, di: usize) {
        self.sb(0xFD, 0);
        self.sb(0x2B, 0);
        let war = if self.b(di + 0x0A) & 2 != 0 {
            true
        } else if self.friendly(di) {
            return;
        } else {
            let (cx, dx) = self.sides(di);
            if dx != 0 {
                true
            } else {
                if cx != 0 {
                    self.sb(0x46D9, 4);
                }
                return;
            }
        };
        if war {
            self.inc_b(0x2B);
            self.set_fd(di);
            let al = self.b(di + 8);
            let mut ah = 0x2F;
            if al >= 0x20 {
                ah += 1;
                if al != 0x30 && al >= 0x28 {
                    ah += 3;
                }
            }
            self.sb(0x11DD, ah);
        }
    }

    /// sub_183FD: the troops of a battle place take job 6.
    pub fn set_attackers(&mut self, di: usize) {
        self.walk_ours(di, |s, t| {
            if s.b(t + 3) & 0x20 != 0 {
                return;
            }
            match s.b(t + 3) & 0x0F {
                1 => s.inactive(t),
                6 => {}
                _ => s.set_job_if(t, 6),
            }
        });
    }

    /// sub_11F64: Harkonnen attack on a sietch (slot 4). Returns the attacked place when
    /// Paul stands in it (war mode starts at once).
    pub fn harkonnen_attack(&mut self) -> Option<usize> {
        if self.b(0x2A) < 0x3C {
            let (v, c) = self.clock().overflowing_sub(self.w(0x1154));
            if c || v < 0x70 {
                return None;
            }
        }
        if self.clock() & 0x10 != 0 {
            return None;
        }
        let x = self.b(0x11BC);
        self.sb(0x11BC, 0);
        if x != 0 || !self.rol0() {
            return None;
        }
        let (di, bp) = self.attack_target()?;
        self.inc_b(0xC4);
        for _ in 0..2 {
            let mut al = self.b(bp + 9);
            let mut found = 0;
            while al != 0 {
                let t = troop_addr(al);
                al = self.b(t + 1);
                if self.b(t + 0x10) & 0x80 != 0 {
                    found = t;
                    break;
                }
            }
            if found == 0 {
                break;
            }
            self.sb(found + 3, 0x8D);
            self.depart(found, di);
            self.and_b(found + 0x10, 0xEF);
            self.arrive(found);
        }
        self.or_b(di + 0x0A, 2);
        self.set_attackers(di);
        let (bx, dx) = Self::where_of(di, self.b(di + 8));
        let mut si = 0x0FD8;
        for _ in 0..9 {
            if self.w(si + 2) == bx {
                self.sw(si, dx);
            }
            si += 0x10;
        }
        let mut al = 0x0C;
        if self.w(PROSPECTORS + 4) as usize == di {
            al += 1;
        }
        self.troop_event(al, di);
        self.icons = true;
        if bx == self.w(6) {
            self.sw(4, dx);
            self.sb(0x0B, dx as u8);
            self.or_b(0x473B, dx as u8);
            self.sb(0x2B, dx as u8);
            self.set_fd(di);
            return Some(di);
        }
        None
    }
    /// sub_12017.
    fn attack_target(&mut self) -> Option<(usize, usize)> {
        let mut di = 0x100;
        self.sw(0xD816, 0);
        let mut bx: i16 = 0x64;
        while self.b(di) < 8 {
            if self.b(di + 8) < 0x20 && self.b(di + 0x0A) & 0x82 == 0 && bx > self.w(di + 4) as i16 && !self.ill_at(di) {
                self.analyse(di);
                if self.b(0x60) != self.b(0x63) {
                    let fort = if self.w(0xE2) < 0x1E {
                        Some(self.w(0xE4) as usize)
                    } else if self.w(0xDC) < 0x1E {
                        Some(self.w(0xDE) as usize)
                    } else {
                        None
                    };
                    if let Some(bp) = fort {
                        if bp != HARK_PALACE {
                            let (cx, dx) = self.sides(bp);
                            if cx != 0 && dx == 0 {
                                bx = self.w(di + 4) as i16;
                                self.sw(0xD816, di as u16);
                                self.sw(0xD818, bp as u16);
                            }
                        }
                    }
                }
            }
            di += 0x1C;
        }
        let di = self.w(0xD816) as usize;
        (di != 0).then(|| (di, self.w(0xD818) as usize))
    }

    // ------------------------------------------------------------------ travel (§2.8)
    /// sub_1848F: troop 3 skips itinerary entries equal to its place; returns ds:11D3.
    fn itinerary_next(&mut self, si: usize) -> usize {
        let mut guard = 0;
        loop {
            let di = self.w(0x11D3);
            if self.w(si + 4) == di && self.b(si + 3) & 0x40 == 0 && guard < 4 {
                self.itinerary_shift();
                guard += 1;
                continue;
            }
            return di as usize;
        }
    }
    /// seg000:8347.
    fn itinerary_shift(&mut self) {
        for k in 0..6 {
            let v = self.b(0x11D5 + k);
            self.sb(0x11D3 + k, v);
        }
    }
    /// sub_184A6: the troop starts moving to di. Returns false if refused.
    pub fn depart(&mut self, si: usize, mut di: usize) -> bool {
        if si == PROSPECTORS {
            di = self.itinerary_next(si);
            if di == 0 {
                return false;
            }
        }
        if self.b(si + 3) & 0x40 != 0 {
            self.sw(si + 4, di as u16);
            if self.b(si + 3) & 3 == 3 {
                self.and_b(si + 3, 0xFC);
            }
            self.icons = true;
            return true;
        }
        // sub_16EBF: strikers of the origin re-apply their job.
        let origin = self.w(si + 4) as usize;
        self.walk_ours(origin, |s, t| {
            if s.b(t + 0x12) & 0x10 != 0 {
                let cl = s.b(t + 3) & 0x0F;
                s.set_job(t, cl);
            }
        });
        self.unlink(si);
        // Only a troop whose status is exactly 6 (attacking) does these checks.
        if self.b(si + 3) == 6 {
            let o = self.w(si + 4) as usize;
            if self.b(o + 0x0A) & 2 != 0 {
                let (cx, dx) = self.sides(o);
                if cx != 0 && (dx as i16) - 1 <= 0 {
                    self.capture(o);
                }
            }
            if self.b(di + 8) < 0x28 {
                if self.b(di + 0x0A) & 2 != 0 {
                    // Quirk: refused, but the troop is already unlinked.
                    return false;
                }
                self.motiv_sub(si, 3);
            }
        }
        let o = self.w(si + 4) as usize;
        self.sw(si + 4, di as u16);
        self.or_b(si + 3, 0x40);
        self.sb(si + 2, 0);
        self.stock_sub(si, o);
        self.icons = true;
        if self.b(si + 0x10) & 0x10 == 0 {
            self.march(si, 7);
        }
        true
    }
    /// sub_18308 / loc_18313: cx steps (4, 8 with an orni; 7 at departure).
    pub fn march(&mut self, si: usize, mut cx: u16) {
        if cx == 4 && self.b(si + 0x19) & 0x40 != 0 {
            cx = 8;
        }
        for _ in 0..cx {
            let (dx, bx) = self.step(si);
            if dx == 0 && bx == 0 {
                self.arrive(si);
                return;
            }
            let v = self.w(si + 6).wrapping_add(dx);
            self.sw(si + 6, v);
            let v = self.w(si + 8).wrapping_add(bx);
            self.sw(si + 8, v);
        }
        self.icons = true;
    }
    /// sub_18604.
    fn step(&mut self, si: usize) -> (u16, u16) {
        let di = self.w(si + 4) as usize;
        let dlon = self.w(di + 2);
        let row = self.w(si + 8) as i16;
        let r_abs = (row as i32).unsigned_abs().min(98) as usize;
        let mut bp = self.w(planet::LON_UNITS + 2 * r_abs);
        let r = (self.w(di + 4) as i16).wrapping_sub(row);
        let (sr, ar): (u16, u16) = if r < 0 { (0xFFFF, r.wrapping_neg() as u16) } else { (1, r as u16) };
        let l = dlon.wrapping_sub(self.w(si + 6));
        let al = if (l as i16) < 0 { l.wrapping_neg() } else { l };
        let c = if bp == 0 { 0xFFFF } else { al / bp };
        if c >= ar {
            if c < 7 {
                return (0, 0);
            }
            let mut bx = sr;
            if ar == 0 {
                bx = 0;
            }
            if (l as i16) < 0 {
                bp = bp.wrapping_neg();
            }
            if c >> 1 == 0 {
                return (l, bx);
            }
            let dx = bp;
            if !self.rol0() {
                bx = 0;
            }
            (dx, bx)
        } else {
            if ar < 7 {
                return (0, 0);
            }
            if c == 0 {
                return (l, sr);
            }
            if (l as i16) < 0 {
                bp = bp.wrapping_neg();
            }
            let dx = if self.rol0() { bp } else { 0 };
            (dx, sr)
        }
    }
    /// loc_18357: arrival.
    pub fn arrive(&mut self, si: usize) {
        if si == PROSPECTORS && self.w(0x11D3) == self.w(si + 4) {
            self.itinerary_shift();
        }
        let mut al = self.b(si + 3) & 0x0F;
        let di = self.w(si + 4) as usize;
        let (dx, bx) = (self.w(di + 2), self.w(di + 4));
        self.sw(si + 6, dx);
        self.sw(si + 8, bx);
        self.icons = true;
        if self.b(di + 0x0A) & 2 != 0 || !self.friendly(di) {
            self.arrive_core(si);
            let di = self.w(si + 4) as usize;
            if al == 5 && self.b(di + 0x0A) & 0x80 != 0 {
                // Espionage discovers the fort.
                self.and_b(di + 0x0A, 0x7F);
                return;
            }
            if al != 5 && self.b(si) == self.b(di + 9) {
                self.victory(di);
                return;
            }
            self.set_attackers(di);
            return;
        }
        if al == 5 || al == 6 {
            self.job_base(si);
            al = 0;
        }
        if al & 3 == 3 {
            // Search for equipment (2.6 step 6).
            let di = self.w(si + 4) as usize;
            if di == self.w(si + 0x0C) as usize {
                self.and_b(si + 3, 0xFC);
                self.arrive_core(si);
                return;
            }
            self.free_stock(di);
            let mut cx = self.w(si + 0x0E);
            let bl = cx as u8 as usize;
            if self.b(0x46FE + bl) != 0 {
                self.dec_b(di + 0x14 + bl);
                self.or_b(si + 0x19, (cx >> 8) as u8);
                cx &= 0x00FF;
            }
            self.sw(si + 0x0E, cx);
            let home = self.w(si + 0x0C);
            self.sw(si + 4, home);
            return;
        }
        self.arrive_core(si);
    }
    /// seg000:83BC.
    fn arrive_core(&mut self, si: usize) {
        let di = self.w(si + 4) as usize;
        self.link(si, di);
        self.and_b(si + 3, 0xBF);
        self.make_room(si);
        let di = self.w(si + 4) as usize;
        self.stock_add(si, di);
        let cl = self.b(si + 3);
        if (cl as i8) < 0 {
            return;
        }
        self.set_job(si, cl & 0x0F);
        if di == self.w(0x114E) as usize {
            // Menu 1F0E active and in the right room: room refresh (checked by the UI).
            let al = 1 + (self.b(0x2B) < 1) as u8;
            if self.b(0x0B) == al {
                self.or_b(0x473B, 1);
            }
        }
    }

    // ------------------------------------------------------------------ the Emperor (§3.7)
    /// sub_12090.
    pub fn emperor_start(&mut self) {
        let d = self.day();
        self.sw(0x118D, d);
        self.new_demand(d & 0xFF00);
    }
    /// loc_120D2 (ah = high byte of the day in the original multiplicand).
    pub fn new_demand(&mut self, ah: u16) {
        let n = (ah & 0xFF00) | self.b(0xC3) as u16;
        self.inc_b(0xC3);
        let p = n as u32 * 0x96;
        let bc = 'calc: {
            if p >> 16 != 0 {
                break 'calc 0xFFFF;
            }
            let (v, c) = (p as u16).overflowing_add(0x64);
            if c {
                break 'calc 0xFFFF;
            }
            let r = self.rnd2(0x3F).wrapping_add(0xE0);
            let p = r as u32 * v as u32;
            if p >> 24 != 0 {
                break 'calc 0xFFFF;
            }
            let q = ((p >> 8) & 0xFFFF) as u16;
            let be = self.b(0xBE);
            if be & 0x80 != 0 {
                break 'calc q;
            }
            let al = !(be << 1);
            let mul = 0x100u32 | al as u32;
            let p2 = mul * q as u32;
            if p2 >> 24 != 0 {
                0xFFFF
            } else {
                ((p2 >> 8) & 0xFFFF) as u16
            }
        };
        self.sw(0xBC, bc);
        self.sb(0xCF, 0);
        self.or_b(0xBF, 0x90);
        let ax = if self.b(0xBE) & 0x80 != 0 { 0x020B } else { 0x030B };
        self.post_message(ax);
    }
    /// loc_120A4 (slot 3).
    pub fn emperor_check(&mut self) {
        if self.b(0xBF) & 0x80 == 0 {
            return;
        }
        let d = self.day();
        if self.b(0xC2) != 0 {
            let k = d.wrapping_sub(self.w(0x118D));
            if k != 0 {
                self.sb(0xCF, k.wrapping_neg() as u8);
            }
            return;
        }
        if self.b(0xBF) & 0x10 != 0 {
            let k = d.wrapping_sub(self.w(0x118D));
            if k == 0 {
                return;
            }
            if k >= 4 {
                self.sb(0x46D9, 7);
                return;
            }
            let bl = (k as u8) << 2;
            let mut b = self.bucket();
            if b >= 3 {
                b = 2;
            }
            const T: [u8; 16] = [0xD9, 0x46, 0x07, 0xC3, 4, 5, 6, 0, 5, 6, 0, 0, 6, 0, 0, 0];
            let p = T[(bl + b) as usize & 15];
            if p == 0 {
                self.sb(0x46D9, 7);
            } else {
                self.post_message(((p as u16) << 8) | 0x0B);
            }
            return;
        }
        if self.b(0x11BB) != 0 {
            self.sb(0x46D9, 7);
            return;
        }
        let ax = d.wrapping_sub(self.w(0x118D));
        if ax == 0 {
            self.new_demand(0);
            return;
        }
        self.sb(0xCF, ax.wrapping_neg() as u8);
    }
    /// sub_124D2: ah = bucket of ds:BE.
    pub fn bucket(&self) -> u8 {
        let al = self.b(0xBE);
        [1u8, 0x40, 0x80, 0x90, 0xFF].iter().filter(|&&t| al < t).count() as u8
    }
    /// sub_122B1: the four proposed amounts.
    pub fn offers(&mut self, ax: u16) {
        self.and_b(0xBF, 0xF9);
        let bx = self.w(0xBC);
        let cx = (bx >> 1).wrapping_add(bx);
        let dx = bx.wrapping_add(bx);
        let si = ax >> 1;
        let di = (ax >> 2).wrapping_add(si);
        let bp = 0xB4;
        if ax < bx {
            self.sw(bp, ax);
            self.sw(bp + 2, di);
            self.sw(bp + 4, si);
            self.sw(bp + 6, di.wrapping_sub(si));
            return;
        }
        self.sw(bp, bx);
        self.sw(bp + 2, ax);
        if ax < cx {
            self.sw(bp + 4, di);
            self.sw(bp + 6, si);
            self.or_b(0xBF, 2);
        } else if ax < dx {
            self.sw(bp + 4, di);
            self.sw(bp + 6, cx);
            self.or_b(0xBF, 4);
        } else {
            self.sw(bp + 4, cx);
            self.sw(bp + 6, dx);
            self.or_b(0xBF, 6);
        }
    }
    /// sub_12566 up to the cut-scene.
    pub fn ship(&mut self, amount: u16) {
        let a0 = self.w(0xA0).wrapping_sub(amount);
        self.sw(0xA0, a0);
        let v = self.w(0x1172).wrapping_add(amount);
        self.sw(0x1172, v);
        let bc = self.w(0xBC).max(1) as u32;
        let mut r = (((amount as u32) << 8) / bc).min(0xFFFF);
        if r >= 0x200 {
            r = 0x1FF;
        }
        let mut al = (r >> 1) as u8;
        if al == 0 {
            al = 1;
        }
        self.sb(0xBE, al);
        let (bx, mut ah) = if al >= 0xC0 {
            (7, 0x40)
        } else if al > 0x80 {
            (6, 0x40)
        } else if al == 0x80 {
            (5, 0)
        } else {
            if self.b(0xBF) & 8 != 0 {
                self.sb(0xBE, 0);
            }
            (4, 8)
        };
        ah |= 0x80;
        self.sb(0xBF, ah);
        let v = self.w(0x118D).wrapping_add(bx);
        self.sw(0x118D, v);
        let m = ((self.b(0xC3) >> 1) & 3) as u16;
        let r = self.rnd2(m);
        let v = self.w(0x118D).wrapping_add(r);
        self.sw(0x118D, v);
        let cf = v.wrapping_sub(self.day());
        self.sb(0xCF, cf as u8);
        self.sw(0x1158, 0);
    }
    /// sub_11071: phase 0x15, the spice demands start.
    pub fn demands_start(&mut self) {
        self.sb(0xFF, 0);
        self.sb(0x2A, 0x15);
        self.sb(0x0FDB, 1);
        self.sw(0x1018, 0x200B);
        self.sw(0x101A, 0x0180);
        self.sb(0x0FE8, 0x0A);
        self.sb(0xD5, 0xFF);
        self.emperor_start();
        self.or_b(0x0A, 1);
        self.post_event(1, 0);
    }

    // ------------------------------------------------------------------ smugglers (§3.12)
    /// sub_12318: room entry in a village.
    pub fn smuggler_enter(&mut self, di: usize) {
        let al = self.b(di);
        let mut si = 0x10C7;
        loop {
            si += 0x11;
            if self.b(si) == al || si > 0x1140 {
                break;
            }
        }
        if si > 0x1140 {
            return;
        }
        self.smuggler_vars(si);
        let mut al = (self.day() as u8).wrapping_sub(self.b(si + 3));
        if self.b(si + 2) & 8 == 0 {
            al = 1;
            self.or_b(si + 2, 8);
        }
        self.sb(0x1E, al);
        self.sb(0x9D, 0);
        let mut ax = self.w(0) & 7;
        let n = self.b(0x1141) as u16;
        if n != 0 {
            while ax >= n {
                ax -= n;
            }
        }
        self.sw(0x11F1, ax + 0xE8);
        self.sb(0x9F, 0);
    }
    /// sub_1235F.
    fn smuggler_vars(&mut self, si: usize) {
        self.sw(0x10B4, si as u16);
        let v = self.b(si + 2);
        self.sb(0x1C, v);
        let ax = self.w(si + 0x0E);
        self.sw(0x20, ax);
        self.sb(0x1F, 0);
        if ax != 0 {
            let v = (self.day() as u8).wrapping_sub(self.b(si + 0x10));
            self.sb(0x1F, v);
        }
        let v = self.b(si + 1);
        self.sb(0x1D, v);
    }
    /// seg000:2388: the smuggler's offer (action 8).
    pub fn smuggler_offer(&mut self) {
        self.raise_phase(0x3C);
        let r = self.rnd2(3) as u8;
        self.sb(0x9E, r);
        let di = self.w(0x10B4) as usize;
        let d = self.day() as u8;
        self.sb(di + 3, d);
        self.sb(0x1A, 0);
        let n = self.b(0x1141);
        let mut ax = self.w(0x11F1).wrapping_sub(0xE8) as u8;
        let mut cx = 2;
        loop {
            ax = ax.wrapping_add(1);
            loop {
                if ax < n {
                    break;
                }
                ax = ax.wrapping_sub(n);
                cx -= 1;
                if cx == 0 {
                    return;
                }
            }
            if self.b(di + 4 + ax as usize) != 0 {
                break;
            }
        }
        self.sw(0x11F1, ax as u16 + 0xE8);
        let p = (self.b(di + 9 + ax as usize) & 0x7F) << 1;
        self.sb(0x9D, p);
    }
    /// sub_123E6: bought.
    pub fn smuggler_accept(&mut self) {
        let di = self.w(0x10B4) as usize;
        self.and_b(di + 2, 0x9F);
        let ax = self.b(0x9D) as u16;
        self.sb(0x9D, 0);
        let v = self.w(0x20).wrapping_add(ax);
        self.sw(0x20, v);
        let v = self.w(di + 0x0E).wrapping_add(ax);
        self.sw(di + 0x0E, v);
        if ax == v {
            self.inc_b(0x22);
        }
        let d = self.day() as u8;
        self.sb(di + 0x10, d);
        let bx = self.w(0x11F1).wrapping_sub(0xE8) as usize & 0xFF;
        self.dec_b(di + 4 + bx);
        let si = self.w(0x114E) as usize;
        self.inc_b(si + 0x14 + bx);
    }
    /// ARGUE / ACCEPT / REFUSE handlers (241A / 2432 / 2453): the answer code ds:9F.
    pub fn haggle(&mut self, answer: u8, smuggler: bool) {
        let al = if !smuggler {
            answer
        } else {
            let di = self.w(0x10B4) as usize;
            match answer {
                1 => {
                    self.smuggler_accept();
                    1
                }
                2 => {
                    if self.rnd2(7) == 0 {
                        self.or_b(0x9E, 0x10);
                        3
                    } else {
                        self.sb(0x9D, 0);
                        2
                    }
                }
                _ => {
                    let r = self.rnd2(3);
                    if r == 0 {
                        self.or_b(0x9E, 0x10);
                    } else {
                        let v = (self.b(0x9E).wrapping_add(1)) & 3;
                        self.sb(0x9E, v);
                        let (_, c) = ((r as u8 & 1).wrapping_add(self.b(0x1A))).overflowing_sub(self.b(di + 1));
                        if !c {
                            self.sb(0x9D, 0);
                        } else {
                            let p = self.b(0x9D);
                            self.sb(0x9D, p - (p >> 3));
                        }
                    }
                    3
                }
            }
        };
        self.sb(0x9F, al);
        self.inc_b(0x1A);
    }
    /// seg000:2239 (Idaho, action 8): shipment offers and smuggler debts.
    pub fn idaho_setup(&mut self) {
        self.sb(0x9F, 0);
        self.sw(0x20, 0);
        self.sb(0x1A, 0);
        let a0 = self.w(0xA0);
        if a0 != 0 {
            self.sb(0x9F, 3);
        }
        self.offers(a0);
        let d = self.day() as u8;
        let mut si = 0x10D8;
        let mut dl = 0u8;
        let mut di = 0;
        while self.b(si) != 0xFF {
            if self.w(si + 0x0E) != 0 && self.b(si + 2) & 0x60 == 0 {
                let ah = d.wrapping_sub(self.b(si + 0x10));
                if ah > dl {
                    dl = ah;
                    di = si;
                }
            }
            si += 0x11;
        }
        let si = if dl != 0 {
            di
        } else {
            let start = self.w(0x113F) as usize;
            let mut s = start;
            let mut found = None;
            for _ in 0..8 {
                s += 0x11;
                if self.b(s) == 0xFF {
                    s = 0x10D8;
                }
                if self.w(s + 0x0E) != 0 {
                    found = Some(s);
                    break;
                }
                if s == start {
                    break;
                }
            }
            match found {
                Some(s) => {
                    self.sw(0x113F, s as u16);
                    s
                }
                None => return,
            }
        };
        self.smuggler_vars(si);
        let al = self.b(si) as u16;
        self.sw(0x11F7, al);
    }
    /// seg000:24EE (Idaho, action 9): shipment agreed / smuggler paid.
    pub fn idaho_accept(&mut self) {
        let f = self.b(0x9F);
        let smug = self.b(0x476D) != 0;
        if f == 2 {
            if smug {
                let di = self.w(0x10B4) as usize;
                self.and_b(di + 2, 0x9F);
                self.or_b(di + 2, 0x40);
            }
            return;
        }
        if f > 2 {
            if smug {
                let di = self.w(0x10B4) as usize;
                self.and_b(di + 2, 0x9F);
                self.or_b(di + 2, 0x20);
            }
            return;
        }
        if !smug {
            let bx = ((self.b(0x1A).wrapping_sub(1)) & 3) as usize;
            let ax = self.w(0xB4 + 2 * bx);
            self.sw(0xC0, ax);
            self.sw(0x1158, 0xFFFF);
            return;
        }
        let di = self.w(0x10B4) as usize;
        let ax = self.w(di + 0x0E);
        self.sw(di + 0x0E, 0);
        self.dec_b(0x22);
        let v = self.w(0xA0).wrapping_sub(ax);
        self.sw(0xA0, v);
        let v = self.w(0x1172).wrapping_add(ax);
        self.sw(0x1172, v);
    }
    /// seg000:24A3 (Idaho, action 15).
    pub fn idaho_reaction(&mut self) {
        if self.b(0x2A) < 0x10 {
            self.or_b(0x0FF7, 0x10);
            return;
        }
        self.inc_b(0x47A8);
        self.sw(0xC0, 0);
        self.or_b(0xBF, 1);
        let ah = self.bucket() + 7;
        if ah == 0x0C {
            self.inc_b(0x11BB);
        }
        self.post_message(((ah as u16) << 8) | 0x0B);
    }

    // ------------------------------------------------------------------ endgame (§6.2)
    /// sub_11243: Hawat's check, C2 4 → 5.
    pub fn hawat_check(&mut self) {
        let mut bx = 0u16;
        for di in [0x138, 0x154, 0x170] {
            self.walk_ours(di, |s, t| {
                if s.b(t + 3) == 4 && s.b(t + 0x19) & 4 != 0 {
                    bx = bx.wrapping_add(s.b(t + 0x1A) as u16);
                }
            });
        }
        if bx >= 0x3E8 {
            self.inc_b(0xC2);
        }
    }
    /// seg000:2D2C (Stilgar, action 9): the final assault.
    pub fn final_assault(&mut self) {
        self.inc_b(0xC2);
        let mut list = vec![];
        for di in [0x138, 0x154, 0x170] {
            self.walk_ours(di, |s, t| {
                if s.b(t + 3) == 4 && s.b(t + 0x19) & 4 != 0 {
                    list.push(t);
                }
            });
        }
        for &t in list.iter().take(7) {
            self.depart(t, HARK_PALACE);
            self.march(t, 4);
        }
    }

    // ------------------------------------------------------------------ player tools (§4.7)
    /// MASSIVE ATTACK (seg000:7317) without the scene flashes.
    pub fn massive_attack(&mut self) {
        self.sb(0x473A, 1);
        let di = self.w(0x114E) as usize;
        self.side_sums(di);
        let (h, f) = (self.w(0x94), self.w(0x96));
        self.sw(0x98, h);
        self.sw(0x9A, f);
        let fremen = (self.rnd() as u8) < self.b(0x9C);
        for _ in 0..16 {
            let di = self.w(0x114E) as usize;
            self.side_sums(di);
            if fremen {
                self.walk_all(di, |s, t| {
                    if s.b(t + 3) != 6 {
                        return;
                    }
                    if s.w(0x94) == 0 {
                        s.victory(di);
                    } else {
                        s.fremen_strike(t, di);
                    }
                });
            } else {
                self.walk_all(di, |s, t| {
                    if s.b(t + 3) == 6 {
                        s.harkonnen_strike(t, di);
                    }
                });
            }
            if self.w(0x94) == 0 || self.w(0x96) == 0 {
                break;
            }
        }
        let di = self.w(0x114E) as usize;
        self.side_sums(di);
        let v = self.w(0x98).wrapping_sub(self.w(0x94));
        self.sw(0x98, v);
        let v = self.w(0x9A).wrapping_sub(self.w(0x96));
        self.sw(0x9A, v);
    }

    /// sub_131F6: the CONDIT variables of a troop.
    pub fn troop_vars(&mut self, si: usize) {
        let di = self.w(si + 4);
        self.sw(0x2C, di);
        let v = self.b(si);
        self.sb(0x2E, v);
        let al = self.b(si + 3);
        self.sb(0x30, al);
        let job = al & 0x0F;
        self.sb(0x2F, job);
        self.sw(0x11F3, job as u16 + 0x18);
        // sub_132C7
        let ax = self.clock().wrapping_sub(self.w(si + 0x0A));
        self.sw(0x42, ax);
        self.sb(0x41, (ax >> 4) as u8);
        let id = if self.b(si + 3) & 0x10 != 0 {
            0x74
        } else if ax < 3 {
            0x70
        } else if ax < 0x10 {
            0x71
        } else if ax < 0x20 {
            0x72
        } else {
            0x73
        };
        self.sw(0x11F5, id);
        // sub_1329D
        let ax = if self.b(si + 3) == 0 {
            let cx = self.w(0x42);
            let mut a = 0;
            if cx != 0 {
                let e = self.w(si + 0x0E);
                let (q, r) = (e / cx, e % cx);
                // shl dx,1 ; cmp cx,dx ; adc ax,0
                a = q.wrapping_add((cx < r.wrapping_shl(1)) as u16);
            }
            self.sw(0x4A, a);
            let place = self.w(si + 4) as usize;
            self.rate(si, place)
        } else {
            self.and_w(si + 0x10, 0xFFF3);
            0
        };
        self.sw(0x48, ax);
        let v = self.w(si + 0x10);
        self.sw(0x32, v);
        let ax = self.w(si + 0x12);
        self.sw(0x34, ax);
        self.sb(0x31, (ax & 0x0F) as u8);
        self.sw(0x11FF, ax & 0x0F);
        let m = self.motivation(si);
        self.sb(0x36, m);
        for (k, (var, txt)) in [(0x38usize, 0x11F7usize), (0x39, 0x11F9), (0x3A, 0x11FB)].iter().enumerate() {
            let s = self.b(si + 0x16 + k);
            self.sb(*var, s);
            self.sw(*txt, (s >> 4) as u16 + 0xD1);
        }
        let v = self.w(si + 0x0C);
        self.sw(0x44, v);
        let v = self.w(si + 0x0E);
        self.sw(0x46, v);
        self.sw(0x11F1, (v & 0xFF) + 0xE8);
        let c = self.class(si) as usize;
        let v = self.b(si + 0x16 + c);
        self.sb(0x37, v);
        let v = self.b(si + 0x19);
        self.sb(0x3B, v);
        let v = self.b(si + 0x1A);
        self.sb(0x3C, v);
        let v = (self.day() as u8).wrapping_sub(self.b(si + 0x14));
        self.sb(0x40, v);
        let place = self.w(si + 4) as usize;
        if place >= 0x100 && place < 0x8A8 {
            self.analyse(place);
        }
    }
    /// sub_12E98: place-name ids of ds:2C for the "-" substitution (ds:11ED/11EF).
    pub fn place_name_vars(&mut self, di: usize) {
        if di < 0x100 || di >= 0x8A8 {
            return;
        }
        let (b0, b1) = (self.b(di) as u16, self.b(di + 1) as u16);
        self.sw(0x11ED, b0);
        self.sw(0x11EF, b1 + 0x0C);
    }

    /// sub_17C63: distance in cells from Paul to a troop.
    pub fn troop_distance(&self, si: usize, paul: (u16, i16)) -> u16 {
        let (lon, row) = paul;
        let units = self.w(planet::LON_UNITS + 2 * (row as i32).unsigned_abs().min(98) as usize).max(1);
        let d = (self.w(si + 6).wrapping_sub(lon) as i16 as i32).unsigned_abs() as u16 / units;
        let dr = (row.wrapping_sub(self.w(si + 8) as i16) as i32).unsigned_abs() as u16;
        d.max(dr)
    }
}

#[cfg(test)]
mod tests;
