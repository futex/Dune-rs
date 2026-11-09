//! The globe screen (seg000:B8C6 entry, B827 layout, task B9AE, menu ds:204A, results
//! B8F3/BE1D/BE57). docs/re/globe.md §5-§7.
//!
//! Keyboard equivalents: Up/Down = tilt arrows (zones 13/15), Left = zone 16 (rotate +32),
//! Right = zone 14 (rotate -32), `c` = centre on Paul (zone 17), `x` = EXIT GLOBE
//! (zone 12), Enter = click the centre of the disc (160,79) -> map of that area,
//! `m` = toggle the SEE MAP OF THIS AREA hover menu as if the mouse entered the disc.

use crate::{
    assets::Assets,
    input::Key,
    panel,
    world::{fill, rect, text, Cmd, Line, Scr, World, VIEW},
};
use dune_formats::{globe::Globe, palette::Palette, sprite::DrawOpts};

/// Pacing of the CPU-bound loops (fresco slide, auto-centre, zoom box) [L]: one step per
/// displayed retrace, i.e. 3 ticks (globe.md §9).
const STEP: u64 = 3;

#[derive(Clone, Copy, PartialEq)]
pub enum GAnim {
    /// B8F3: frescos slide out (SEE RESULTS).
    SlideOut,
    /// B915: frescos slide back (STANDARD VISION).
    SlideIn,
    /// BA9E auto-centre; `exit` = then EXIT GLOBE.
    Centre { exit: bool },
    /// BC99 zoom box towards rect ds:2440, then the map centred on (lon, lat).
    Zoom { r: (i32, i32, i32, i32), step: u8, lon: u16, lat: i16 },
}

pub struct GlobeState {
    pub g: Globe,
    /// Back buffer (320x200): the progressive render target.
    pub back: Vec<u8>,
    /// What is on screen in the view area.
    pub screen: Vec<u8>,
    /// ds:DBDE clean copy of the view (results slide).
    pub clean: Vec<u8>,
    pub pal: Palette,
    /// ds:DD0F fresco slide offset.
    pub dd0f: i32,
    /// Menu ds:2062 (SEE MAP OF THIS AREA) is up.
    pub hover_menu: bool,
    pub bars: bool,
    pub cur: [u8; 6],
    pub target: [u8; 6],
    pub trend: [u8; 6],
    pub bar_acc: u32,
    pub anim: Option<GAnim>,
    pub next: u64,
}

impl World {
    /// Zone 0 handler seg000:B8C6.
    pub(crate) fn globe_enter(&mut self, assets: &mut Assets) {
        if self.scr != Scr::Map {
            return;
        }
        let (Ok(tab), Ok(gd)) = (assets.dat.read("TABLAT.BIN"), assets.dat.read("GLOBDATA.HSQ")) else { return };
        // B8A7: GLOBDATA -> ds:4C60 (the map renderers' scratch), set_view(197C, 197E).
        let n = gd.len().min(0x3EDB);
        self.mem[0x4C60..0x4C60 + n].copy_from_slice(&gd[..n]);
        let Some(mut g) = Globe::new(&tab, gd) else { return };
        g.set_view(self.get16(0x197C), self.get16(0x197E) as i16);
        self.map_reset();
        let mut pal = self.mapst.pal.clone();
        if let Some(f) = assets.sheet("FRESK.HSQ") {
            f.apply_palette(&mut pal);
        }
        self.mem[0xDD03] = self.mem[0xDD03].wrapping_add(1);
        self.mem[0xDD02] = 0;
        let mut st = GlobeState {
            g,
            back: vec![0; 320 * 200],
            screen: vec![0; VIEW],
            clean: vec![0; VIEW],
            pal,
            dd0f: 0,
            hover_menu: false,
            bars: false,
            cur: [0; 6],
            target: [0; 6],
            trend: [3; 6],
            bar_acc: 0,
            anim: None,
            next: 0,
        };
        // B827: fill F0 + ring + full render, frescos, ornaments, menu, panels.
        globe_base(assets, &mut st, &self.map);
        frescos(assets, &mut st.back, 0);
        st.screen.copy_from_slice(&st.back[..VIEW]);
        self.globe = Some(st);
        self.scr = Scr::Globe;
        let mem = self.mem.clone();
        self.panel.set_left(&mem, panel::LEFT_GLOBE);
        self.panel.set_compass(&mem, panel::COMPASS_GLOBE);
        self.build_menu();
        self.effect = Some(0);
        self.medallion_to = 10;
        self.dirty = true;
    }

