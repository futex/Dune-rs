//! Movement outside rooms: leaving a place on foot and walking in the desert, the
//! ornithopter (destination screen, take-off, flight, landing) and worms.
//! docs/re/travel.md §3-§5, animations.md §3 and §4.4.
//!
//! Keyboard equivalents for the destination screen: Tab = hover the next place icon
//! (sets ds:46FC and its label), Enter = click it, `n`/`e`/`s`/`w` = hover the desert
//! direction north/east/south/west (as if the mouse were on that compass ray of the
//! position marker), arrows = scroll (zones 13..16), `c` = re-centre. In flight with
//! manual steering: Left/Right = turn (zones 13/15), Up = RESUME FLIGHT (zone 14).

use crate::{
    assets::Assets,
    audio::Audio,
    input::Key,
    intro::play_voc,
    panel,
    world::{fill, rect, text, Cmd, Line, Scr, World, VIEW},
};
use dune_formats::{
    hnm::HnmPlayer,
    palette::Palette,
    planet,
    sprite::{DrawOpts, SpriteSheet},
};

const TRAIL: usize = (0xE85C - 0xE40C) / 4;

/// HNM number -> file (resource 0xD2 + n).
fn hnm_name(n: u16) -> &'static str {
    match n {
        1 => "DFL2.HNM",
        2 => "MNT1.HNM",
        3 => "MNT2.HNM",
        4 => "MNT3.HNM",
        5 => "MNT4.HNM",
        6 => "SIET.HNM",
        7 | 8 => "PALACE.HNM",
        9 | 10 => "FORT.HNM",
        _ => "VER.HNM",
    }
}

fn ticks_per_frame(n: u16) -> u32 {
    match n {
        1 => 18,
        0x0E => 0x19,
        _ => 16,
    }
}

// ---------------------------------------------------------------------------------------
// Backdrops and the parked ornithopter

/// sub_1380C: picture resource of the exterior room / the desert.
pub(crate) fn backdrop_resource(w: &World, assets: &Assets) -> Option<usize> {
    let tab = assets.exe_bytes(0x1972, 10)?;
    let si = w.get16(0x1150) as usize;
    let si = if si < 0x100 { 0x100 } else { si };
    let k = planet::kind(w.mem[si + 8]);
    let (base, lim) = (tab[k], tab[k + 5]);
    let ds6 = w.get16(6);
    let v = if ds6 & 0xFF == 0x80 {
        Some(0u8)
    } else if w.get16(4) == w.get16(si + 2) {
        Some((ds6 >> 8) as u8)
    } else {
        None
    };
    if let Some(v) = v.filter(|&v| v < lim) {
        let mut al = base.wrapping_add(v);
        if al == 0x7F {
            al = al.wrapping_add(w.mem[si] >> 1).wrapping_sub(5);
        }
        return Some(al as usize);
    }
    let (n, first) = if w.mem[si + 0x0A] & 1 != 0 {
        (10u32, 0x88u32)
    } else {
        let (lon, row) = w.cur_pos();
        let (di, _) = planet::cell_ptr(&w.mem, lon, row);
        let veg = (0..4).any(|i| w.map.get((di + i).wrapping_sub(1)).map(|&c| c & 0x30 == 0x10).unwrap_or(false));
        if veg {
            (10, 0x88)
        } else {
            (0x13, 0x42)
        }
    };
    let h = (ds6.swap_bytes() ^ w.get16(4)).wrapping_add(1) as u32;
    Some((first + h % n) as usize)
}

/// seg000:3A95: orni position, sietch (0x95,0x39) / other places (0xCA,0x49).
pub(crate) fn orni_base(w: &World) -> (i32, i32) {
    if w.mem[5] < 0x20 {
        (0x95, 0x39)
    } else {
        (0xCA, 0x49)
    }
}

/// sub_13AA9: the four parts of an ornithopter for frame p.
pub(crate) fn draw_orni_parts(fb: &mut [u8], bank: &SpriteSheet, x: i32, y: i32, p: u8) {
    let clip = Some((0, 0, 320, 152));
    let put = |fb: &mut [u8], i: usize, x: i32, y: i32| {
        if let Some(s) = bank.sprites.get(i) {
            s.draw(fb, 320, x, y, DrawOpts { clip, ..Default::default() });
        }
    };
    put(fb, 0, x, y);
    put(fb, 1, x + 6, y + 30);
    let g = (p as i32 - 15).clamp(0, 5) as usize;
    put(fb, 2 + g, x + 4, y + 50);
    put(fb, 8 + (p.min(14)) as usize, x - 81, y - 3);
}

// ---------------------------------------------------------------------------------------
// On foot

/// Compass click on an exit FB..FF (seg000:3FD2): leave the place in direction -v.
pub(crate) fn leave_place(w: &mut World, assets: &mut Assets, audio: &mut Audio, v: u8) {
    w.mem[0xE7] = 0;
    let bp = (v as i8).wrapping_neg() as usize;
    let si = w.get16(0x114E) as usize;
    w.set16(0x114E, 0);
    let dx = w.get16(si + 2);
    let bx = w.get16(si + 4) & 0x00FF;
    w.mem[8] = 0xFF;
    w.mem[9] = 0xFF;
    step(w, assets, audio, bp, dx, bx);
}

/// Outdoors, an arrow click (seg000:3F49): step counter, spice exposure, then the step.
pub(crate) fn walk(w: &mut World, assets: &mut Assets, audio: &mut Audio, dir: usize) {
    if w.in_transit() {
        return;
    }
    let n = ((w.mem[0x4735] & 0x7F) + 1).min(0x7F);
    w.mem[0x4735] = 0x80 | n;
    if w.mem[0xF4] < 0x14 {
        w.mem[0xF4] += 1;
    }
    let (dx, bx) = (w.get16(4), w.get16(6));
    step(w, assets, audio, dir + 1, dx, bx);
}

/// sub_1B5CF and the place check loc_14002.
fn step(w: &mut World, assets: &mut Assets, audio: &mut Audio, bp: usize, mut dx: u16, bx: u16) {
    let ax = w.get16(0x1454 + 2 * bp);
    let (al, ah) = (ax as u8 as i8, (ax >> 8) as u8);
    let (mut bl, mut bh) = (bx as u8, (bx >> 8) as u8);
    if ah != 0 {
        if (ah as i8) > 0 {
            let (v, c) = bh.overflowing_add(ah);
            bh = v;
            if c {
                bl = bl.wrapping_add(1);
                if (bl as i8) >= 0x62 {
                    bl = bl.wrapping_sub(1);
                    bh = bh.wrapping_sub(ah);
                }
            }
        } else {
            let (v, c) = bh.overflowing_add(ah);
            bh = v;
            if !c {
                bl = bl.wrapping_sub(1);
                if (bl as i8) <= -0x62 {
                    bl = bl.wrapping_add(1);
                    bh = bh.wrapping_sub(ah);
                }
            }
        }
    }
    dx = dx.wrapping_add(al as i16 as u16);
    let bx = ((bh as u16) << 8) | bl as u16;
    if bh == 0 && arrive(w, assets, audio, dx, bl as i8 as i16) {
        return;
    }
    commit(w, assets, audio, dx, bx);
}

/// loc_14002: entering a place whose cell and exact longitude match.
pub(crate) fn arrive(w: &mut World, assets: &mut Assets, audio: &mut Audio, dx: u16, row: i16) -> bool {
    let (di, _) = planet::cell_ptr(&w.mem, dx, row);
    if w.map.get(di).map(|c| c & 0x40 == 0).unwrap_or(true) {
        return false;
    }
    let Some(i) = planet::place_at_cell(&w.mem, di) else { return false };
    let si = planet::loc_addr(i);
    if dx != w.get16(si + 2) {
        return false;
    }
    w.mem[0x4735] = 0;
    w.set16(0x114E, si as u16);
    w.set16(0x1150, si as u16);
    w.set16(0x9A, 0);
    w.set16(0x98, 0);
    // sub_1503C: hostility (war mode, or game over 4 in a garrisoned fort).
    w.hostility_check();
    discover(w, si);
    let app = w.mem[si + 8];
    w.mem[8] = app;
    w.mem[9] = i as u8;
    if app >= 0x20 {
        w.mem[si + 0x0A] |= 0x10;
    }
    w.set16(6, ((i as u16) << 8) | 0x80);
    let place = ((app as u16) << 8) | 1;
    if w.sal_kind == usize::MAX || place >> 8 != w.place() >> 8 {
        w.sal_kind = usize::MAX;
    }
    let _ = audio;
    w.enter(assets, place);
    true
}

/// sub_1425B: a place becomes known.
fn discover(w: &mut World, si: usize) {
    if w.mem[si + 0x0A] & 0x80 == 0 {
        return;
    }
    w.mem[si + 0x0A] &= 0x7F;
    w.mem[si + 0x0B] = 0;
    if w.mem[si + 8] < 0x20 {
        w.mem[0x27] = w.mem[0x27].wrapping_add(1);
        if w.get16(si) == 0x0603 && w.mem[0x2A] < 0x10 {
            w.mem[0x2A] = 0x10;
        }
    }
}

