//! The player's side of the war and the story glue (docs/re/war.md §5–§7):
//! the per-slot hook of the simulation, hostility on entering a place, war mode with the
//! ATTACK.HSQ scene (`sub_10ACD` + task 0xB45), MASSIVE ATTACK, FIGHT FOR A WHOLE DAY,
//! game overs (`sub_10D8E`, DEAD.HNM / DEAD2.HNM), report events and visions
//! (`sub_12B2A`), the communications room messages, the Emperor's shipments and the ending.

use crate::{
    assets::Assets,
    attract::{dir, subpixel, Bank},
    audio::Audio,
    talk::After,
    world::{Cmd, Line, Scr, World, VIEW},
};
use dune_formats::{hnm::HnmPlayer, palette::Palette, sprite::DrawOpts};

// ---------------------------------------------------------------------------------------
// ATTACK scene (animations.md §2.7)

#[derive(Clone, Copy, Default)]
struct AObj {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    sprite: u16,
    vel: u16,
    frac: u16,
}

pub struct AttackScene {
    bank: Option<Bank>,
    s0: i8,
    s1: i8,
    s2: u8,
    s3: u8,
    s4: i16,
    s6: i8,
    pub s7: u8,
    objs: Vec<AObj>,
    clean: Vec<u8>,
    pub view: Vec<u8>,
    acc: u32,
    /// Flash target (ATTACK palette entry 0x35) and the current colours 128..155.
    target: Palette,
}