    pub(crate) fn globe_close(&mut self) {
        self.globe = None;
        self.mem[0xDD03] = 0;
    }

    /// B941: the globe menu ds:204A (or the hover menu ds:2062).
    pub(crate) fn globe_menu(&mut self) {
        let Some(st) = &self.globe else { return };
        if st.hover_menu {
            let mut l = Line::new(0xB6, Cmd::SeeMapArea, true);
            l.hl = true;
            self.menu.push(l);
            return;
        }
        self.menu.push(Line::new(0xB7, Cmd::ExitGlobe, true));
        if self.mem[0xDD02] == 0 {
            self.menu.push(Line::new(0xB1, Cmd::SeeResults, true));
        } else {
            self.menu.push(Line::new(0xB2, Cmd::StandardVision, true));
        }
        self.menu.push(Line::new(0xB4, Cmd::LoadMenu, true));
        self.menu.push(Line::new(0xB3, Cmd::SaveMenu, true));
        self.menu.push(Line::new(0xBB, Cmd::ExitGame, true));
    }

    /// sub_1BAF2 + BC0C: Paul's marker (ICONES 0x36) on the screen.
    fn globe_marker(&mut self, assets: &mut Assets) {
        let (lon, row) = self.cur_pos();
        let Some(st) = &mut self.globe else { return };
        if self.mem[0x227D] != 0 {
            return;
        }
        let Some((x, y)) = st.g.marker(lon, row) else { return };
        if let Some(ic) = assets.sheet("ICONES.HSQ") {
            if let Some(s) = ic.sprites.get(0x36) {
                let yy = (y as u8).wrapping_sub(s.height as u8) as i32;
                s.draw(&mut st.screen, 320, x, yy, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
        }
    }

    /// B98E: copy rect (96,15)-(234,134) back -> screen, marker, rotate(+1).
    fn globe_frame_done(&mut self, assets: &mut Assets) {
        let Some(st) = &mut self.globe else { return };
        for y in 15..134 {
            let o = y * 320;
            st.screen[o + 96..o + 234].copy_from_slice(&st.back[o + 96..o + 234]);
        }
        self.globe_marker(assets);
        if let Some(st) = &mut self.globe {
            st.g.rotate(1);
        }
        self.dirty = true;
    }

    /// B98B: full render, then B98E.
    fn globe_full(&mut self, assets: &mut Assets) {
        let Some(st) = &mut self.globe else { return };
        st.g.results = self.mem[0xDD02] != 0;
        st.g.render(&mut st.back, &self.map);
        st.g.restart();
        self.globe_frame_done(assets);
    }

    pub(crate) fn globe_tick(&mut self, assets: &mut Assets) {
        let Some(st) = &mut self.globe else { return };
        if let Some(a) = st.anim {
            if self.now >= st.next {
                st.next = self.now + STEP;
                self.globe_anim_step(assets, a);
            }
            return;
        }
        // Task B9AE (period 1): one globe line per tick.
        st.g.results = self.mem[0xDD02] != 0;
        if st.g.render_line(&mut st.back, &self.map) {
            self.globe_frame_done(assets);
        }
        // Task BE57 (period 12): results bar graphs.
        if let Some(st) = &mut self.globe {
            if st.bars {
                st.bar_acc += 1;
                if st.bar_acc >= 12 {
                    st.bar_acc = 0;
                    bars_step(assets, st);
                    self.dirty = true;
                }
            }
        }
    }

    fn globe_anim_step(&mut self, assets: &mut Assets, a: GAnim) {
        match a {
            GAnim::SlideOut => {
                let Some(st) = &mut self.globe else { return };
                st.screen.copy_from_slice(&st.clean);
                frescos(assets, &mut st.screen, st.dd0f);
                st.dd0f -= 16;
                if st.dd0f <= -0x6A {
                    st.anim = None;
                    self.mem[0xDD02] = 0xFF;
                    self.globe_full(assets);
                    self.build_menu();
                }
                self.dirty = true;
            }
            GAnim::SlideIn => {
                let Some(st) = &mut self.globe else { return };
                st.dd0f += 16;
                frescos(assets, &mut st.screen, st.dd0f);
                if st.dd0f == 0 {
                    st.anim = None;
                    // Frescos into the back buffer, standard colours.
                    frescos(assets, &mut st.back, 0);
                    self.mem[0xDD02] = 0;
                    self.globe_full(assets);
                    self.build_menu();
                }
                self.dirty = true;
            }
            GAnim::Centre { exit } => {
                let (lon, row) = self.cur_pos();
                let Some(st) = &mut self.globe else { return };
                let tl = ((398u32 * lon as u32) >> 16) as i32;
                let d = tl - st.g.longitude();
                let p = d.rem_euclid(398);
                let n = p - 398;
                let mut d = if -n < p { n } else { p };
                let mut cnt = 1;
                if d >= 32 {
                    d = 32;
                    cnt += 1;
                }
                if d <= -32 {
                    d = -32;
                    cnt += 1;
                }
                let mut e = row as i32 - st.g.tilt;
                if e >= 24 {
                    e = 24;
                    cnt += 1;
                }
                if e <= -24 {
                    e = -24;
                    cnt += 1;
                }
                st.g.rotate(d);
                st.g.add_tilt(e);
                self.globe_full(assets);
                cnt -= 1;
                if cnt == 0 {
                    let st = self.globe.as_mut().unwrap();
                    st.anim = None;
                    if exit {
                        // BC81: centre = Paul, zoom from (160,79), then the map.
                        self.globe_zoom(160, 79, lon, row);
                    }
                }
            }
            GAnim::Zoom { r, step, lon, lat } => {
                let Some(st) = &mut self.globe else { return };
                let (mut x0, mut y0, mut x1, mut y1) = r;
                x0 = (x0 - 4).max(96);
                y0 = (y0 - 2).max(15);
                x1 = (x1 + 4).min(234);
                y1 = (y1 + 2).min(134);
                // Outline in colour 7 on the screen, then the inside back -> screen.
                let mut fb = vec![0u8; 320 * 200];
                fb[..VIEW].copy_from_slice(&st.screen);
                rect(&mut fb, x0, y0, x1 - 1, y1 - 1, 7);
                for y in (y0 + 1).max(0)..(y1 - 1).min(152) {
                    for x in (x0 + 1).max(0)..(x1 - 1).min(320) {
                        let p = (y * 320 + x) as usize;
                        fb[p] = st.back[p];
                    }
                }
                st.screen.copy_from_slice(&fb[..VIEW]);
                self.dirty = true;
                if step + 1 >= 8 {
                    st.anim = None;
                    // sub_1ADBE + loc_15A3D: the flat map centred there, effect 2.
                    self.globe_close();
                    self.mem[0x46EB] = 0;
                    self.map_enter(assets, None, Some((lon, lat)));
                } else {
                    st.anim = Some(GAnim::Zoom { r: (x0, y0, x1, y1), step: step + 1, lon, lat });
                }
            }
        }
    }

    fn globe_zoom(&mut self, x: i32, y: i32, lon: u16, lat: i16) {
        self.set16(0x197C, lon);
        self.set16(0x197E, lat as u16);
        if let Some(st) = &mut self.globe {
            st.anim = Some(GAnim::Zoom { r: (x, y, x + 2, y + 1), step: 0, lon, lat });
            st.next = self.now + STEP;
        }
    }

    /// BC81: EXIT GLOBE.
    pub(crate) fn globe_exit(&mut self, _assets: &mut Assets) {
        if let Some(st) = &mut self.globe {
            if st.anim.is_none() {
                st.anim = Some(GAnim::Centre { exit: true });
                st.next = self.now;
            }
        }
    }

    /// Zones 13..17 of preset ds:1DC6.
    pub(crate) fn globe_zone(&mut self, assets: &mut Assets, h: u16) {
        let Some(st) = &mut self.globe else { return };
        if st.anim.is_some() {
            return;
        }
        match h {
            0xB9B9 => st.g.add_tilt(8),
            0xB9C0 => st.g.add_tilt(-8),
            0xB9CC => st.g.rotate(-32),
            0xB9D3 => st.g.rotate(32),
            _ => {
                st.anim = Some(GAnim::Centre { exit: false });
                st.next = self.now;
                return;
            }
        }
        self.globe_full(assets);
    }

    /// SEE RESULTS (B96B) / STANDARD VISION (B961).
    pub(crate) fn globe_results(&mut self, assets: &mut Assets, on: bool) {
        let mut st = match self.globe.take() {
            Some(s) if s.anim.is_none() => s,
            other => {
                self.globe = other;
                return;
            }
        };
        if on {
            // B8F3: B84A into the back buffer, then the results background (BE1D).
            st.g.results = false;
            globe_base(assets, &mut st, &self.map);
            self.results_background(assets, &mut st);
            st.clean.copy_from_slice(&st.back[..VIEW]);
            st.dd0f = 0;
            st.anim = Some(GAnim::SlideOut);
        } else {
            st.bars = false;
            st.anim = Some(GAnim::SlideIn);
        }
        st.next = self.now;
        self.globe = Some(st);
    }

    /// BE1D + BED7 + BDFA: legend, statistics, texts, bar targets (task BE57 starts).
    fn results_background(&mut self, assets: &mut Assets, st: &mut GlobeState) {
        if let Some(f) = assets.sheet("FRESK.HSQ") {
            for (spr, x, y) in [(3usize, 11, 124), (4, 11, 136)] {
                if let Some(s) = f.sprites.get(spr) {
                    s.draw(&mut st.back, 320, x, y, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
                }
            }
        }
        // BED7: ds:115C = today, then sub_1C02E (statistics) and sub_1BF26 (trends).
        let today = self.clock() & 0xFFF0;
        self.set16(0x115C, today);
        self.sim().stats();
        self.sim().trends();
        let vals = [self.get16(0xA4), self.get16(0xA2), self.get16(0xA8), self.get16(0xA6), self.get16(0xAC), self.get16(0xAA)];
        st.target = [
            ((vals[0] >> 1) + 1) as u8,
            ((vals[1] >> 1) + 1) as u8,
            ((vals[2] >> 4) as u8).wrapping_add(1),
            ((vals[3] >> 4) as u8).wrapping_add(1),
            ((vals[4] >> 8) as u8).wrapping_add(1),
            ((vals[5] >> 8) as u8).wrapping_add(1),
        ];
        for k in 0..6 {
            st.trend[k] = self.mem[0x116A + k];
        }
        st.cur = [0; 6];
        // Texts: lists ds:2482 (big font) and ds:2494 (small font), {id, x, y, colours}.
        let day = ((self.clock() as u32 + 3) >> 4) + 1;
        let fmt3 = |s: &mut Vec<u8>, n: u32| {
            if let Some(end) = s.iter().position(|b| b.is_ascii_digit()).map(|p| {
                let mut e = p;
                while e < s.len() && s[e].is_ascii_digit() {
                    e += 1;
                }
                e
            }) {
                if end >= 3 {
                    let n = n.min(999);
                    let h = if n >= 100 { b'0' + (n / 100) as u8 } else { b' ' };
                    let t = if n >= 10 { b'0' + (n / 10 % 10) as u8 } else { b' ' };
                    s[end - 3] = h;
                    s[end - 2] = t;
                    s[end - 1] = b'0' + (n % 10) as u8;
                }
                end
            } else {
                0
            }
        };
        let fmt5 = |s: &mut Vec<u8>, n: u32| {
            if let Some(p) = s.iter().position(|b| b.is_ascii_digit()) {
                let mut e = p;
                while e < s.len() && s[e].is_ascii_digit() {
                    e += 1;
                }
                let at = e - 1;
                s[at] = b'0';
                if at >= 5 {
                    let f = format!("{:>5}", n.min(99999));
                    s[at - 5..at].copy_from_slice(f.as_bytes());
                }
            }
        };
        let lang = (assets.lang.max(1) - 1) as usize;
        let suffix = assets.exe_bytes(0x251A + 8 * lang.min(7), 8).map(|b| b.to_vec()).unwrap_or(b"thstndrd".to_vec());
        let mut texts: Vec<(u16, Vec<u8>)> = vec![];
        let mut c2 = World::cmd_text(assets, 0xC2);
        let end = fmt3(&mut c2, day);
        if end >= 2 && end + 1 < c2.len() {
            let tens = c2[end - 2];
            let units = (c2[end - 1] & 0x0F) as usize;
            let k = if tens == b'1' || units >= 4 { 0 } else if units == 1 && lang == 1 && tens != b' ' { 0 } else { units };
            c2[end] = suffix[2 * k];
            c2[end + 1] = suffix[2 * k + 1];
        }
        texts.push((0xC2, c2));
        let mut c3 = World::cmd_text(assets, 0xC3);
        fmt3(&mut c3, (self.mem[0x29] >> 1) as u32);
        texts.push((0xC3, c3));
        for (id, v, five) in [(0xC4u16, vals[0], false), (0xC5, vals[1], false), (0xC7, vals[2], true), (0xC8, vals[3], true), (0xCA, vals[4], true), (0xCB, vals[5], true)] {
            let mut s = World::cmd_text(assets, id);
            if five {
                fmt5(&mut s, v as u32);
            } else {
                fmt3(&mut s, v as u32);
            }
            texts.push((id, s));
        }
        let get = |id: u16| texts.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_else(|| World::cmd_text(assets, id));
        for (list, big) in [(0x2482usize, true), (0x2494, false)] {
            let mut a = list;
            for _ in 0..16 {
                let id = self.get16(a);
                if id == 0xFFFF {
                    break;
                }
                let (x, y, c) = (self.get16(a + 2) as i32, self.get16(a + 4) as i32, self.get16(a + 6));
                let font = if big { &assets.font } else { &assets.font2 };
                text(&mut st.back, font, x, y, &get(id), c as u8, (c >> 8) as u8);
                a += 8;
            }
        }
        st.bars = true;
        st.bar_acc = 0;
        // The task's first run happens at once (BE57 follows the registration).
        bars_step(assets, st);
        st.clean.copy_from_slice(&st.back[..VIEW]);
    }

    pub(crate) fn globe_compose(&mut self, _assets: &mut Assets, fb: &mut [u8]) -> Palette {
        let Some(st) = &self.globe else { return self.palette.clone() };
        fb[..VIEW].copy_from_slice(&st.screen);
        st.pal.clone()
    }

    pub(crate) fn globe_mouse(&mut self, assets: &mut Assets, x: i32, y: i32, clicked: bool) {
        let Some(st) = &mut self.globe else { return };
        if st.anim.is_some() {
            return;
        }
        // BC1F: hover menu while inside the globe rect.
        let inside = (0..0x80).contains(&(x - 0x60)) && (0..0x6D).contains(&(y - 0x19));
        if inside != st.hover_menu && self.view_mode == crate::world::View::Room {
            st.hover_menu = inside;
            self.build_menu();
            self.dirty = true;
        }
        if clicked && inside {
            self.globe_click(assets, x, y);
        }
    }

    /// BC64: a click on the disc -> map of this area.
    fn globe_click(&mut self, assets: &mut Assets, x: i32, y: i32) {
        let Some(st) = &mut self.globe else { return };
        let Some((lon, lat)) = st.g.pick(x, y) else { return };
        self.set16(0x197C, lon);
        self.set16(0x197E, lat as u16);
        self.globe_full(assets);
        self.globe_zoom(x, y, lon, lat);
    }

    pub(crate) fn globe_key(&mut self, assets: &mut Assets, k: Key) {
        match k {
            Key::Up => self.globe_zone(assets, 0xB9B9),
            Key::Down => self.globe_zone(assets, 0xB9C0),
            Key::Right => self.globe_zone(assets, 0xB9CC),
            Key::Left => self.globe_zone(assets, 0xB9D3),
            Key::Char('c') => self.globe_zone(assets, 0xBA9E),
            Key::Char('x') => self.globe_exit(assets),
            Key::Enter => self.globe_click(assets, 160, 79),
            Key::Char('m') => {
                if let Some(st) = &mut self.globe {
                    st.hover_menu = !st.hover_menu;
                    self.build_menu();
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }
}

/// B84A: fill the view with F0, the ring (FRESK 2 at (91,20)), full render.
fn globe_base(assets: &mut Assets, st: &mut GlobeState, map: &[u8]) {
    fill(&mut st.back, 0, 0, 320, 152, 0xF0);
    if let Some(f) = assets.sheet("FRESK.HSQ") {
        if let Some(s) = f.sprites.get(2) {
            s.draw(&mut st.back, 320, 91, 20, DrawOpts::default());
        }
    }
    st.g.render(&mut st.back, map);
    st.g.restart();
}

/// B87E: the two frescos at +-DD0F, clipped to the view.
fn frescos(assets: &mut Assets, fb: &mut [u8], dd0f: i32) {
    if let Some(f) = assets.sheet("FRESK.HSQ") {
        let clip = Some((0, 0, 320, 152));
        if let Some(s) = f.sprites.get(0) {
            s.draw(fb, 320, dd0f, 0, DrawOpts { clip, ..Default::default() });
        }
        if let Some(s) = f.sprites.get(1) {
            s.draw(fb, 320, 214 - dd0f, 0, DrawOpts { clip, ..Default::default() });
        }
    }
}

/// Task BE57: each bar steps one pixel toward its target, drawn on the screen.
fn bars_step(assets: &mut Assets, st: &mut GlobeState) {
    let Some(ic) = assets.sheet("ICONES.HSQ") else { return };
    const POS: [(i32, i32); 6] = [(26, 62), (54, 62), (252, 54), (280, 54), (252, 125), (280, 125)];
    // Drawn on the screen only (c08e): the view copies leave them alone.
    let mut targets: [&mut Vec<u8>; 1] = [&mut st.screen];
    for bp in 0..6 {
        if st.target[bp] == st.cur[bp] {
            continue;
        }
        if st.target[bp] > st.cur[bp] {
            st.cur[bp] += 1;
        } else {
            st.cur[bp] -= 1;
        }
        let h = st.cur[bp].min(30) as i32;
        let (x, y0) = POS[bp];
        let y = y0 - h;
        for fb in targets.iter_mut() {
            if let Some(s) = ic.sprites.get(0x37 + (bp & 1)) {
                s.draw(fb, 320, x, y, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
            if let Some(s) = ic.sprites.get(0x39) {
                s.draw(fb, 320, x, y - 10, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
            if st.cur[bp] == st.target[bp] {
                let fg = if bp & 1 == 0 { 0x3F } else { 0x25 };
                text(fb, &assets.font2, x + 4, y - 10, &[st.trend[bp]], fg, 0);
            }
        }
    }
}