/// sub_14057 outdoors: commit ds:4/ds:6 and redraw, then the after-redraw hook.
fn commit(w: &mut World, assets: &mut Assets, audio: &mut Audio, dx: u16, bx: u16) {
    w.set16(4, dx);
    let old = w.mem[0xB];
    w.mem[0xB] = dx as u8;
    w.mem[0xD] = old;
    w.set16(6, bx);
    w.move_followers();
    w.redraw_room(assets);
    after_step(w, assets, audio);
}

/// seg000:3723 (from sub_12E52): sun pictures and the collapse.
fn after_step(w: &mut World, _assets: &mut Assets, _audio: &mut Audio) {
    let al = w.mem[0x4735];
    if al & 0x80 == 0 {
        return;
    }
    w.mem[0x4735] &= 0x7F;
    let n = al.wrapping_sub(0x94);
    if al < 0x94 || n & 0x0F != 0 {
        return;
    }
    let k = ((n as u32 >> 4) + 1).min(10);
    w.seq = Some(Seq::Sun { until: w.now + 256 * k as u64, view: vec![], pal: None });
    w.dirty = true;
}

/// End of the sun wait: collapse (n >= 0x37) or just the view again (sub_12DB1).
fn sun_done(w: &mut World, assets: &mut Assets) {
    if w.mem[0x4735] < 0x37 {
        w.redraw_room(assets);
        return;
    }
    // Collapse: 5 slots pass, fade to black, back to the last place.
    advance_slots(w, assets, 5);
    let si = (w.get16(0x1150) as usize).max(0x100);
    let i = (si - 0xE4) / 0x1C;
    w.mem[0x4735] = 0;
    w.set16(0x114E, si as u16);
    discover(w, si);
    let app = w.mem[si + 8];
    w.mem[8] = app;
    w.mem[9] = i as u8;
    if app >= 0x20 {
        w.mem[si + 0x0A] |= 0x10;
    }
    w.set16(6, ((i as u16) << 8) | 0x80);
    let dl = if app < 0x20 {
        2
    } else if app == 0x20 {
        0x0A
    } else {
        1
    };
    w.sal_kind = usize::MAX;
    w.effect = Some(0x34);
    w.via_black = true;
    w.enter(assets, ((app as u16) << 8) | dl);
    w.mem[0xE7] = w.mem[0xE7].wrapping_add(1);
}

/// sub_10FD9: advance the clock by n slots.
pub(crate) fn advance_slots(w: &mut World, assets: &mut Assets, n: u16) {
    w.skipping = true;
    for _ in 0..n {
        let t = w.clock().wrapping_add(1);
        w.set16(2, t);
        w.new_slot(assets);
        if w.mem[0x46D9] != 0 {
            break;
        }
    }
    w.skipping = false;
}

/// WAIT FOR EVENING (0F48) / WAIT FOR MORNING (0F67).
pub(crate) fn wait(w: &mut World, assets: &mut Assets, evening: bool) {
    let t = w.clock();
    let slot = t & 15;
    let target = if evening {
        if slot >= 12 {
            return;
        }
        (t & 0xFFF0) | 0x0C
    } else {
        if slot < 11 {
            return;
        }
        (t & 0xFFF0) + 0x10
    };
    w.fade = None;
    let n = target.wrapping_sub(t);
    advance_slots(w, assets, n);
    w.fade = None;
    w.effect = Some(0x2A);
    w.redraw_room(assets);
    if w.mem[0x2A] == 0x14 {
        w.idle_since = w.idle_since.saturating_sub(1000);
    }
}

// ---------------------------------------------------------------------------------------
// Destination screen (loc_1430B)

pub struct Dest {
    pub cockpit: bool,
    /// Displayed view and the clean copy (before the marker).
    pub screen: Vec<u8>,
    pub clean: Vec<u8>,
    pub pal: Palette,
    /// ds:46FC: hovered target (record, 0xFFF0 + dir, 0 = none).
    pub di: u16,
    /// Typewriter (task 46B5): next character index, pen x, running.
    pub tw: Option<(usize, i32)>,
    pub tw_acc: u32,
    /// Marker rect ds:4749 and blink counter ds:4751.
    pub marker: Option<(i32, i32, i32, i32)>,
    pub blink: u8,
    pub blink_acc: u32,
    pub kb: usize,
    /// The mouse position used for a free-flight target.
    pub click: (i32, i32),
}

fn text_y(cockpit: bool) -> i32 {
    if cockpit {
        0x26
    } else {
        0x22
    }
}

/// TAKE AN ORNITHOPTER from a room (42E9).
pub(crate) fn take_orni(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    w.sky_slot_change(assets);
    w.mem[0x473E] = 1;
    w.mem[0x11C9] = 4;
    w.set16(0x487E, 2);
    open_dest(w, assets, audio);
}

/// TAKE AN ORNITHOPTER from the maps menu (42D9): to the exterior room first.
pub(crate) fn take_orni_maps(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    let p = (w.place() & 0xFF00) | 1;
    w.set16(4, p);
    w.mem[0xFB] = 1;
    w.map_reset();
    w.mem[0x11CA] = 0;
    w.effect = Some(0x34);
    w.redraw_room(assets);
    // The transition is synchronous in the original: 42E9 runs once it is over.
    w.after_trans = Some(After::TakeOrni);
    let _ = audio;
}

/// Actions that the original runs after a synchronous transition.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum After {
    TakeOrni,
    TakeOff(u16),
}

pub(crate) fn run_after(w: &mut World, assets: &mut Assets, audio: &mut Audio, a: After) {
    match a {
        After::TakeOrni => take_orni(w, assets, audio),
        After::TakeOff(di) => {
            set_destination(w, di, (0, 0));
            take_off(w, assets, audio);
        }
    }
}

/// CALL A WORM (42D1): sub_14285 then the destination screen without the cockpit.
pub(crate) fn call_worm(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    worm_setup(w);
    open_dest(w, assets, audio);
}

fn worm_setup(w: &mut World) {
    w.set16(0x487E, 1);
    w.mem[0x473E] = 0;
    w.mem[0x11C9] = 8;
}

/// 4305 / 430B: the destination selection screen.
fn open_dest(w: &mut World, assets: &mut Assets, _audio: &mut Audio) {
    // sub_149EA: clear the trail ring.
    clear_trail(w);
    w.mem[0x11CA] = 1;
    let (lon, row) = w.cur_pos();
    w.set16(0x197C, lon);
    w.set16(0x197E, row as u16);
    w.mem[0x46EB] = 1;
    for (k, v) in [81u16, 45, 241, 134].iter().enumerate() {
        w.set16(0x46E3 + 2 * k, *v);
    }
    let cockpit = w.mem[0x473E] != 0;
    let mut pal = w.palette.clone();
    let mut screen = vec![0u8; VIEW];
    let base = match &w.flight {
        Some(f) if w.in_transit() => f.frame.clone(),
        _ => w.view.clone(),
    };
    screen.copy_from_slice(&base);
    if let Some(p) = assets.sheet("ORNYPAN.HSQ") {
        if cockpit {
            // sub_1439F: sky bands, then ORNYPAN list ds:14B4.
            if w.mem[0x2B] == 0 {
                w.mem[0x22E3] = 1;
                w.sky_palette(assets, &mut pal);
                crate::sky::draw_bands(assets, &mut screen);
            }
            for (i, x, y) in [(0usize, 0, 19), (1, 10, 43), (2, 79, 45)] {
                if let Some(s) = p.sprites.get(i) {
                    s.draw(&mut screen, 320, x, y, DrawOpts::default());
                }
            }
        }
        p.apply_palette(&mut pal);
    }
    let mut fb = vec![0u8; 320 * 200];
    fb[..VIEW].copy_from_slice(&screen);
    if !cockpit {
        // Worm: frame around the map and the text bar ds:14A4 (77,33)-(245,41) in F5.
        let mut c = 0xFCu8;
        for k in 1..=4 {
            rect(&mut fb, 81 - k, 45 - k, 240 + k, 133 + k, c);
            c -= 2;
        }
        fill(&mut fb, 77, 33, 245, 41, 0xF5);
    }
    w.dest = Some(Dest {
        cockpit,
        screen: vec![],
        clean: vec![],
        pal,
        di: 0,
        tw: None,
        tw_acc: 0,
        marker: None,
        blink: 0,
        blink_acc: 0,
        kb: usize::MAX,
        click: (0, 0),
    });
    w.dest.as_mut().unwrap().screen = fb[..VIEW].to_vec();
    dest_redraw(w, assets);
    // Narrator PZ2BC "select destination", the typewriter prompt.
    play_voc(assets, _audio, "PZ\\PZ2BCI.VOC");
    if let Some(d) = &mut w.dest {
        d.tw = Some((0, 0x55));
    }
    w.scr = Scr::Dest;
    let mem = w.mem.clone();
    w.panel.set_compass(&mem, panel::COMPASS_MAP);
    w.build_menu();
    w.dirty = true;
}