impl AttackScene {
    /// sub_10ACD: tiles, the ground strip ds:11DD, the task.
    pub fn new(assets: &Assets, strip: u16, pal: &mut Palette) -> AttackScene {
        let bank = Bank::load(assets, 0x2B);
        let mut clean = vec![0u8; VIEW];
        let mut target = pal.clone();
        if let Some(b) = &bank {
            if let Some(sh) = assets.dat.read(&assets.resource_name(0x2B).unwrap_or_default()).ok().and_then(|d| dune_formats::sprite::SpriteSheet::parse(&d)) {
                sh.apply_palette(pal);
            }
            for (spr, r) in [(2u16, (0, 0, 320, 81)), (3, (0, 81, 320, 134))] {
                let (w, h) = b.dims(spr as usize);
                let (w, h) = (w.max(1), h.max(1));
                if let Some(s) = b.sprite(spr as usize) {
                    let mut tile = vec![0u8; (w * h) as usize];
                    s.draw(&mut tile, w as usize, 0, 0, DrawOpts::default());
                    for y in r.1..r.3 {
                        for x in r.0..r.2 {
                            let (sx, sy) = ((x - r.0) % w, (y - r.1) % h);
                            clean[(y * 320 + x) as usize] = tile[(sy * w + sx) as usize];
                        }
                    }
                }
            }
            // List ds:11DD: strip @ (0, 76), sprite 1 @ (0, 134).
            for (spr, x, y) in [(strip, 0, 76), (1, 0, 134)] {
                if let Some(s) = b.sprite(spr as usize) {
                    s.draw(&mut clean, 320, x, y, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
                }
            }
            for (k, c) in b.pal_entry(0x35, 0x54).iter().enumerate() {
                if 128 + k < 256 {
                    target.0[128 + k] = *c;
                }
            }
        }
        AttackScene { bank, s0: 0, s1: 0, s2: 0, s3: 0, s4: 0, s6: 0, s7: 0, objs: vec![], view: clean.clone(), clean, acc: 0, target }
    }

    fn spawn(&mut self, sprite: u16, x: i32, y: i32, vel: u16) {
        let Some(b) = &self.bank else { return };
        let (w, h) = b.dims(sprite as usize);
        let (x, y) = (x - (w >> 1), y - (h >> 1));
        self.objs.push(AObj { x0: x, y0: y, x1: x + w, y1: y + h, sprite, vel, frac: 0 });
    }

    /// Task 0xB45 every 3 ticks. `ds0` is ds:0000, `rnd`/`rnd2` the game RNGs.
    pub fn tick(&mut self, w: &mut World, pal: &mut Palette) -> bool {
        self.acc += 1;
        if self.acc < 3 {
            return false;
        }
        self.acc = 0;
        self.s7 = self.s7.wrapping_sub(1);
        let s7 = self.s7;
        if (w.mem[0xEA] as i8) <= 0 && s7 <= 0x10 {
            self.flash(s7, pal);
        }
        self.s4 = self.s4.wrapping_sub(1);
        if self.s4 < 0 {
            self.s6 = self.s6.wrapping_sub(1);
            if self.s6 < 0 {
                let r = w.sim().rnd();
                self.s6 = (r & 0x7F) as i8;
                self.s4 = (r >> 8) as i16;
            } else {
                let r = w.sim().rnd();
                let y = (r & 0x7F) as i32;
                if (0x30..0x60).contains(&y) {
                    let x = ((((r >> 7) & 1) << 8) | (r >> 8)) as i32;
                    if x < 320 {
                        self.spawn(0x1C + (y & 7) as u16, x, y, 0);
                    }
                }
            }
        }
        self.s0 = self.s0.wrapping_sub(1);
        if self.s0 < 0 {
            self.new_aircraft(w);
        }
        let mut i = 0;
        let n = self.objs.len();
        for _ in 0..n {
            if i >= self.objs.len() {
                break;
            }
            let o = self.objs[i];
            let mut s = o.sprite & 0xFF;
            let (mut mx, mut my) = ((o.vel & 0xFF) as u8 as i8 as i32, (o.vel >> 8) as u8 as i8 as i32);
            if s < 0x14 {
                let mut ax = o.sprite >> 2;
                for _ in 0..2 {
                    let v = w.get16(0);
                    w.set16(0, v.rotate_left(1));
                    ax = (ax << 1) | (v >> 15);
                }
                s = ax;
            } else if s < 0x1C {
                let (sx, fx) = subpixel(o.vel as u8, o.frac as u8);
                let (sy, fy) = subpixel((o.vel >> 8) as u8, (o.frac >> 8) as u8);
                self.objs[i].frac = (fy as u16) << 8 | fx as u16;
                let v = w.get16(0).rotate_left(3);
                w.set16(0, v);
                s = 0x14 + (v & 7);
                mx = sx;
                my = sy;
            } else {
                s += 1;
                if s > 0x2D {
                    self.objs.remove(i);
                    continue;
                }
            }
            let ob = &mut self.objs[i];
            ob.sprite = s;
            ob.x0 += mx;
            ob.x1 += mx;
            ob.y0 += my;
            ob.y1 += my;
            if !(0..0x140).contains(&ob.x0) || ob.x1 < 0 || ob.y1 < 0 {
                self.objs.remove(i);
                continue;
            }
            i += 1;
        }
        self.render();
        true
    }

    fn new_aircraft(&mut self, w: &mut World) {
        self.s1 = self.s1.wrapping_sub(1);
        if self.s1 < 0 {
            let ds0 = w.get16(0);
            if ds0 & 3 == 0 {
                self.s7 = 0x0B;
                if ds0 & 0x0C == 0 {
                    self.s7 = 0x11;
                }
            }
            let mut r = w.sim().rnd();
            if w.mem[0x473A] != 0 {
                r &= !0x10;
            }
            let al = w.sim().rnd2(7) as u8;
            self.s1 = al as i8;
            if al >= 4 {
                r |= 0x4000;
            }
            self.s2 = r as u8;
            self.s3 = (r >> 8) as u8;
        }
        self.s0 = 8;
        let (s2, s3) = (self.s2, self.s3);
        let (sprite, vel);
        if s2 & 0x10 == 0 {
            let t = (s3 & 6) as usize;
            vel = w.get16(0x15A2 + t);
            sprite = 4 + 2 * t as u16;
        } else {
            let mut al = s3 & 0x3F;
            let mut ah = s3 & 0xC0;
            if ah & 0x40 != 0 {
                let v = w.get16(0);
                w.set16(0, v.rotate_left(1));
                if v & 0x8000 != 0 {
                    let cl: i8 = if ah & 0x80 != 0 { -10 } else { 10 };
                    let nv = (al as i8).wrapping_add(cl);
                    al = nv as u8;
                    if nv < 0 {
                        ah ^= 0x80;
                        al = 0;
                    }
                    if al >= 0x40 {
                        al = 0x3F;
                        ah ^= 0x80;
                    }
                    self.s3 = ah | al;
                }
            }
            let (dx, by) = dir(al.wrapping_add(0xE0));
            vel = ((by as u8 as u16) << 8) | dx as u8 as u16;
            sprite = 0x14;
        }
        let k = (s2 & 0x0C) as usize;
        let (x, y) = (w.get16(0x1592 + k) as i16 as i32, w.get16(0x1594 + k) as i16 as i32);
        self.spawn(sprite, x, y, vel);
    }

    /// seg000:0D0D.
    fn flash(&mut self, c: u8, pal: &mut Palette) {
        if c == 0x10 || c == 0x0A {
            let e = if c == 0x10 { 0x37 } else { 0x36 };
            if let Some(b) = &self.bank {
                for (k, col) in b.pal_entry(e, 0x54).iter().enumerate() {
                    pal.0[128 + k] = *col;
                }
            }
        } else {
            crate::transition::pal_step(pal, &self.target, 128, 28, c as i32);
        }
    }

    fn render(&mut self) {
        self.view.copy_from_slice(&self.clean);
        let Some(b) = &self.bank else { return };
        for o in &self.objs {
            if let Some(s) = b.sprite(o.sprite as usize) {
                s.draw(&mut self.view, 320, o.x0, o.y0, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Full-screen sequences (videos, the ending)

pub enum Movie {
    /// DEAD.HNM then the death screen; DEAD2.HNM then the restart.
    Dead { p: HnmPlayer, acc: u32, restart: bool },
    /// FINAL.HSQ pictures and the cast cards.
    Ending { step: usize, until: u64, fb: Vec<u8>, pal: Palette },
}

impl World {
    // ------------------------------------------------------------------ per slot (§2)

    /// sub_11B23 hook: the simulation for one slot (the clock has advanced).
    pub(crate) fn slot_sim(&mut self, assets: &mut Assets) {
        if self.mem[0x46D9] != 0 || self.mem[0x2A] >= 0xC8 {
            return;
        }
        let war_before = self.mem[0x2B];
        self.sim().eye_decay();
        let mut s = self.sim();
        s.slot();
        let icons = s.icons;
        if icons {
            self.icons_dirty = true;
        }
        if self.mem[0x46D9] != 0 {
            return;
        }
        if self.scr == Scr::Map && (self.mem[0x46EC] != 0 || self.icons_dirty) {
            self.mem[0x46EC] = 0;
            self.map_redraw(assets);
        }
        let room_view = self.scr == Scr::Room && self.indoors() && !self.in_transit() && !self.skipping;
        if self.mem[0x2B] != war_before {
            if self.mem[0x2B] != 0 {
                if room_view {
                    self.redraw_room(assets);
                }
            } else {
                self.attack = None;
                self.mem[0x473B] |= 1;
            }
        }
        if room_view && self.talk.is_none() {
            let f = self.mem[0x473B];
            if f & 0x80 != 0 {
                let p = self.place();
                self.enter(assets, p);
            } else if f != 0 {
                self.redraw_room(assets);
            }
        }
        self.mem[0x473B] = 0;
    }

    /// sub_1503C on arrival in a place (ds:114E set).
    pub(crate) fn hostility_check(&mut self) {
        let di = self.get16(0x114E) as usize;
        if di < 0x100 {
            return;
        }
        self.sim().hostility(di);
    }

    // ------------------------------------------------------------------ war mode (§5.2)

    /// Room redraw in war mode: the ATTACK scene replaces the room.
    pub(crate) fn war_view(&mut self, assets: &mut Assets, pal: &mut Palette) -> bool {
        if self.mem[0x2B] == 0 || !self.indoors() {
            self.attack = None;
            return false;
        }
        self.mem[0x11BC] |= 1;
        self.mem[0x46DF] = 0;
        let strip = self.mem[0x11DD] as u16;
        let sc = AttackScene::new(assets, strip, pal);
        self.view.copy_from_slice(&sc.clean);
        self.attack = Some(sc);
        self.pending_voc = Some("SN3.HSQ");
        true
    }

    pub(crate) fn attack_tick(&mut self) {
        if self.scr != Scr::Room || self.closeup.is_some() {
            return;
        }
        let Some(mut sc) = self.attack.take() else { return };
        let mut pal = self.palette.clone();
        if sc.tick(self, &mut pal) {
            self.palette = pal;
            self.dirty = true;
        }
        if self.attack.is_none() && self.mem[0x2B] != 0 {
            self.attack = Some(sc);
        }
    }

    /// seg000:7317: MASSIVE ATTACK.
    pub(crate) fn massive_attack(&mut self, assets: &mut Assets) {
        self.sim().massive_attack();
        // 20 flashes (ds:485D = 0x0B or 0x11), each followed by a wait of 40 or 42 ticks.
        self.flashes = 20;
        self.next_flash = self.now;
        self.massive_flash();
        let _ = assets;
    }

    /// One flash of the MASSIVE ATTACK loop; the last one runs loc_11B8D.
    fn massive_flash(&mut self) {
        let r = self.sim().rnd2(0x201);
        if let Some(sc) = &mut self.attack {
            sc.s7 = if r & 0xFF == 0 { 0x0B } else { 0x11 };
        }
        self.next_flash = self.now + 0x28 + (r >> 8) as u64;
        self.busy_until = self.next_flash;
    }

    /// Main-loop part of MASSIVE ATTACK.
    pub(crate) fn massive_tick(&mut self, assets: &mut Assets) {
        if self.flashes == 0 || self.now < self.next_flash {
            return;
        }
        self.flashes -= 1;
        if self.flashes > 0 {
            self.massive_flash();
            return;
        }
        self.mem[0x473A] = 0;
        self.after_slot_ui(assets);
    }

    /// seg000:0FC5: FIGHT FOR A WHOLE DAY.
    pub(crate) fn fight_day(&mut self, assets: &mut Assets) {
        for _ in 0..16 {
            crate::travel::advance_slots(self, assets, 1);
            if self.mem[0x2B] == 0 || self.mem[0x46D9] != 0 {
                break;
            }
        }
        self.effect = Some(0x2A);
        if self.mem[0x46D9] == 0 {
            self.redraw_room(assets);
        }
    }

    /// loc_11B8D: war re-check and redraw.
    fn after_slot_ui(&mut self, assets: &mut Assets) {
        let before = self.mem[0x2B];
        self.sim().war_recheck();
        if self.mem[0x46D9] != 0 {
            return;
        }
        if before != 0 && self.mem[0x2B] == 0 {
            self.attack = None;
            self.redraw_room(assets);
        }
        self.mem[0x473B] = 0;
    }

    // ------------------------------------------------------------------ game over (§5.5)

    /// sub_10D8E when ds:46D9 is set (main loop).
    pub(crate) fn game_over_check(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let code = self.mem[0x46D9];
        if code == 0 || code >= 0x80 || self.movie.is_some() || self.trans.is_some() {
            return;
        }
        if self.scr == Scr::Map {
            self.map_reset();
            self.mem[0xFB] = 1;
        }
        self.scr = Scr::Room;
        self.globe = None;
        self.dest = None;
        self.flight = None;
        self.seq = None;
        self.attack = None;
        self.talk = None;
        self.closeup = None;
        self.talk_menu_for = None;
        self.tui.menus.clear();
        self.set16(2, 0x16C5);
        self.mem[0x4727] = 0;
        self.mem[0x11C9] = 0;
        audio.stop();
        crate::intro::start_music(assets, audio, "WARSONG", 0);
        if let Some(h) = assets.hnm("DEAD.HNM") {
            self.movie = Some(Movie::Dead { p: HnmPlayer::new(h), acc: 66, restart: false });
        }
        self.mem[0x11CA] = 1;
        self.game_over = Some(code);
        self.mem[0x46D9] = 0x80;
        self.dirty = true;
    }

    /// The death screen once DEAD.HNM is over: text, face and the menu ds:20A2.
    fn death_screen(&mut self, assets: &mut Assets, audio: &mut Audio, last: &[u8], pal: &Palette) {
        let code = self.game_over.unwrap_or(7);
        self.view.copy_from_slice(&last[..VIEW]);
        // The panel keeps its colours (0xF0..0xFF are not in DEAD.HNM's palette).
        let mut p = pal.clone();
        for c in 0xF0..0x100 {
            p.0[c] = self.palette.0[c];
        }
        self.palette = p;
        self.attack = None;
        let tab = [0xF2u8, 0xFF, 0x0C, 0x05, 0x0A, 0xFF, 0x09, 0x0B];
        let c = tab.get(code as usize).copied().unwrap_or(0xFF);
        // Box ds:2254 = (0x14, 3)-(0x51, 0x40) (sub_17B1B), then the last portrait sprites
        // of the face bank (entries n - 3 and n - 2) at (x0 + 1, y0 + 1).
        let (x0, y0, x1, y1) = (0x14, 3, 0x51, 0x40);
        crate::troop_ui::frame(&mut self.view, (x0, y0, x1, y1), self.mem[0x2254 + 9], self.mem[0x2254 + 8]);
        if (c as i8) >= 0 {
            // The original clears the box with the bank's blank entries n-3 / n-2 and
            // starts the speaker's face in it (sub_19EFD); here frame 0 of the face,
            // 59x59 around its mouth rectangle (ds:27FA).
            let (idx, variant) = crate::face::bank_and_variant(c as usize, &self.mem);
            let mouth = assets.exe_words(0x27FA + 8 * idx, 4).unwrap_or(vec![40, 60, 60, 70]);
            if let Some(bank) = assets.resource_name(idx + 2).and_then(|n| assets.dat.read(&n).ok()).and_then(dune_formats::face::FaceBank::parse) {
                bank.sheet.apply_palette(&mut self.palette);
                let (fx, fy, _, _) = bank.rect;
                let params = crate::face::FaceParams { character: c as usize, variant, f0: 0, shift: 0, mouth_rect: None, words: 1, story_phase: self.mem[0x2A], seed: self.rng_seed() };
                let anim = crate::face::FaceAnim::new(bank, params, &vec![0u8; VIEW], 0);
                let cx = fx as i32 + (mouth[0] as i32 + mouth[2] as i32) / 2 - 29;
                let cy = (fy as i32 + mouth[3] as i32 - 50).max(0);
                for y in 0..59 {
                    for x in 0..59 {
                        let (sx, sy) = (cx + x, cy + y);
                        if (0..320).contains(&sx) && (0..152).contains(&sy) {
                            let v = anim.back[(sy * 320 + sx) as usize];
                            if v != 0 {
                                self.view[((y0 + 1 + y) * 320 + x0 + 1 + x) as usize] = v;
                            }
                        }
                    }
                }
            }
        }
        let t = self.cmd_t(0xBA + code as u16);
        let t = crate::talk::expand(&self.mem, assets, &t);
        let pages = t.split(|&b| b == 0xFE).map(|p| p.to_vec()).collect();
        self.talk = Some(crate::world::Talk { pages, page: 0, timer: 0.0, after: After::None });
        self.build_menu();
        let _ = audio;
        self.dirty = true;
    }

    /// RESTART GAME (0E47): DEAD2.HNM when a game over is pending, then the restart.
    pub(crate) fn restart(&mut self, assets: &mut Assets) -> bool {
        if self.game_over.is_some() {
            if let Some(h) = assets.hnm("DEAD2.HNM") {
                self.movie = Some(Movie::Dead { p: HnmPlayer::new(h), acc: 66, restart: true });
                self.talk = None;
                return true;
            }
        }
        false
    }

    /// Video / ending tick.
    pub(crate) fn movie_tick(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let Some(mut m) = self.movie.take() else { return };
        match &mut m {
            Movie::Dead { p, acc, restart } => {
                *acc += 1;
                if *acc >= 66 {
                    *acc = 0;
                    if p.done() {
                        let (fb, pal, restart) = (p.fb.clone(), p.palette.clone(), *restart);
                        if restart {
                            self.game_over = None;
                            self.mem[0x46D9] = 0;
                            self.set16(0x47C4, 0);
                            if !self.load_slot(assets, 0) {
                                self.say(b"No restart save (dune37s0.sav).");
                            }
                        } else {
                            self.death_screen(assets, audio, &fb, &pal);
                        }
                        self.dirty = true;
                        return;
                    }
                    p.step();
                    self.dirty = true;
                }
            }
            Movie::Ending { .. } => {
                if !self.ending_tick(assets, audio, &mut m) {
                    return;
                }
            }
        }
        self.movie = Some(m);
    }

    pub(crate) fn movie_frame(&self) -> Option<(Vec<u8>, Palette)> {
        match self.movie.as_ref()? {
            Movie::Dead { p, .. } => Some((p.fb.clone(), p.palette.clone())),
            Movie::Ending { fb, pal, .. } => Some((fb.clone(), pal.clone())),
        }
    }

    // ------------------------------------------------------------------ reports (§7.1)

    /// sub_12B2A (every main-loop pass in a room).
    pub(crate) fn events_tick(&mut self, assets: &mut Assets, audio: &mut Audio) {
        if self.mem[0x46D9] != 0 || self.mem[0x4774] != 0 || self.mem[0x11C9] != 0 || self.scr != Scr::Room {
            return;
        }
        if self.talk.is_some() || self.talk_menu_for.is_some() || self.view_mode != crate::world::View::Room || (self.mem[0xFB] as i8) < 0 || self.trans.is_some() || self.seq.is_some() {
            return;
        }
        let ph = self.mem[0x2A];
        if self.ending_pending {
            self.ending_pending = false;
            return self.ending_next(assets, audio);
        }
        if ph < 0x14 || ph >= 0xC8 {
            return;
        }
        let idle = self.now.saturating_sub(self.idle_since);
        if ph == 0x14 {
            // The spice demands start: outdoors, alone, idle for 1000 ticks.
            if self.get16(0x10) == 0 && self.mem[8] == 0xFF && idle >= 0x3E8 {
                self.idle_since += 0x3B6;
                self.sim().demands_start();
                self.build_menu();
            }
            return;
        }
        if std::mem::take(&mut self.ending_pending) {
            return self.ending_next(assets, audio);
        }
        if self.mem[0x1190] == 0 || idle < 0x32 {
            return;
        }
        let ax = self.get16(0x1191);
        let di = self.get16(0x1193);
        let speaker = (ax >> 8) as usize;
        // sub_12AD8: the speaker is present.
        let present = speaker < 16 && (self.get16(0x12) >> speaker) & 1 != 0 && (speaker != 0x0F || di == self.get16(0x114E));
        if present {
            self.mem[0x23] = 0;
            return self.tell_event(assets, audio, ax, di, false);
        }
        if self.mem[0x2B] == 0 {
            let t = idle;
            if t < 0x96 {
                if self.pulse != Some(0x28) {
                    self.pulse = Some(0x28);
                    self.effect = Some(0x28);
                    self.dirty = true;
                }
                return;
            }
            if t < 0xFA {
                return;
            }
            if t < 0x15E {
                if self.pulse != Some(0x26) {
                    self.pulse = Some(0x26);
                    self.effect = Some(0x26);
                    self.dirty = true;
                }
                return;
            }
            if t < 0x1C2 {
                return;
            }
        }
        self.pulse = None;
        self.tell_event(assets, audio, ax, di, true);
    }

    /// loc_12BD2 / loc_13542: the event told by its speaker (vision when absent).
    fn tell_event(&mut self, assets: &mut Assets, audio: &mut Audio, ax: u16, di: u16, vision: bool) {
        let mut speaker = (ax >> 8) as usize;
        if speaker >= 0x10 {
            self.sim().pop_event();
            return;
        }
        let save = self.get16(0x11CE);
        if di != 0 {
            self.sim().analyse(di as usize);
            self.sim().place_name_vars(di as usize);
        }
        self.mem[0xEA] = ax as u8;
        if speaker == 0x0F || speaker == 0x0E {
            let p = di as usize;
            let si = if (ax as u8) == 0x0E { crate::sim::PROSPECTORS } else if p >= 0x100 { crate::sim::troop_addr(self.mem[p + 9].max(1)) } else { crate::sim::PROSPECTORS };
            self.set16(0x4756, si as u16);
            self.set16(0x4758, si as u16);
            self.mem[0x476C] = 0;
            speaker = 0x0F;
            self.sim().troop_vars(si);
        }
        if vision {
            self.vision = Some(speaker);
            self.idle_since = self.now;
        }
        self.set16(0x47C4, 0x10);
        // Character 16, sub-list 4 with ds:EA = event; the face is the speaker's.
        self.set16(0x47A2, 0x0FD8 + 16 * speaker as u16);
        let save_mask = self.mem[0x47C2];
        self.mem[0x47C2] = 0x20;
        let r = self.sublist_start(0x84).and_then(|p| self.pick_from(p));
        self.mem[0x47C2] = save_mask;
        self.set16(0x47C4, speaker as u16);
        match r {
            Some(r) => {
                let raw = assets.phrases(2).map(|t| t.get(r.phrase as usize - 1).to_vec()).unwrap_or_default();
                let text = crate::talk::expand(&self.mem, assets, &raw);
                if vision {
                    self.vision_screen(assets);
                }
                self.show_line(assets, audio, speaker, r.phrase, &text, After::Event);
            }
            None => self.sim().pop_event(),
        }
        if save >= 0x100 {
            self.sim().analyse(save as usize);
        }
    }

    /// VIS.HSQ full view behind the speaker's face.
    fn vision_screen(&mut self, assets: &mut Assets) {
        let mut v = vec![0u8; VIEW];
        let mut pal = self.palette.clone();
        if let Some(s) = assets.sheet("VIS.HSQ") {
            s.apply_palette(&mut pal);
            if let Some(sp) = s.sprites.first() {
                sp.draw(&mut v, 320, 0, 0, DrawOpts::default());
            }
        }
        self.view.copy_from_slice(&v);
        self.palette = pal;
        self.effect = Some(6);
        self.dirty = true;
    }

    // ------------------------------------------------------------------ comm room (§7.2)

    /// Lines of the communications room (Arrakeen palace room 8, ds:C8 != 0).
    pub(crate) fn comm_lines(&self) -> Vec<Line> {
        let mut v = vec![];
        if (self.place() >> 8) == 0x20 && self.get16(6) >> 8 == 1 && self.place() & 0xFF == 8 && self.mem[0xC8] != 0 {
            v.push(Line::new(0xD7, Cmd::ViewMessages(true), self.mem[0xC9] != 0));
            v.push(Line::new(0xD8, Cmd::ViewMessages(false), self.mem[0xC9] < self.mem[0xC8]));
        }
        v
    }

    /// VIEW NEW MESSAGES / Messages already seen: the list of senders, newest first.
    pub(crate) fn view_messages(&mut self, assets: &mut Assets, audio: &mut Audio, new: bool) {
        play_narrator(assets, audio, if new { 0x2A } else { 0x2B });
        let n = self.mem[0xC8] as usize;
        let mut lines = vec![];
        for k in (0..n).rev() {
            let w = self.get16(0x1179 + 2 * k);
            let seen = w & 0x80 != 0;
            if seen == new {
                continue;
            }
            let sender = (w & 0x7F) as u16;
            lines.push(Line::new(0x78 + sender, Cmd::Message(k), true));
        }
        lines.truncate(4);
        lines.push(Line::new(0xA3, Cmd::MessagesCancel, true));
        self.msg_menu = Some(lines);
        self.build_menu();
    }

    /// sub_128E1: a message is viewed.
    pub(crate) fn view_message(&mut self, assets: &mut Assets, audio: &mut Audio, k: usize) {
        self.msg_menu = None;
        let w = self.get16(0x1179 + 2 * k);
        let sender = (w & 0x7F) as usize;
        let param = (w >> 8) as u8;
        self.mem[0x24] = param;
        if w & 0x80 == 0 {
            self.set16(0x1179 + 2 * k, w | 0x80);
            self.mem[0xC9] = self.mem[0xC9].saturating_sub(1);
        }
        if sender == 0x0B && (param == 2 || param == 3) {
            self.mem[0xBF] |= 0x20;
        }
        play_narrator(assets, audio, 0x1A + sender as u16);
        // sub_196F1: the sender's sub-list 4 (lines keyed on ds:24).
        self.talk_select(sender);
        self.talk_sub(assets, audio, sender, 4, After::None);
        if sender == 0x0B && param == 0x0C {
            self.mem[0x46D9] = 7;
        }
        self.build_menu();
    }

    /// sub_135AD: the shipment leaves from the communications room with Idaho present.
    pub(crate) fn shipment_check(&mut self) {
        if self.get16(0x1158) == 0 || (self.place() >> 8) != 0x20 || self.mem[0x0B] != 8 || self.get16(0x12) & 8 == 0 {
            return;
        }
        let amount = self.get16(0xC0) & self.get16(0x1158);
        self.sim().ship(amount);
        self.say(b"");
        self.message = None;
    }

    // ------------------------------------------------------------------ ending (§6.3)

    /// sub_14057: entering the Harkonnen palace room 2 after the shield fell.
    pub(crate) fn ending_check(&mut self, assets: &mut Assets) -> bool {
        if self.place() != 0x3002 || self.mem[0xC2] < 7 || self.mem[0x2A] >= 0xC8 {
            return false;
        }
        // loc_116FC: ds:2A = 0xC8, script 128F in this room. Speech order of the script.
        self.mem[0x2A] = 0xC8;
        self.ending_queue = vec![4, 0x0B, 3, 2, 0x0B, 2, 5, 9, 7, 0x0A, 5, 0x0B, 2, 1];
        let (p, w) = (self.place(), self.get16(6));
        for &c in &self.ending_queue.clone() {
            self.set16(0x0FD8 + 16 * c, p);
            self.set16(0x0FD8 + 16 * c + 2, w);
        }
        self.set16(0x10, 0);
        self.redraw_room(assets);
        self.ending_pending = true;
        true
    }

    /// The script's next speaker (op 02: sub_19761, the next line of sub-list 7).
    pub(crate) fn ending_next(&mut self, assets: &mut Assets, audio: &mut Audio) {
        if self.ending_queue.is_empty() {
            return self.ending_final(assets);
        }
        let c = self.ending_queue.remove(0);
        self.set16(0x47C4, c as u16);
        self.set16(0x47A2, 0x0FD8 + 16 * c as u16);
        let save = self.mem[0x47C2];
        self.mem[0x47C2] = 0x80;
        let r = self.sublist_start((c << 3) | 7).and_then(|p| self.pick_from(p));
        self.mem[0x47C2] = save;
        match r {
            Some(r) => {
                self.say_record(assets, audio, r, After::EndingNext);
            }
            None => self.ending_next(assets, audio),
        }
    }

    /// op 14: FINAL.HSQ.
    fn ending_final(&mut self, assets: &mut Assets) {
        let mut fb = vec![0u8; 320 * 200];
        let mut pal = self.palette.clone();
        if let Some(f) = assets.sheet("FINAL.HSQ") {
            f.apply_palette(&mut pal);
            if let Some(s) = f.sprites.get(3) {
                s.draw(&mut fb, 320, 0, 0x34, DrawOpts::default());
            }
        }
        self.movie = Some(Movie::Ending { step: 0, until: self.now + 300, fb, pal });
        self.dirty = true;
    }

    /// op 14 (FINAL.HSQ) then op 16 (the cast cards, 600 ticks each), then exit.
    fn ending_tick(&mut self, assets: &mut Assets, audio: &mut Audio, m: &mut Movie) -> bool {
        let Movie::Ending { step, until, fb, pal } = m else { return true };
        if self.now < *until && !self.ending_skip {
            return true;
        }
        self.ending_skip = false;
        *step += 1;
        self.dirty = true;
        let fin = assets.sheet("FINAL.HSQ");
        match *step {
            1 => {
                if let Some(f) = &fin {
                    if let Some(s) = f.sprites.get(4) {
                        s.draw(fb, 320, 0x5A, 0x40, DrawOpts::default());
                    }
                }
                *until = self.now + 300;
            }
            2 => {
                audio.stop();
                crate::intro::start_music(assets, audio, "WORMSUIT", 0);
                fb.fill(0);
                if let Some(f) = &fin {
                    if let Some(s) = f.sprites.get(5) {
                        s.draw(fb, 320, 0x40, 0x34, DrawOpts::default());
                    }
                }
                self.ending_caption(assets, fb, 0);
                *until = self.now + 600;
            }
            3..=16 => {
                let i = *step - 2;
                fb.fill(0);
                // Cast cards: the character over its room (simplified: the face).
                let cast: [usize; 16] = [0xFF, 0x11, 0, 1, 4, 3, 0x0B, 8, 2, 5, 9, 0x0A, 7, 6, 0xFF, 0xFF];
                if i == 15 {
                    if let Some(f) = &fin {
                        for k in 0..3 {
                            if let Some(s) = f.sprites.get(k) {
                                s.draw(fb, 320, 0, 0, DrawOpts::default());
                            }
                        }
                        f.apply_palette(pal);
                    }
                } else if cast[i] != 0xFF {
                    let c = cast[i];
                    let (idx, variant) = crate::face::bank_and_variant(c, &self.mem);
                    if let Some(bank) = assets.resource_name(idx + 2).and_then(|n| assets.dat.read(&n).ok()).and_then(dune_formats::face::FaceBank::parse) {
                        bank.sheet.apply_palette(pal);
                        let params = crate::face::FaceParams { character: c, variant, f0: 0, shift: 0, mouth_rect: None, words: 1, story_phase: 0xC8, seed: 1 };
                        let anim = crate::face::FaceAnim::new(bank, params, &vec![0u8; VIEW], 0);
                        fb[..VIEW].copy_from_slice(&anim.back);
                    }
                }
                self.ending_caption(assets, fb, i);
                *until = self.now + 600;
            }
            _ => {
                if self.now < *until + 0x7FFF && !self.ending_done {
                    self.ending_done = true;
                    *until = u64::MAX;
                    return true;
                }
                self.quit = true;
                return false;
            }
        }
        true
    }

    fn ending_caption(&mut self, assets: &mut Assets, fb: &mut [u8], i: usize) {
        let t = self.cmd_t(0x121 + i as u16);
        crate::world::text(fb, &assets.font, 10, 0x9D, &t, 0xFF, 0);
    }

}

/// sub_1AB4F: narrator line PZ0xx.
fn play_narrator(assets: &mut Assets, audio: &mut Audio, n: u16) {
    audio.stop();
    crate::intro::play_voc(assets, audio, &format!("PZ\\PZ{:03X}I.VOC", n));
}