/// 4377: the flight map in (81,45)-(241,134), icons, grid, the position marker.
fn dest_redraw(w: &mut World, assets: &mut Assets) {
    let Some(mut d) = w.dest.take() else { return };
    let mut fb = vec![0u8; 320 * 200];
    fb[..VIEW].copy_from_slice(&d.screen);
    w.mem[0x46EB] = 1;
    planet::render_flight(&mut w.mem, &w.map, &mut fb, &w.drv);
    w.draw_icons(assets, &mut fb, 1, (81, 45, 241, 134));
    if d.cockpit {
        if let Some(p) = assets.sheet("ORNYPAN.HSQ") {
            if let Some(s) = p.sprites.get(2) {
                s.draw(&mut fb, 320, 79, 45, DrawOpts::default());
            }
        }
    }
    d.clean = fb[..VIEW].to_vec();
    d.screen = d.clean.clone();
    // sub_1445D: the blinking position marker (ICONES 0x4C at (x - 13, y - h)).
    d.marker = None;
    let (lon, row) = w.cur_pos();
    let (x, y) = planet::map_to_screen(&w.mem, lon, row);
    if (81..241).contains(&x) && (45..134).contains(&y) {
        if let Some(ic) = assets.sheet("ICONES.HSQ") {
            if let Some(s) = ic.sprites.get(0x4C) {
                let (mw, mh) = (s.width as i32, s.height as i32);
                d.marker = Some((x - 13, y - mh, x - 13 + mw, y));
            }
        }
    }
    d.blink = 0;
    d.blink_acc = 0;
    w.dest = Some(d);
    dest_blink(w, assets);
    label(w, assets);
}

/// Task 44AB: marker blink (every 300 ticks).
fn dest_blink(w: &mut World, assets: &mut Assets) {
    let Some(d) = &mut w.dest else { return };
    d.blink = d.blink.wrapping_add(1);
    let Some((x0, y0, x1, y1)) = d.marker else { return };
    let (x0c, y0c, x1c, y1c) = (x0.max(81), y0.max(45), x1.min(241), y1.min(134));
    for y in y0c..y1c {
        for x in x0c..x1c {
            let p = (y * 320 + x) as usize;
            d.screen[p] = d.clean[p];
        }
    }
    if d.blink & 1 != 0 {
        if let Some(ic) = assets.sheet("ICONES.HSQ") {
            if let Some(s) = ic.sprites.get(0x4C) {
                s.draw(&mut d.screen, 320, x0, y0, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
        }
    }
    w.dirty = true;
}

/// sub_145DE: the label line for ds:46FC (place / desert direction / prompt).
fn label(w: &mut World, assets: &mut Assets) {
    let di = match &w.dest {
        Some(d) => d.di,
        None => return,
    };
    let cockpit = w.dest.as_ref().unwrap().cockpit;
    let y = text_y(cockpit);
    let (fg, bg) = (0xFE, if cockpit { 0x20 } else { 0xF5 });
    let mut fb = vec![0u8; 320 * 200];
    fb[..VIEW].copy_from_slice(&w.dest.as_ref().unwrap().screen);
    let font = &assets.font2;
    let mut x = 0x55;
    if di == 0 {
        // sub_14658: restart the typewriter unless it runs, and clear the line.
        if let Some(d) = &mut w.dest {
            if d.tw.is_none() {
                d.tw = Some((0, 0x55));
                d.tw_acc = 0;
            }
        }
    } else {
        if let Some(d) = &mut w.dest {
            d.tw = None;
        }
        if di >= 0xFFF0 {
            x = text(&mut fb, font, x, y, &World::cmd_text(assets, 0xA4), fg, bg);
            let dir = di.wrapping_sub(0xFFF0);
            if dir < 8 {
                x = text(&mut fb, font, x, y, b" ", fg, bg);
                x = text(&mut fb, font, x, y, &World::cmd_text(assets, 0xDA + dir), fg, bg);
            }
        } else {
            let rec = di as usize;
            x = text(&mut fb, font, x, y, &w.place_type_text(assets, rec), fg, bg);
            x = text(&mut fb, font, x, y, &w.place_name(assets, rec), fg, bg);
        }
    }
    while x <= 0xED {
        x = text(&mut fb, font, x, y, b" ", fg, bg);
    }
    if let Some(d) = &mut w.dest {
        d.screen.copy_from_slice(&fb[..VIEW]);
    }
    w.dirty = true;
}

/// Task 46B5: one character of "SELECT DESTINATION ON MAP" every 24 ticks.
fn typewriter(w: &mut World, assets: &mut Assets) {
    let Some(d) = &mut w.dest else { return };
    let Some((mut i, mut x)) = d.tw else { return };
    let s = World::cmd_text(assets, 0x57);
    let y = text_y(d.cockpit);
    let bg = if d.cockpit { 0x20 } else { 0xF5 };
    let mut fb = vec![0u8; 320 * 200];
    fb[..VIEW].copy_from_slice(&d.screen);
    loop {
        let Some(&c) = s.get(i) else {
            d.tw = None;
            break;
        };
        i += 1;
        x = text(&mut fb, &assets.font2, x, y, &[c], 0x61, bg);
        d.tw = Some((i, x));
        if c != b' ' {
            break;
        }
    }
    d.screen.copy_from_slice(&fb[..VIEW]);
    w.dirty = true;
}

pub(crate) fn dest_tick(w: &mut World, assets: &mut Assets) {
    let Some(d) = &mut w.dest else { return };
    d.tw_acc += 1;
    let tw = d.tw_acc >= 0x18;
    if tw {
        d.tw_acc = 0;
    }
    d.blink_acc += 1;
    let bl = d.blink_acc >= 0x12C;
    if bl {
        d.blink_acc = 0;
    }
    if tw {
        typewriter(w, assets);
    }
    if bl {
        dest_blink(w, assets);
    }
}

pub(crate) fn dest_compose(w: &mut World, _assets: &mut Assets, fb: &mut [u8]) -> Palette {
    match &w.dest {
        Some(d) => {
            fb[..VIEW].copy_from_slice(&d.screen);
            d.pal.clone()
        }
        None => w.palette.clone(),
    }
}

pub(crate) fn dest_overlays(_w: &mut World, _assets: &mut Assets, _fb: &mut [u8]) {}

/// sub_14586: hover.
fn hover(w: &mut World, assets: &mut Assets, mx: i32, my: i32) {
    let Some(d) = &w.dest else { return };
    let mut di = 0u16;
    if (81..241).contains(&mx) && (45..134).contains(&my) {
        let near = w.mapst.icons.iter().map(|&(rec, x, y)| (rec, (x - mx).abs() + (y - my).abs())).min_by_key(|e| e.1);
        if let Some((rec, _)) = near.filter(|e| e.1 < 9) {
            di = rec as u16;
        } else if let Some((x0, _, _, y1)) = d.marker {
            let a = angle(mx - (x0 + 0x0B), my - y1).unwrap_or(0);
            let b = a.wrapping_add(3);
            if b & 0x1F < 6 {
                di = 0xFFF0 | ((b >> 5) & 7) as u16;
            }
        }
    }
    set_hover(w, assets, di);
}

fn set_hover(w: &mut World, assets: &mut Assets, di: u16) {
    let Some(d) = &mut w.dest else { return };
    if d.di == di {
        return;
    }
    d.di = di;
    label(w, assets);
}

pub(crate) fn dest_mouse(w: &mut World, assets: &mut Assets, audio: &mut Audio, mx: i32, my: i32, clicked: bool) {
    if w.seq.is_some() {
        return;
    }
    hover(w, assets, mx, my);
    if clicked && my < 152 {
        if let Some(d) = &mut w.dest {
            d.click = (mx, my);
        }
        dest_click(w, assets, audio);
    }
}

pub(crate) fn dest_key(w: &mut World, assets: &mut Assets, audio: &mut Audio, k: Key) {
    if w.seq.is_some() {
        return;
    }
    match k {
        Key::Up | Key::Down | Key::Left | Key::Right => {
            let d = match k {
                Key::Up => 0xFFF4u16 as i32,
                Key::Down => 12,
                _ => 0,
            };
            if d != 0 {
                w.set16(0x197E, w.get16(0x197E).wrapping_add(d as u16));
            } else {
                let dl = if k == Key::Right { 0x1002u16 } else { 0xEFFE };
                w.set16(0x197C, w.get16(0x197C).wrapping_add(dl));
            }
            dest_redraw(w, assets);
        }
        Key::Char('c') => {
            let (lon, row) = w.cur_pos();
            w.set16(0x197C, lon);
            w.set16(0x197E, row as u16);
            dest_redraw(w, assets);
        }
        Key::Tab => {
            let n = w.mapst.icons.len();
            if n == 0 {
                return;
            }
            let Some(d) = &mut w.dest else { return };
            d.kb = if d.kb == usize::MAX { 0 } else { (d.kb + 1) % n };
            let rec = w.mapst.icons[d.kb].0 as u16;
            set_hover(w, assets, rec);
        }
        Key::Char(c @ ('n' | 'e' | 's' | 'w')) => {
            let dir = match c {
                'n' => 0,
                'e' => 2,
                's' => 4,
                _ => 6,
            };
            // A point 30 px along that ray from the marker centre, as the mouse would be.
            if let Some(d) = &mut w.dest {
                if let Some((x0, _, _, y1)) = d.marker {
                    let (cx, cy) = (x0 + 0x0B, y1);
                    let (vx, vy) = [(0, -30), (21, -21), (30, 0), (21, 21), (0, 30), (-21, 21), (-30, 0), (-21, -21)][dir];
                    d.click = (cx + vx, cy + vy);
                }
            }
            set_hover(w, assets, 0xFFF0 | dir as u16);
        }
        Key::Enter => dest_click(w, assets, audio),
        Key::Esc => cancel_dest(w, assets),
        _ => {}
    }
}

/// 450E: click -> narrator, flash the label 9 times, then take off.
fn dest_click(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    if w.mem[0x11C9] & 0x0F == 0 {
        return;
    }
    let Some(d) = &w.dest else { return };
    let di = d.di;
    if di == 0 {
        return;
    }
    if di < 0xFFF0 && !w.in_transit() && di == w.get16(0x114E) {
        return;
    }
    // sub_1456C: PZ voice of the place (or of the desert direction).
    let id = if di >= 0xFFF0 {
        di.wrapping_add(0x2BC)
    } else {
        let (b0, b1) = (w.mem[di as usize] as u16, w.mem[di as usize + 1] as u16);
        ((((b0.wrapping_sub(1)) << 4) as u8) | b1 as u8) as u16 + 0x2BC
    };
    play_voc(assets, audio, &format!("PZ\\PZ{id:03X}I.VOC"));
    let voice_end = w.now + (audio.queued_secs() * crate::video::TICK_HZ) as u64;
    w.seq = Some(Seq::Flash { k: 0, on: true, next: w.now + 20, di, voice_end });
}

/// Menu Cancel / exit callback 4415.
pub(crate) fn cancel_dest(w: &mut World, assets: &mut Assets) {
    w.mem[0x46EB] = 0;
    w.mapst.icons.clear();
    w.dest = None;
    if w.get16(0x11C5) == 0 {
        w.mem[0x11C9] = 0;
    }
    w.mem[0x11CA] = 0;
    if w.in_transit() {
        w.mem[0x4728] = 1;
        flight_view(w, assets);
    } else {
        w.redraw_room(assets);
    }
}

// ---------------------------------------------------------------------------------------
// Steering (sub_15124 .. sub_15206)

/// sub_1514E: angle of (dx, dy), 256 units, 0 = N, 0x40 = E. None when both are 0.
fn angle(dx: i32, dy: i32) -> Option<u8> {
    let (ax, cx) = (dy.unsigned_abs() as i32, dx.unsigned_abs() as i32);
    if cx >= ax {
        if cx < 1 {
            return None;
        }
        let q = (0x20 * dy as i64 / dx as i64) as i32;
        Some(if dx >= 0 { (q as u8).wrapping_add(0x40) } else { (q as u8).wrapping_add(0xC0) })
    } else {
        let q = (0x20 * dx as i64 / dy as i64) as i32;
        let mut al = q as u8;
        if dy >= 0 {
            al = al.wrapping_sub(0x80);
        }
        Some(al.wrapping_neg())
    }
}

/// sub_15133: heading from (lon, row) to (tlon, trow).
fn heading(lon: u16, row: i16, tlon: u16, trow: i16) -> Option<u8> {
    let mut bx = (trow as i32 - row as i32) as i16 as i32;
    let mut dx = tlon.wrapping_sub(lon) as i16 as i32;
    if !(-0x80..0x80).contains(&bx) {
        bx >>= 1;
        dx >>= 1;
    }
    let bx = ((bx as u8 as u16) << 8) as i16 as i32;
    angle(dx, bx)
}

/// sub_15124: heading from the current position to a place.
fn heading_to(w: &World, rec: usize) -> Option<u8> {
    let (lon, row) = w.cur_pos();
    heading(lon, row, w.get16(rec + 2), w.get16(rec + 4) as i16)
}

/// sub_15198: heading -> square vector (x, y), max component 32.
fn vector(h: u8) -> (i32, i32) {
    let bl = h.wrapping_add(0x20);
    if bl & 0x7F >= 0x40 {
        let mut dx = 0x20;
        let mut al = h.wrapping_sub(0x40);
        if bl & 0x80 != 0 {
            dx = -0x20;
            al = al.wrapping_sub(0x80).wrapping_neg();
        }
        (dx, al as i8 as i32)
    } else {
        let mut bx = -0x20;
        let mut al = h;
        if bl & 0x80 != 0 {
            al = al.wrapping_sub(0x80).wrapping_neg();
            bx = 0x20;
        }
        (al as i8 as i32, bx)
    }
}

/// sub_151CB + sub_15206: one journey step from (lon, row).
fn step_pos(w: &mut World, lon: u16, row: i16) -> (u16, i16) {
    if w.mem[0x11CB] != 0 || w.mem[0x11C8] != 0 {
        if !(-0x4D..=0x4D).contains(&row) {
            let al = w.mem[0x11C7];
            let ah = al.wrapping_sub(0x40);
            let bh = if row < 0 { 0xFF } else { 0 };
            if (ah ^ bh) & 0x80 == 0 {
                w.mem[0x11C7] = (al & 0x80) | 0x40;
            }
        }
    } else {
        let dest = w.get16(0x11C5) as usize;
        if dest >= 0x100 {
            if let Some(a) = heading_to(w, dest) {
                w.mem[0x11C7] = a;
            }
        }
    }
    let h = w.mem[0x11C7];
    let bp = w.get16(planet::LON_UNITS + 2 * (row as i32).unsigned_abs().min(98) as usize) as i16 as i32;
    let (vx, vy) = vector(h);
    let mut dlon = bp * vx / 32;
    let dlat = bp * vy / 32;
    let mut a = dlat.unsigned_abs() as i32 + w.get16(0x11CC) as i32;
    if a >= 0x200 {
        dlon = ((dlon as i64 * 256) / a as i64) as i32;
        a = 0x100;
    }
    w.mem[0x11CC] = a as u8;
    let mut drow = ((a >> 8) as u8) as i8 as i32;
    if dlat < 0 {
        drow = -drow;
    }
    let mut row = (row as i32 + drow) as i16;
    let mut lon = lon.wrapping_add(dlon as i16 as u16);
    if (row as u16).wrapping_add(0x60) >= 0xC0 {
        w.mem[0x11C7] = w.mem[0x11C7].wrapping_add(0x80);
        lon = lon.wrapping_add(0x8000);
    }
    let _ = &mut row;
    (lon, row)
}

/// sub_14944: destination / heading from ds:46FC.
fn set_destination(w: &mut World, di: u16, click: (i32, i32)) {
    w.mem[0x11CB] = 0;
    let (rec, cl, h) = if di >= 0xFFF0 {
        w.mem[0x11CB] = 0xFF;
        let (tlon, trow) = planet::screen_to_map(&w.mem, click.0, click.1);
        let (lon, row) = w.cur_pos();
        (w.get16(0x1150), 1u8, heading(lon, row, tlon, trow).unwrap_or(0))
    } else {
        (di, 0, heading_to(w, di as usize).unwrap_or(0))
    };
    w.set16(0x11C5, rec);
    w.mem[0x11C8] = cl;
    w.mem[0x11C7] = h;
    w.set16(0x11CC, 0x80);
}

// ---------------------------------------------------------------------------------------
// Take-off and flight

fn clear_trail(w: &mut World) {
    w.mem[0x4728] = 0;
    if let Some(f) = &mut w.flight {
        f.trail = vec![(0x0800, 0x0800); TRAIL];
    }
    w.set16(0x149A, 0xE40C);
}

pub struct Flight {
    pub cur: u16,
    pub next: u16,
    pub player: Option<HnmPlayer>,
    pub frame_acc: u32,
    /// ds:DBEA: frames of the current HNM shown so far.
    pub shown: usize,
    pub frame: Vec<u8>,
    /// UI buffer ds:DC32 (the small map at (200,0)-(320,64)).
    pub overlay: Vec<u8>,
    pub trail: Vec<(u16, u16)>,
    pub trail_w: usize,
    pub pal: Palette,
    /// Harkonnen warning text over the cockpit (menu ds:1F9E up).
    pub warning: bool,
    pub warn_view: Vec<u8>,
    /// VER.BIN worm overlay script.
    pub worm: Option<WormScript>,
    pub closed: bool,
}

impl Flight {
    fn new(w: &World) -> Flight {
        Flight {
            cur: 0,
            next: 0,
            player: None,
            frame_acc: 0,
            shown: 0,
            frame: w.view.clone(),
            overlay: vec![0; 320 * 200],
            trail: vec![(0x0800, 0x0800); TRAIL],
            trail_w: 0,
            pal: w.palette.clone(),
            warning: false,
            warn_view: vec![],
            worm: None,
            closed: false,
        }
    }
}

/// GO THERE FLYING AN ORNI (50DB) / RIDING A WORM (50EA) from the map popup.
pub(crate) fn go_there(w: &mut World, assets: &mut Assets, audio: &mut Audio, worm: bool) {
    let di = w.mapst.selected as u16;
    if worm {
        worm_setup(w);
    } else {
        w.set16(0x487E, 2);
        w.mem[0x473E] = 1;
    }
    w.map_deselect();
    clear_trail(w);
    if !worm {
        let p = (w.place() & 0xFF00) | 1;
        w.set16(4, p);
    }
    // loc_1186B: leave the map (the exterior room through effect 0x34).
    w.mem[0xFB] = 1;
    w.map_reset();
    w.mem[0x11CA] = 0;
    w.effect = Some(0x34);
    w.redraw_room(assets);
    w.mem[0x11C9] = if worm { 8 } else { 4 };
    w.after_trans = Some(After::TakeOff(di));
    let _ = audio;
}

/// loc_14703 after sub_14944.
fn take_off(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    w.sky_slot_change(assets);
    let old = w.mem[0x11C9];
    w.mem[0x11C9] |= old >> 2;
    w.mem[0x2B] = 0;
    if let Some(d) = w.dest.take() {
        w.screen_pal = Some(d.pal);
    }
    w.mem[0x46EB] = 0;
    if old & 3 != 0 {
        // CHANGE DESTINATION while flying.
        w.mem[0x11CA] = 0;
        w.mem[0x4728] = 1;
        flight_view(w, assets);
        return;
    }
    w.set16(0x472B, 0);
    w.mem[0x4726] = 0;
    w.mem[0x21FD] = 0;
    if w.mem[0x11C9] & 3 == 1 {
        w.medallion_to = 0;
    }
    w.mem[0x11CA] = 0;
    let anim = std::mem::replace(&mut w.mem[0x4732], 0) & 0x80 != 0;
    if old == 4 {
        if anim {
            // sub_14795: ORNYTK take-off over the exterior room, sfx 6.
            // The destination menu is popped (exit callback 4415): room menu, compass.
            w.scr = Scr::Room;
            w.compass();
            w.build_menu();
            w.mem[0x4731] = 0xFF;
            w.render_room(assets);
            let clean = w.view.clone();
            w.mem[0x4731] = 0;
            play_voc(assets, audio, "SN6.HSQ");
            let pal = w.palette.clone();
            let mut s = Seq::Orni { p: 0, cl: 1, next: w.now, clean, view: vec![0; VIEW], pal, then: OrniThen::Fly };
            s.orni_frame(w, assets, audio);
            w.seq = Some(s);
            return;
        }
        fly(w, assets);
    } else {
        // Worm: story phase 0x50, VER.HNM (the worm rising) with sfx 8.
        if w.mem[0x2A] < 0x50 {
            w.mem[0x2A] = 0x50;
        }
        w.mem[0xE8] = 0;
        w.medallion_to = 0;
        if let Some(h) = assets.hnm("VER.HNM") {
            let mut p = HnmPlayer::new(h);
            // The video's palette chunks over the current palette.
            let mut pal = w.palette.clone();
            if let Ok(raw) = assets.dat.read("VER.HNM") {
                let hs = u16::from_le_bytes([raw[0], raw[1]]) as usize;
                pal.apply_chunks(&raw[2..hs.min(raw.len())]);
            }
            p.step();
            play_voc(assets, audio, "SN8.VOC");
            let mut view = vec![0u8; 320 * 200];
            view[24 * 320..176 * 320].copy_from_slice(&p.fb[..152 * 320]);
            w.effect = Some(0x10);
            w.seq = Some(Seq::Video { p, n: 0x0E, acc: 0, view, pal, shown: 1, then: VideoThen::Fly, full: true });
            w.dirty = true;
            return;
        }
        fly(w, assets);
    }
}

/// End of the take-off: first step, the flight view, the journey starts.
fn fly(w: &mut World, assets: &mut Assets) {
    w.mem[8] = 0xFF;
    let mut f = Flight::new(w);
    f.trail_w = ((w.get16(0x149A).wrapping_sub(0xE40C)) / 4) as usize % TRAIL;
    w.flight = Some(f);
    journey_step(w, assets);
    w.set16(0x114E, 0);
    w.set16(0x4729, 0);
    flight_view(w, assets);
    w.mem[0x4727] = 0xFF;
    if w.mem[0x11C9] & 3 == 1 {
        let home = w.get16(0x1150) as usize;
        if home >= 0x100 {
            w.mem[home + 0x15] = w.mem[home + 0x15].wrapping_sub(1);
        }
    }
}

/// After a load in flight: rebuild the flight state.
pub(crate) fn resume_flight(w: &mut World, assets: &mut Assets) {
    let mut f = Flight::new(w);
    f.trail_w = ((w.get16(0x149A).wrapping_sub(0xE40C)) / 4) as usize % TRAIL;
    w.flight = Some(f);
    w.mem[0x4728] = 1;
    flight_view(w, assets);
}

/// sub_137F4: the flight view (small map, trail, HNM ds:487E, sky palette).
pub(crate) fn flight_view(w: &mut World, assets: &mut Assets) {
    if w.flight.is_none() {
        w.flight = Some(Flight::new(w));
    }
    w.scr = Scr::Flight;
    w.mem[0x4728] = 0;
    w.mem[0x46DF] = 1;
    small_map(w, assets);
    let n = w.get16(0x487E);
    let mut pal = w.palette.clone();
    // Colours 16..55 keep the last map-bank palette (ONMAP / ORNYPAN) like the DAC shadow.
    if let Some(sp) = &w.screen_pal {
        pal.0[16..56].copy_from_slice(&sp.0[16..56]);
    }
    w.mem[0x22E3] = 1;
    w.sky_palette(assets, &mut pal);
    let restart = w.flight.as_ref().map(|f| f.player.is_none() || f.closed).unwrap_or(true);
    if let Some(f) = &mut w.flight {
        f.pal = pal;
        f.warning = false;
    }
    if restart {
        open_hnm(w, assets, n);
    }
    if n == 1 {
        if let Some(f) = &mut w.flight {
            if f.worm.is_none() {
                f.worm = assets.dat.read("VER.BIN").ok().and_then(WormScript::parse);
            }
        }
    }
    w.compass();
    w.panel.set_left(&w.mem.clone(), panel::LEFT_DEFAULT);
    w.build_menu();
    w.dirty = true;
}

fn open_hnm(w: &mut World, assets: &mut Assets, n: u16) {
    let Some(f) = &mut w.flight else { return };
    f.cur = n;
    f.next = n;
    f.shown = 0;
    f.frame_acc = 0;
    f.closed = false;
    f.player = assets.hnm(hnm_name(n)).map(HnmPlayer::new);
    hnm_frame(w, assets);
}

/// One frame of the flight HNM, chained through ds:DC02 at the end (LOP splice).
fn hnm_frame(w: &mut World, assets: &mut Assets) {
    let Some(f) = &mut w.flight else { return };
    if f.closed {
        return;
    }
    let done = f.player.as_ref().map(|p| p.done()).unwrap_or(true);
    if done {
        if f.next != f.cur {
            f.cur = f.next;
            f.player = assets.hnm(hnm_name(f.cur)).map(HnmPlayer::new);
        } else if let Some(p) = &mut f.player {
            p.next = 0;
        }
        f.shown = 0;
    }
    if let Some(p) = &mut f.player {
        p.step();
        f.frame.copy_from_slice(&p.fb[..VIEW]);
        f.shown += 1;
    }
    if let Some(ws) = &mut f.worm {
        if f.cur == 1 {
            if let Some(ver) = assets.sheet("VER.HSQ") {
                ws.frame(&mut f.frame, &ver);
            }
        }
    }
    w.dirty = true;
}

/// sub_14988 / 149A0: the small map in (204,4)-(316,60) into the UI buffer, the trail.
fn small_map(w: &mut World, assets: &mut Assets) {
    let (lon, row) = w.cur_pos();
    w.set16(0x197C, lon);
    w.set16(0x197E, row as u16);
    for (k, v) in [204u16, 4, 316, 60].iter().enumerate() {
        w.set16(0x46E3 + 2 * k, *v);
    }
    let Some(mut f) = w.flight.take() else { return };
    w.mem[0x46EB] = 1;
    let mut fb = std::mem::take(&mut f.overlay);
    planet::render_flight(&mut w.mem, &w.map, &mut fb, &w.drv);
    let mut c = 0xFCu8;
    for k in 1..=4 {
        rect(&mut fb, 204 - k, 4 - k, 315 + k, 59 + k, c);
        c -= 2;
    }
    w.draw_icons(assets, &mut fb, 1, (204, 4, 316, 60));
    let ic = assets.sheet("ICONES.HSQ");
    let dest = w.get16(0x11C5) as usize;
    if dest >= 0x100 {
        let (x, y) = planet::map_to_screen(&w.mem, w.get16(dest + 2), w.get16(dest + 4) as i16);
        if (204..316).contains(&x) && (4..60).contains(&y) {
            if let Some(s) = ic.as_ref().and_then(|i| i.sprites.get(0x2E)) {
                s.draw(&mut fb, 320, x - 1, y - 1, DrawOpts::default());
            }
        }
    }
    w.mem[0x46EB] = 0;
    // sub_14A5A: every stored trail point inside (204..313, 4..57) as ICONES 0x2F.
    let n = f.trail.len();
    for k in 0..n {
        let (lon, rw) = f.trail[(f.trail_w + k) % n];
        if (rw >> 8) as u8 == 0x08 {
            continue;
        }
        let (x, y) = planet::map_to_screen(&w.mem, lon, rw as i16);
        let (x, y) = (x - 1, y - 1);
        if (0xCC..0x13A).contains(&x) && (4..0x3A).contains(&y) {
            if let Some(s) = ic.as_ref().and_then(|i| i.sprites.get(0x2F)) {
                s.draw(&mut fb, 320, x, y, DrawOpts::default());
            }
        }
    }
    f.overlay = fb;
    w.flight = Some(f);
}

/// sub_14B3B: one journey step (the clock advances one slot every 16 steps).
fn journey_step(w: &mut World, assets: &mut Assets) {
    let n = w.get16(0x472B).wrapping_add(1);
    w.set16(0x472B, n);
    if n & 0x0F == 0 {
        advance_slots(w, assets, 1);
    }
    let (lon, row) = w.cur_pos();
    let (lon, row) = step_pos(w, lon, row);
    w.set16(4, lon);
    w.set16(6, row as u16);
    w.move_followers();
}

/// The main-loop hook sub_14F0C: HNM step every pass, a journey step every 768 ticks.
pub(crate) fn journey_tick(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    if w.mem[0x4727] == 0 || w.mem[0x11CA] != 0 || w.flight.is_none() {
        return;
    }
    if let Some(f) = &mut w.flight {
        f.frame_acc += 1;
        if f.frame_acc >= ticks_per_frame(f.cur) {
            f.frame_acc = 0;
            hnm_frame(w, assets);
        }
    }
    if (w.now as u16).wrapping_sub(w.get16(0x4729)) < 0x300 {
        return;
    }
    w.set16(0x4729, w.now as u16);
    journey_step(w, assets);
    if std::env::var("DUNE_DEBUG").is_ok() {
        let f = w.flight.as_ref().unwrap();
        eprintln!("step t={} pos={:04x},{} h={:02x} dest={:04x} cur={} next={} shown={} 11CA={}", w.now, w.get16(4), w.mem[6] as i8, w.mem[0x11C7], w.get16(0x11C5), f.cur, f.next, f.shown, w.mem[0x11CA]);
    }
    trail_point(w, assets);
    let (lon, row) = w.cur_pos();
    push_trail(w, lon, row);
    let (di, _) = planet::cell_ptr(&w.mem, lon, row);
    let dest = w.get16(0x11C5) as usize;
    if dest >= 0x100 && di == w.get16(dest + 6) as usize {
        arrival(w, assets, audio, true);
        return;
    }
    flight_events(w, assets);
    if w.mem[0x46D9] != 0 {
        // Shot down (game over 2): the journey ends.
        w.mem[0x4727] = 0;
        w.mem[0x11C9] = 0;
        w.mem[0x11CA] = 0;
        w.flight = None;
        w.scr = Scr::Room;
        w.compass();
        w.build_menu();
        w.dirty = true;
        return;
    }
    if w.mem[0x47A7] != 0 || w.mem[0x11CA] != 0 {
        look_ahead(w);
        return;
    }
    if w.mem[0x4728] & 0x80 == 0 {
        if w.mem[0x4728] == 1 {
            w.mem[0x4728] = 0;
            small_map(w, assets);
        }
        // Current dot (ICONES 0x30), or re-centre next step when it nears the edge.
        for (k, v) in [204u16, 4, 316, 60].iter().enumerate() {
            w.set16(0x46E3 + 2 * k, *v);
        }
        let (x, y) = planet::map_to_screen(&w.mem, lon, row);
        if (204..316).contains(&x) && (4..60).contains(&y) {
            if !(0xD6..0x132).contains(&x) || !(0x0A..0x36).contains(&y) {
                w.mem[0x4728] = 1;
            } else if let (Some(ic), Some(f)) = (assets.sheet("ICONES.HSQ"), &mut w.flight) {
                if let Some(s) = ic.sprites.get(0x30) {
                    s.draw(&mut f.overlay, 320, x - 1, y - 1, DrawOpts::default());
                }
            }
        }
    }
    look_ahead(w);
}

/// sub_14A1A: draw the previous trail point (ICONES 0x2F) into the UI buffer.
fn trail_point(w: &mut World, assets: &mut Assets) {
    if w.mem[0x4728] & 0x80 != 0 {
        return;
    }
    let Some(f) = &w.flight else { return };
    let n = f.trail.len();
    let (lon, rw) = f.trail[(f.trail_w + n - 1) % n];
    if (rw >> 8) as u8 == 0x08 {
        return;
    }
    let (x, y) = planet::map_to_screen(&w.mem, lon, rw as i16);
    if !((204..316).contains(&x) && (4..60).contains(&y)) {
        return;
    }
    if let (Some(ic), Some(f)) = (assets.sheet("ICONES.HSQ"), &mut w.flight) {
        if let Some(s) = ic.sprites.get(0x2F) {
            s.draw(&mut f.overlay, 320, x - 1, y - 1, DrawOpts::default());
        }
    }
}

/// sub_14A00: push the position into the ring cs:E40C..E85C.
fn push_trail(w: &mut World, lon: u16, row: i16) {
    let mut tw = None;
    if let Some(f) = &mut w.flight {
        let n = f.trail.len();
        f.trail[f.trail_w % n] = (lon, row as u16);
        f.trail_w = (f.trail_w + 1) % n;
        tw = Some(f.trail_w);
    }
    if let Some(t) = tw {
        w.set16(0x149A, (0xE40C + 4 * t) as u16);
    }
}

/// loc_14E8E + sub_14EC6: look 6 and 7 steps ahead, queue the next MNT video.
fn look_ahead(w: &mut World) {
    let (mut lon, mut row) = w.cur_pos();
    let frac = w.get16(0x11CC);
    let h = w.mem[0x11C7];
    for _ in 0..6 {
        (lon, row) = step_pos(w, lon, row);
    }
    let c6 = planet::cell(&w.mem, &w.map, lon, row);
    (lon, row) = step_pos(w, lon, row);
    w.set16(0x11CC, frac);
    let c7 = planet::cell(&w.mem, &w.map, lon, row);
    let _ = h;
    let t = c6.wrapping_add(c7) >> 1;
    queue(w, t);
}

fn queue(w: &mut World, terrain: u8) {
    let bx = w.get16(0x487E);
    let Some(cur) = w.flight.as_ref().map(|f| f.cur) else { return };
    let next = if bx < 2 {
        bx
    } else if terrain & 0x0F < 8 {
        if cur <= 2 {
            bx
        } else if cur <= 4 {
            5
        } else {
            w.set16(0x487E, 2);
            2
        }
    } else if cur <= 2 || cur == 5 {
        3
    } else {
        4
    };
    if let Some(f) = &mut w.flight {
        f.next = next;
    }
}

/// sub_135E9 in flight: Harkonnen zone (sub_14182) and the warning when alone.
fn flight_events(w: &mut World, assets: &mut Assets) {
    w.mem[0x47A7] = 0;
    w.mem[0x23] = 0;
    harkonnen_zone(w);
    if w.mem[0x23] == 0 {
        return;
    }
    // Warning (seg000:3641): cockpit, text 0xBF at (0x66,0x4E), pause, menu ds:1F9E.
    let mut view = vec![0u8; 320 * 200];
    if let Some(f) = &w.flight {
        view[..VIEW].copy_from_slice(&f.frame);
    }
    if w.mem[0x2B] == 0 {
        crate::sky::draw_bands(assets, &mut view);
    }
    if let Some(p) = assets.sheet("ORNYPAN.HSQ") {
        for (i, x, y) in [(0usize, 0, 19), (1, 10, 43), (2, 79, 45)] {
            if let Some(s) = p.sprites.get(i) {
                s.draw(&mut view, 320, x, y, DrawOpts::default());
            }
        }
    }
    let t = World::cmd_text(assets, 0xBF);
    crate::world::text_lines(&mut view, &assets.font, 0x66, 0x4E, &t, 0x0C, 0x20);
    w.mem[0x11CA] = 1;
    if let Some(f) = &mut w.flight {
        f.warning = true;
        f.warn_view = view[..VIEW].to_vec();
    }
    w.compass();
    w.build_menu();
    w.dirty = true;
}

/// sub_14182.
fn harkonnen_zone(w: &mut World) {
    if w.mem[0x11C9] & 3 != 1 {
        return;
    }
    let dest = w.get16(0x11C5) as usize;
    let captured = dest >= 0x100 && w.mem[dest + 8] >= 0x28 && w.mem[dest + 0x0A] & 8 != 0;
    let (lon, row) = w.cur_pos();
    if !(w.mem[0x11CB] == 0 && captured) && planet::cell(&w.mem, &w.map, lon, row) & 0x30 == 0x30 {
        if w.mem[0x4726] == 0 {
            w.mem[0x23] = 4;
        }
        w.mem[0x21FD] = 0x40;
        w.mem[0x4726] = w.mem[0x4726].wrapping_sub(0x20);
        if w.mem[0x4726] == 0 {
            w.mem[0x46D9] = 2;
        }
        return;
    }
    w.mem[0x4726] = 0;
    w.mem[0x21FD] = 0;
}

/// Flight menu (sub_12EFB, ds:11C9 & 3) or the warning menu ds:1F9E.
pub(crate) fn flight_menu(w: &mut World) {
    if w.flight.as_ref().map(|f| f.warning).unwrap_or(false) {
        w.menu.push(Line::new(0x58, Cmd::ChangeDest, true));
        w.menu.push(Line::new(0xAE, Cmd::IgnoreWarning, true));
        w.menu.push(Line::new(0x95, Cmd::IgnoreWarning, false));
        return;
    }
    if w.mem[0x11CB] == 0 {
        w.menu.push(Line::new(0xA9, Cmd::Skip, w.mem[0x21FD] & 0x40 == 0));
    } else {
        w.menu.push(Line::new(0xAC, Cmd::BackToStart, true));
        if w.mem[0x2A] >= 0x32 {
            w.menu.push(Line::new(0xAA, Cmd::Nearest, true));
        }
    }
    w.menu.push(Line::new(0x58, Cmd::ChangeDest, true));
}

pub(crate) fn ignore_warning(w: &mut World) {
    if let Some(f) = &mut w.flight {
        f.warning = false;
    }
    w.mem[0x11CA] = 0;
    w.compass();
    w.build_menu();
    w.dirty = true;
}

/// SKIP TO DESTINATION (4FFB).
pub(crate) fn skip(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    if let Some(f) = &mut w.flight {
        f.closed = true;
        f.player = None;
    }
    w.mem[0x11C8] = 0;
    let dest = w.get16(0x11C5) as usize;
    for _ in 0..200 {
        journey_step(w, assets);
        let (lon, row) = w.cur_pos();
        let (di, _) = planet::cell_ptr(&w.mem, lon, row);
        if dest >= 0x100 && di == w.get16(dest + 6) as usize {
            break;
        }
        w.mem[0x23] = 0;
        harkonnen_zone(w);
        if w.mem[0x23] != 0 {
            w.mem[0x4726] = w.mem[0x4726].wrapping_add(0x20);
            if let Some(f) = &mut w.flight {
                f.closed = false;
            }
            open_hnm(w, assets, w.get16(0x487E));
            flight_events(w, assets);
            return;
        }
    }
    arrival(w, assets, audio, false);
}

/// BACK TO STARTING POINT (50A5).
pub(crate) fn back_to_start(w: &mut World, assets: &mut Assets) {
    let di = w.get16(0x1150) as usize;
    if let Some(h) = heading_to(w, di) {
        w.mem[0x11C7] = h;
    }
    w.set16(0x11C5, di as u16);
    w.mem[0x11C8] = 0;
    w.set16(0x11CC, 0x80);
    w.mem[0x11CA] = 0;
    w.mem[0x11CB] = 0;
    let _ = assets;
    w.compass();
    w.build_menu();
    w.dirty = true;
}

/// TOWARDS NEAREST PLACE (50C4): sub_15344.
pub(crate) fn nearest(w: &mut World, assets: &mut Assets) {
    let (lon, row) = w.cur_pos();
    let mut best = (0xFFFFu32, 0usize);
    for i in 0..planet::loc_count(&w.mem) {
        let si = planet::LOCATIONS + planet::LOC_SIZE * i;
        if w.mem[si + 0x0A] & 0x80 != 0 {
            continue;
        }
        let cx = (w.get16(si + 2).wrapping_sub(lon) as i16 as i32).unsigned_abs() >> 8;
        let ax = (w.get16(si + 4) as i16 as i32 - row as i32).unsigned_abs();
        let d = cx.max(ax);
        if d < best.0 {
            best = (d, si);
        }
    }
    if best.1 != 0 {
        set_destination(w, best.1 as u16, (0, 0));
    }
    w.mem[0x11CA] = 0;
    w.mem[0x11C8] = 0;
    w.mem[0x11CB] = 0;
    let _ = assets;
    w.compass();
    w.build_menu();
    w.dirty = true;
}

/// CHANGE DESTINATION (497A): back to the destination screen.
pub(crate) fn change_dest(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    if let Some(f) = &mut w.flight {
        f.warning = false;
    }
    w.mem[0x4728] = 1;
    open_dest(w, assets, audio);
}

/// Zones 13/15 of preset ds:1D72: heading -/+ 4, manual steering.
pub(crate) fn turn(w: &mut World, d: u8) {
    if !w.in_transit() {
        return;
    }
    w.mem[0x11C8] = 1;
    w.mem[0x11C7] = w.mem[0x11C7].wrapping_add(d);
    w.set16(0x11CC, 0x80);
}

/// RESUME FLIGHT (4F09).
pub(crate) fn resume(w: &mut World) {
    w.mem[0x11CA] = 0;
}

pub(crate) fn flight_key(w: &mut World, k: Key) {
    match k {
        Key::Left if w.mem[0x11CB] != 0 => turn(w, 0xFC),
        Key::Right if w.mem[0x11CB] != 0 => turn(w, 4),
        Key::Up => resume(w),
        _ => {}
    }
}

pub(crate) fn flight_compose(w: &mut World, _assets: &mut Assets, fb: &mut [u8]) -> Palette {
    let Some(f) = &w.flight else { return w.palette.clone() };
    if f.warning {
        fb[..VIEW].copy_from_slice(&f.warn_view);
        return f.pal.clone();
    }
    fb[..VIEW].copy_from_slice(&f.frame);
    // sub_14B2B: the small map rect (200,0)-(320,64) from the UI buffer.
    if w.mem[0x4728] & 0x80 == 0 {
        for y in 0..64 {
            let o = y * 320;
            fb[o + 200..o + 320].copy_from_slice(&f.overlay[o + 200..o + 320]);
        }
    }
    f.pal.clone()
}

pub(crate) fn flight_overlays(_w: &mut World, _assets: &mut Assets, _fb: &mut [u8]) {}

/// loc_14FB0 (`landing`) / loc_14FC3 (SKIP): arrival at the destination.
fn arrival(w: &mut World, assets: &mut Assets, audio: &mut Audio, landing: bool) {
    let mode = w.mem[0x11C9];
    if landing {
        w.mem[0x4732] = mode & 1;
    }
    if mode & 1 == 0 {
        if let Some(f) = &mut w.flight {
            f.closed = true;
        }
    }
    w.mem[0xC5] = (w.now as u8).wrapping_mul(0x5B).wrapping_add(w.mem[0xC5]);
    w.mem[0x4727] = 0;
    w.mem[0x11C9] = 0;
    let dest = w.get16(0x11C5) as usize;
    if mode & 3 == 1 && dest >= 0x100 {
        w.mem[dest + 0x15] = w.mem[dest + 0x15].wrapping_add(1);
    }
    w.mem[0x11CA] = 0;
    w.set16(0x11C5, 0);
    if dest < 0x100 {
        return;
    }
    let (lon, row) = (w.get16(dest + 2), w.get16(dest + 4) as i16);
    w.set16(4, lon);
    w.set16(6, row as u16);
    if w.mem[0x4732] & 1 != 0 {
        start_landing(w, assets, audio, dest, lon, row);
        return;
    }
    w.flight = None;
    arrive(w, assets, audio, lon, row);
}

/// sub_1488A: landing at a sietch / palace (SIET / PALACE HNM), fort (FORT.HNM), or the
/// ORNYTK landing for villages and the Harkonnen palace.
fn start_landing(w: &mut World, assets: &mut Assets, audio: &mut Audio, dest: usize, lon: u16, row: i16) {
    let kind = 6 + planet::kind(w.mem[dest + 8]) as u16;
    if kind < 8 {
        w.mem[0x4728] = 0x80;
        let pal = w.flight.as_ref().map(|f| f.pal.clone()).unwrap_or_else(|| w.palette.clone());
        w.seq = Some(Seq::Approach { kind, lon, row, acc: 0, pal });
        return;
    }
    if kind == 9 {
        if let Some(h) = assets.hnm("FORT.HNM") {
            let pal = w.flight.as_ref().map(|f| f.pal.clone()).unwrap_or_else(|| w.palette.clone());
            let p = HnmPlayer::new(h);
            w.seq = Some(Seq::Video { p, n: 9, acc: 0, view: vec![0; VIEW], pal, shown: 0, then: VideoThen::Land(lon, row), full: false });
            return;
        }
    }
    // Villages / Harkonnen palace: the exterior room, sfx 7, ORNYTK frames 0x1F down to 0.
    w.flight = None;
    w.mem[0x4732] = 0;
    arrive(w, assets, audio, lon, row);
    w.mem[0x4731] = 0xFF;
    w.render_room(assets);
    let clean = w.view.clone();
    play_voc(assets, audio, "SN7.VOC");
    let pal = w.palette.clone();
    let mut s = Seq::Orni { p: 0x1F, cl: -1, next: w.now, clean, view: vec![0; VIEW], pal, then: OrniThen::Landed };
    s.orni_frame(w, assets, audio);
    w.seq = Some(s);
    w.effect = None;
}

// ---------------------------------------------------------------------------------------
// Blocking sequences

#[derive(Clone, Copy, PartialEq)]
pub enum OrniThen {
    Fly,
    Landed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VideoThen {
    Fly,
    Land(u16, i16),
}

pub enum Seq {
    /// SUN.HSQ full view and a busy wait (desert steps 20, 36, 52, ...).
    Sun { until: u64, view: Vec<u8>, pal: Option<Palette> },
    /// ORNYTK frames, 20 ticks each (sub_147FB).
    Orni { p: u8, cl: i8, next: u64, clean: Vec<u8>, view: Vec<u8>, pal: Palette, then: OrniThen },
    /// Destination flash: 9 x (20 ticks label, 10 ticks prompt), then the voice end.
    Flash { k: u8, on: bool, next: u64, di: u16, voice_end: u64 },
    /// Landing approach: the flight HNM forced to MNT1 until frame 60/22, then SIET/PALACE.
    Approach { kind: u16, lon: u16, row: i16, acc: u32, pal: Palette },
    /// A video played to the end (FORT, SIET/PALACE tail, VER rising).
    Video { p: HnmPlayer, n: u16, acc: u32, view: Vec<u8>, pal: Palette, shown: usize, then: VideoThen, full: bool },
}

impl Seq {
    pub fn blocks_input(&self) -> bool {
        true
    }
    pub fn blocks_tasks(&self) -> bool {
        !matches!(self, Seq::Flash { .. })
    }
    pub fn view(&self) -> Option<&[u8]> {
        match self {
            Seq::Sun { view, .. } if !view.is_empty() => Some(view),
            Seq::Orni { view, .. } => Some(view),
            Seq::Video { view, full: false, .. } => Some(view),
            _ => None,
        }
    }
    /// A video shown over the whole screen (VER.HNM with ds:227D = 1, at y = 24).
    pub fn full_view(&self) -> Option<(&[u8], &Palette)> {
        match self {
            Seq::Video { view, pal, full: true, .. } => Some((view, pal)),
            _ => None,
        }
    }
    pub fn palette(&self) -> Option<&Palette> {
        match self {
            Seq::Sun { pal, .. } => pal.as_ref(),
            Seq::Orni { pal, .. } => Some(pal),
            Seq::Video { pal, .. } => Some(pal),
            Seq::Approach { pal, .. } => Some(pal),
            _ => None,
        }
    }

    /// Frame routine seg000:4821.
    fn orni_frame(&mut self, w: &mut World, assets: &mut Assets, audio: &mut Audio) {
        let Seq::Orni { p, cl, clean, view, .. } = self else { return };
        view.copy_from_slice(clean);
        let Some(tk) = assets.sheet("ORNYTK.HSQ") else { return };
        let (bx0, by0) = orni_base(w);
        let (mut x, mut y) = (bx0, by0);
        if *p == 0x0D {
            audio.stop();
        }
        let a = *p as i32 - 14;
        if a > 0 {
            x -= 5 * a * (*cl as i32).signum();
            y -= (a * a) / 2;
        }
        draw_orni_parts(view, &tk, x, y, *p);
        let n = w.mem[0x46FF] as i32;
        for k in 1..n {
            draw_orni_parts(view, &tk, bx0 + 0x46 * k, by0 + 0x0A * k, 0);
        }
        w.dirty = true;
    }
}

/// Runs the blocking sequence one tick.
pub(crate) fn seq_tick(w: &mut World, assets: &mut Assets, audio: &mut Audio) {
    let Some(mut s) = w.seq.take() else { return };
    let keep = true;
    match &mut s {
        Seq::Sun { until, view, pal } => {
            if view.is_empty() {
                let mut v = vec![0u8; VIEW];
                if let Some(sun) = assets.sheet("SUN.HSQ") {
                    if let Some(sp) = sun.sprites.first() {
                        sp.draw(&mut v, 320, 0, 0, DrawOpts::default());
                    }
                }
                *view = v;
                *pal = Some(w.palette.clone());
                w.dirty = true;
            }
            if w.now >= *until {
                w.seq = None;
                sun_done(w, assets);
                return;
            }
        }
        Seq::Orni { p, cl, next, then, .. } => {
            if w.now >= *next + 20 {
                *next = w.now;
                *p = p.wrapping_add(*cl as u8);
                if *p == 0x1A && *cl > 0 {
                    audio.stop();
                }
                if *p >= 0x21 {
                    let t = *then;
                    w.seq = None;
                    w.mem[0x4731] = 0;
                    match t {
                        OrniThen::Fly => fly(w, assets),
                        OrniThen::Landed => {
                            w.mem[0x4732] = 0;
                            w.redraw_room(assets);
                        }
                    }
                    return;
                }
                s.orni_frame(w, assets, audio);
            }
        }
        Seq::Flash { k, on, next, di, voice_end } => {
            if w.now >= *next {
                if *k >= 9 {
                    if w.now >= *voice_end || w.now >= *next + 1000 {
                        let di = *di;
                        let click = w.dest.as_ref().map(|d| d.click).unwrap_or((0, 0));
                        w.seq = None;
                        w.mem[0x4732] = 0x80;
                        set_destination(w, di, click);
                        take_off(w, assets, audio);
                        return;
                    }
                } else {
                    let show = if *on { 0 } else { *di };
                    if let Some(d) = &mut w.dest {
                        d.di = show;
                    }
                    label(w, assets);
                    if *on {
                        *next = w.now + 10;
                    } else {
                        *next = w.now + 20;
                        *k += 1;
                        if *k >= 9 {
                            *next = w.now;
                        }
                    }
                    *on = !*on;
                }
            }
        }
        Seq::Approach { kind, lon, row, acc, pal: _ } => {
            *acc += 1;
            let cur = w.flight.as_ref().map(|f| f.cur).unwrap_or(2);
            if *acc >= ticks_per_frame(cur) {
                *acc = 0;
                let stop = if *kind == 6 { 60 } else { 22 };
                let ready = w.flight.as_ref().map(|f| f.cur == 2 && f.shown == stop).unwrap_or(true);
                if ready {
                    let (k, lon, row) = (*kind, *lon, *row);
                    if let Some(h) = assets.hnm(hnm_name(k)) {
                        let pal = w.flight.as_ref().map(|f| f.pal.clone()).unwrap_or_else(|| w.palette.clone());
                        let p = HnmPlayer::new(h);
                        w.seq = Some(Seq::Video { p, n: k, acc: ticks_per_frame(k), view: vec![0; VIEW], pal, shown: 0, then: VideoThen::Land(lon, row), full: false });
                    } else {
                        w.seq = None;
                        land_done(w, assets, audio, lon, row);
                    }
                    return;
                }
                hnm_frame(w, assets);
                queue(w, 0);
                w.dirty = true;
            }
        }
        Seq::Video { p, n, acc, view, shown, then, full, .. } => {
            *acc += 1;
            if *acc >= ticks_per_frame(*n) {
                *acc = 0;
                if p.done() {
                    let t = *then;
                    w.seq = None;
                    match t {
                        VideoThen::Fly => {
                            audio.stop();
                            w.effect = None;
                            fly(w, assets);
                        }
                        VideoThen::Land(lon, row) => land_done(w, assets, audio, lon, row),
                    }
                    return;
                }
                p.step();
                *shown += 1;
                if *n == 0x0E && *shown == 11 {
                    audio.stop();
                }
                if *full {
                    let _ = view;
                } else {
                    view.copy_from_slice(&p.fb[..VIEW]);
                }
                w.dirty = true;
            }
            if *full {
                // VER.HNM with ds:227D = 1: shown at y = 24 over the whole screen.
                if let Seq::Video { p, view, .. } = &mut s {
                    view.fill(0);
                    view[24 * 320..176 * 320].copy_from_slice(&p.fb[..152 * 320]);
                }
            }
        }
    }
    if keep {
        w.seq = Some(s);
    }
}

/// End of a landing video: ds:46E0-- (forces the 0x10 transition), enter the place.
fn land_done(w: &mut World, assets: &mut Assets, audio: &mut Audio, lon: u16, row: i16) {
    w.flight = None;
    w.mem[0x4732] = 0;
    w.mem[0x46E0] = w.mem[0x46E0].wrapping_sub(1);
    w.mem[0x4728] = 0;
    arrive(w, assets, audio, lon, row);
}

// ---------------------------------------------------------------------------------------
// Worm overlay (VER.BIN)

pub struct WormScript {
    data: Vec<u8>,
    start: usize,
    pos: usize,
    off: (i32, i32),
    clip: (i32, i32, i32, i32),
}

impl WormScript {
    /// VER.BIN is loaded at cs:015F; its +8 word is relocated by +0x167 (sub_14285).
    pub fn parse(data: Vec<u8>) -> Option<WormScript> {
        let w = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
        let (x0, y0, x1, y1) = (w(0)?, w(2)?, w(4)?, w(6)?);
        let p167 = w(8)? + 0x167;
        let f = p167.checked_sub(0x15F)?;
        let start = (p167 + w(f)?).checked_sub(0x15F)?;
        Some(WormScript { data, start, pos: start, off: (x0 as i32, y0 as i32), clip: (x0 as i32, y0 as i32, x1 as i32, y1 as i32) })
    }

    /// sub_14D6C: one frame of the script, drawn into the view.
    fn frame(&mut self, fb: &mut [u8], ver: &SpriteSheet) {
        let d = &self.data;
        if d.get(self.pos) == Some(&0xFF) {
            self.pos = self.start;
        }
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 256 {
                break;
            }
            let Some(&b) = d.get(self.pos) else { break };
            self.pos += 1;
            if b == 0 {
                break;
            }
            let mut id = b as usize;
            if b == 1 {
                id = 256 + *d.get(self.pos).unwrap_or(&0) as usize;
                self.pos += 1;
            }
            let Some(rel) = id.checked_sub(2).and_then(|k| d.get(10 + 2 * k..12 + 2 * k)).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize) else { continue };
            let mut g = 10 + rel;
            while let Some(&spr) = d.get(g) {
                if spr == 0 {
                    break;
                }
                let (x, y) = (*d.get(g + 1).unwrap_or(&0) as i32, *d.get(g + 2).unwrap_or(&0) as i32);
                g += 3;
                if let Some(s) = ver.sprites.get(spr as usize - 1) {
                    s.draw(fb, 320, self.off.0 + x, self.off.1 + y, DrawOpts { clip: Some(self.clip), ..Default::default() });
                }
            }
        }
    }
}
