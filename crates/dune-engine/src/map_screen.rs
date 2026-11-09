//! SEE DUNE MAP (seg000:186B toggle, 5A1A enter, 5A56 build, 5A9A redraw) and the map
//! screen input (handler set ds:1A9E). docs/re/travel.md §2.
//!
//! Keyboard equivalents (the original is mouse-only): arrows = scroll zones 13..16,
//! `c` = re-centre (zone 12), `g` = globe button (zone 0), Tab = click the next place icon,
//! Enter = click the current place icon again, Esc = Cancel the popup.

use crate::{
    assets::Assets,
    audio::Audio,
    input::Key,
    panel,
    world::{fill, rect, text, text_lines, Cmd, Line, Scr, World, VIEW},
};
use dune_formats::{palette::Palette, planet, sprite::DrawOpts};

/// The info panel ds:1668 (sub_1600E) drawn next to a selected place.
#[derive(Clone, Default)]
pub struct Popup {
    pub rect: (i32, i32, i32, i32),
    pub rec: usize,
    pub state: u8,
}

/// SEE SPICE DENSITY window (seg000:53F1, sub_1542F): ds:4722 mode, origin ds:11C1/11C3,
/// centre ds:1980/1982.
pub struct Density {
    pub mode: u8,
    pub x: i32,
    pub y: i32,
    pub centre: (u16, i16),
    pub img: Vec<u8>,
    /// Places drawn in the window: (record, x, y).
    pub icons: Vec<(usize, i32, i32)>,
    pub hover: usize,
}

#[derive(Default)]
pub struct MapState {
    /// The map view (frame, terrain, icons, position marker), 320x152.
    pub view: Vec<u8>,
    /// ds:A5C0: {record, x, y} of every drawn place icon.
    pub icons: Vec<(usize, i32, i32)>,
    /// The DUNE MAP message box (ds:DBE0 = 194A).
    pub msgbox: bool,
    pub msgbox_pending: bool,
    /// ds:46F8: selected place (record address).
    pub selected: usize,
    pub popup: Option<Popup>,
    /// GO THERE menu pushed (Some(worm)).
    pub go_menu: Option<bool>,
    pub go_enabled: bool,
    pub pal: Palette,
    pub kb: usize,
}

impl World {
    /// loc_1186B: SEE DUNE MAP / EXIT MAPS toggle on ds:FB.
    pub(crate) fn see_dune_map(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let fb = self.mem[0xFB] as i8;
        let r = fb.wrapping_neg();
        self.mem[0xFB] = r as u8;
        if r < 0 {
            self.map_enter(assets, Some(audio), None);
        } else {
            self.map_leave(assets);
        }
    }

    /// sub_15A1A (from a room) / loc_15A3D (from the globe, `centre` set, effect 2).
    pub(crate) fn map_enter(&mut self, assets: &mut Assets, audio: Option<&mut Audio>, centre: Option<(u16, i16)>) {
        if centre.is_none() {
            self.mem[0x28E7] = 1;
        }
        // sub_118BA: stop sound/voice, pause the journey, no day/night palette.
        if let Some(a) = audio {
            a.stop();
        }
        self.mem[0x11CA] = 1;
        self.mem[0x46DF] = 0;
        self.fade = None;
        self.talk_menu_for = None;
        self.closeup_off();
        let (lon, row) = centre.unwrap_or_else(|| self.cur_pos());
        self.set16(0x197C, lon);
        self.set16(0x197E, row as u16);
        if centre.is_some() {
            self.mem[0xFB] = 0xFF;
        }
        self.scr = Scr::Map;
        self.mapst.selected = 0;
        self.mapst.popup = None;
        self.mapst.go_menu = None;
        self.mapst.msgbox = false;
        self.mapst.msgbox_pending = centre.is_none() && self.mem[0x46F3] == 0;
        // 5A56: rect ds:1482 -> 46E3, frame, ornaments, mode 0x80, redraw, panels.
        for (k, v) in [4u16, 4, 316, 148].iter().enumerate() {
            self.set16(0x46E3 + 2 * k, *v);
        }
        self.mem[0x46EB] = 0x80;
        let mut pal = self.palette.clone();
        if let Some(on) = assets.sheet("ONMAP.HSQ") {
            on.apply_palette(&mut pal);
        }
        self.screen_pal = Some(pal.clone());
        self.mapst.pal = pal;
        self.map_redraw(assets);
        let mem = self.mem.clone();
        self.panel.set_left(&mem, panel::LEFT_MAP);
        self.panel.set_compass(&mem, panel::COMPASS_MAP);
        self.build_menu();
        self.effect = Some(if centre.is_some() { 2 } else { 0x34 });
        self.medallion_to = 10;
        self.dirty = true;
    }

    /// After the entry transition: the message box (sub_15BB0) unless ds:46F3.
    pub(crate) fn map_after_enter(&mut self, _assets: &mut Assets) {
        if std::mem::take(&mut self.mapst.msgbox_pending) {
            self.mapst.msgbox = true;
            self.idle_since = self.now;
            self.dirty = true;
        }
    }

    /// Leave the map: back to the room (or desert) view through effect 0x34.
    pub(crate) fn map_leave(&mut self, assets: &mut Assets) {
        self.map_reset();
        self.mem[0x28E7] = self.mem[0x28E8];
        self.mem[0x11CA] = 0;
        self.panel.set_left(&self.mem.clone(), panel::LEFT_DEFAULT);
        self.effect = Some(0x34);
        self.redraw_room(assets);
        self.medallion_to = 10;
    }

    /// sub_15ADF: reset the map state.
    pub(crate) fn map_reset(&mut self) {
        if !self.tui.menus.is_empty() || self.tui.contact.is_some() {
            self.tui.menus.clear();
            self.contact_end();
        }
        self.unhighlight_all();
        self.mem[0x1954] = 0;
        self.tui.card = None;
        self.tui.objs.clear();
        self.density = None;
        self.prompt = None;
        self.mem[0x46EB] = 0;
        self.mem[0x46F3] = 0;
        self.mapst.icons.clear();
        self.mapst.msgbox = false;
        self.mapst.popup = None;
        self.mapst.go_menu = None;
        self.mapst.selected = 0;
    }

    pub(crate) fn closeup_off(&mut self) {
        // Talking faces end when a map / globe opens.
    }

    /// sub_15A9A: terrain, place icons, Paul's position marker, frame.
    pub(crate) fn map_redraw(&mut self, assets: &mut Assets) {
        let mut fb = vec![0u8; 320 * 200];
        planet::render_map(&mut self.mem, &self.map, &mut fb, &self.drv);
        // sub_15B69: 4 rectangles in FC, FA, F8, F6 around (4,4)-(316,148).
        let (x0, y0, x1, y1) = (4, 4, 316, 148);
        let mut c = 0xFCu8;
        for k in 1..=4 {
            rect(&mut fb, x0 - k, y0 - k, x1 - 1 + k, y1 - 1 + k, c);
            c -= 2;
        }
        if self.mem[0x46EB] & 0x80 != 0 {
            self.draw_vegetation(assets, &mut fb);
        }
        self.draw_icons(assets, &mut fb, 0x80, (x0, y0, x1, y1));
        // sub_16314: Paul's position, ICONES 0x4C at (x - 13, y - h).
        let (lon, row) = self.cur_pos();
        let (x, y) = planet::map_to_screen(&self.mem, lon, row);
        if x >= x0 && x < x1 && y >= y0 && y < y1 {
            if let Some(ic) = assets.sheet("ICONES.HSQ") {
                if let Some(s) = ic.sprites.get(0x4C) {
                    let yy = (y as u8).wrapping_sub(s.height as u8) as i32;
                    s.draw(&mut fb, 320, x - 13, yy, DrawOpts { clip: Some((x0, y0, x1, y1)), ..Default::default() });
                }
            }
        }
        self.mapst.view = fb[..VIEW].to_vec();
        let keep = self.mapst.popup.as_ref().map(|p| p.rec);
        self.mapst.popup = None;
        self.build_troop_objs(assets);
        if let Some(rec) = keep {
            self.map_info(assets, rec);
        }
        if self.density.is_some() {
            self.density_render(assets);
        }
        self.dirty = true;
    }

    /// sub_15DCE: icons of the discovered places inside the view rect, list ds:A5C0.
    /// `mode` = ds:46EB (0x80 map: ONMAP 0x7A+kind; 1 flight: ICONES 0x3A+kind).
    pub(crate) fn draw_icons(&mut self, assets: &mut Assets, fb: &mut [u8], mode: u8, r: (i32, i32, i32, i32)) {
        let bank = if mode & 0x80 != 0 { assets.sheet("ONMAP.HSQ") } else { assets.sheet("ICONES.HSQ") };
        let Some(bank) = bank else { return };
        let mut icons = vec![];
        for i in 0..planet::loc_count(&self.mem) {
            let rec = planet::LOCATIONS + planet::LOC_SIZE * i;
            if self.mem[rec + 0x0A] & 0x80 != 0 {
                continue;
            }
            let (lon, row) = (self.get16(rec + 2), self.get16(rec + 4) as i16);
            let (x, y) = planet::map_to_screen(&self.mem, lon, row);
            if !(x >= r.0 && x < r.2 && y >= r.1 && y < r.3) {
                continue;
            }
            icons.push((rec, x, y));
            let app = self.mem[rec + 8];
            let mut spr = if mode & 0x80 != 0 { 0x7A } else { 0x3A } + planet::kind(app);
            if app < 0x20 && self.distance(rec) > self.get16(0x1176) {
                spr += 5;
            }
            if let Some(s) = bank.sprites.get(spr) {
                let (w, h) = (s.width as i32, s.height as i32);
                s.draw(fb, 320, x - (w >> 1), y - (h >> 1), DrawOpts { clip: Some(r), ..Default::default() });
            }
        }
        self.mapst.icons = icons;
    }

    /// sub_17C8F: distance in cells from Paul to a place, max(|dlon| / units, |drow|).
    pub(crate) fn distance(&self, rec: usize) -> u16 {
        let (lon, row) = self.cur_pos();
        let units = self.get16(planet::LON_UNITS + 2 * (row as i32).unsigned_abs().min(98) as usize).max(1);
        let d = (self.get16(rec + 2).wrapping_sub(lon) as i16 as i32).unsigned_abs() as u16 / units;
        let dr = (row.wrapping_sub(self.get16(rec + 4) as i16) as i32).unsigned_abs() as u16;
        d.max(dr)
    }

    /// sub_1878C: the maps menu ds:20F2 (or the GO THERE popup menus 20DA / 20E6).
    pub(crate) fn map_menu(&mut self) {
        if let Some(lines) = self.orders_lines() {
            self.menu = lines;
            return;
        }
        if let Some(worm) = self.mapst.go_menu {
            if worm {
                self.menu.push(Line::new(0x5A, Cmd::GoThereWorm, true));
            } else {
                self.menu.push(Line::new(0x59, Cmd::GoThereOrni, self.mapst.go_enabled));
            }
            self.menu.push(Line::new(0xA3, Cmd::CancelPopup, true));
            return;
        }
        self.menu.push(Line::new(0x63, Cmd::ExitMaps, true));
        if self.get16(0x1176) < 2 {
            let rec = self.get16(0x114E) as usize;
            let ok = rec >= 0x100 && self.sim().troop_ids(rec).iter().any(|&t| self.mem[t + 3] < 0x80);
            self.menu.push(Line::new(0x4093 & 0x3FFF, Cmd::GiveOrders, ok));
        } else {
            self.menu.push(Line::new(0x62, Cmd::ContactTroops, true));
        }
        self.menu.push(Line::new(0x64, Cmd::SpiceDensity, self.mem[0x2A] >= 5));
        let rec = self.get16(0x114E) as usize;
        let mut orni = false;
        if self.mem[8] != 0xFF && (self.mem[8] < 0x20 || self.mem[0xB] <= 2) {
            self.equipment_here(rec);
            orni = self.mem[0x46FF] != 0;
        }
        self.menu.push(Line::new(0xA7, Cmd::TakeOrniMaps, orni));
        if self.mem[0x2A] >= 5 {
            self.menu.push(Line::new(0x67, Cmd::FindProspectors, true));
        }
    }

    pub(crate) fn map_tick(&mut self, _assets: &mut Assets) {
        // sub_15C03: the message box goes after 1000 idle ticks.
        if self.mapst.msgbox && self.now.saturating_sub(self.idle_since) >= 0x3E8 {
            self.mapst.msgbox = false;
            self.dirty = true;
        }
    }

    /// Compass zones 13..16 (seg000:881F..882E).
    pub(crate) fn map_scroll(&mut self, assets: &mut Assets, h: u16) {
        if self.scr != Scr::Map {
            return;
        }
        match h {
            0x8829 => self.set16(0x197E, self.get16(0x197E).wrapping_add(0xFFF4)),
            0x882E => self.set16(0x197E, self.get16(0x197E).wrapping_add(12)),
            0x8824 => self.set16(0x197C, self.get16(0x197C).wrapping_add(0x1002)),
            _ => self.set16(0x197C, self.get16(0x197C).wrapping_add(0xEFFE)),
        }
        self.map_deselect();
        self.map_redraw(assets);
    }

    /// Zone 12 (seg000:5B05): re-centre on the current position.
    pub(crate) fn map_recentre(&mut self, assets: &mut Assets) {
        if self.scr != Scr::Map {
            return;
        }
        let (lon, row) = self.cur_pos();
        self.set16(0x197C, lon);
        self.set16(0x197E, row as u16);
        self.map_deselect();
        self.map_redraw(assets);
    }

    /// sub_15E6D: nearest icon by Manhattan distance (returns record, distance).
    fn nearest_icon(&self, x: i32, y: i32, max_app: u8) -> Option<(usize, i32)> {
        let mut best: Option<(usize, i32)> = None;
        for &(rec, ix, iy) in &self.mapst.icons {
            if self.mem[rec + 8] >= max_app {
                continue;
            }
            let d = (ix - x).abs() + (iy - y).abs();
            if best.map(|b| d < b.1).unwrap_or(true) {
                best = Some((rec, d));
            }
        }
        best
    }

    pub(crate) fn map_mouse(&mut self, assets: &mut Assets, audio: &mut Audio, x: i32, y: i32, clicked: bool, right: bool) {
        if right && y < 152 {
            // 5CE4: right button on a troop icon -> troop card, elsewhere closes it.
            match self.troop_hit(x, y) {
                Some(si) => {
                    self.tui.card = None;
                    self.troop_card(si)
                }
                None => {
                    self.tui.card = None;
                    self.dirty = true;
                }
            }
            return;
        }
        if clicked && y < 152 {
            if self.equipment_click(x, y) {
                return;
            }
            if self.density_click(assets, x, y) {
                return;
            }
            if self.talk.is_some() {
                return;
            }
            // 5C76: a troop icon -> contact (loc_1872C).
            if matches!(self.tui.menus.last(), None | Some(crate::troop_ui::OrdMenu::Orders) | Some(crate::troop_ui::OrdMenu::Far) | Some(crate::troop_ui::OrdMenu::Moving)) {
                if let Some(si) = self.troop_hit(x, y) {
                    self.mapst.msgbox = false;
                    let id = self.mem[si];
                    if self.get16(0x1176) >= 2 || self.get16(si + 4) == self.get16(0x114E) {
                        if id != self.mem[0x1954] {
                            self.mem[0x1954] = id;
                            self.troop_select(assets, audio);
                        } else {
                            self.contact_talk(assets, audio);
                        }
                    }
                    return;
                }
            }
            // 5C76: a click on empty map while a troop is contacted ends the orders.
            let on_icon = self.mapst.icons.iter().any(|&(_, ix, iy)| (ix - x).abs() + (iy - y).abs() < 0x14);
            if !on_icon && self.mem[0x1954] != 0 && matches!(self.tui.menus.last(), Some(crate::troop_ui::OrdMenu::Orders) | Some(crate::troop_ui::OrdMenu::Far) | Some(crate::troop_ui::OrdMenu::Moving)) {
                self.no_more_orders(assets, audio);
                return;
            }
            self.map_click(assets, x, y);
            if let Some(crate::troop_ui::OrdMenu::MoveWait { .. }) = self.tui.menus.last() {
                let sel = self.mapst.selected;
                self.map_deselect();
                if sel != 0 {
                    self.move_click(assets, audio, sel);
                }
            }
        }
    }

    fn unhighlight_all(&mut self) {
        self.tui.objs.retain(|o| o.flags & 0x40 == 0);
    }

    /// Left click 5C76.
    pub(crate) fn map_click(&mut self, assets: &mut Assets, x: i32, y: i32) {
        if self.mapst.msgbox {
            self.mapst.msgbox = false;
            self.dirty = true;
        }
        if let Some(p) = &self.mapst.popup {
            let (x0, y0, x1, y1) = p.rect;
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                return;
            }
        }
        match self.nearest_icon(x, y, 0x31) {
            Some((rec, d)) if d < 0x14 && rec != self.mapst.selected => self.map_select(assets, rec),
            _ => self.map_deselect(),
        }
    }

    /// loc_15FB0: select a place -> GO THERE popup menu, or the info panel only.
    fn map_select(&mut self, assets: &mut Assets, rec: usize) {
        self.map_deselect();
        let cur = self.get16(0x114E) as usize;
        let mut menu = None;
        if rec != cur && self.tui.menus.is_empty() {
            if self.mem[8] == 0xFF {
                if self.mem[0x0A] & 0x40 != 0 {
                    menu = Some(true);
                }
            } else if self.mem[0xB] <= 2 || self.mem[8] < 0x20 {
                self.equipment_here(cur);
                self.mapst.go_enabled = self.mem[0x46FF] != 0;
                menu = Some(false);
            }
        }
        self.map_info(assets, rec);
        if menu.is_some() {
            self.mapst.go_menu = menu;
            self.build_menu();
        }
        self.dirty = true;
    }

    /// sub_15F79 / sub_15F91: deselect, close the popup menu and the info panel.
    pub(crate) fn map_deselect(&mut self) {
        self.mapst.selected = 0;
        self.mapst.popup = None;
        if self.mapst.go_menu.take().is_some() {
            self.build_menu();
        }
        self.dirty = true;
    }

    pub(crate) fn map_cancel_popup(&mut self) {
        self.map_deselect();
    }

    /// sub_15EE4 + sub_1600E: info panel geometry and state.
    pub(crate) fn map_info(&mut self, _assets: &mut Assets, rec: usize) {
        let Some(&(_, x, y)) = self.mapst.icons.iter().find(|i| i.0 == rec) else { return };
        let state = self.place_state(rec);
        let h = [0x58, 0x3C, 0x1E][state as usize];
        let mut by = y - h / 2;
        if by < 4 {
            by = 4;
        }
        if by >= 0x94 - h {
            by = 0x94 - h;
        }
        let mut dx = x + 0x0F;
        if dx >= 0xD2 {
            dx -= 0x82;
        }
        self.mapst.selected = rec;
        self.mapst.popup = Some(Popup { rect: (dx, by, dx + 0x6A, by + h), rec, state });
    }

    /// sub_1627E: battle view.
    fn battle_view(&mut self, rec: usize) -> bool {
        if self.mem[rec + 0x0A] & 2 != 0 {
            return true;
        }
        !self.sim().friendly(rec) && self.sim().sides(rec).1 != 0
    }
    /// sub_16252: 0 water + equipment, 1 equipment (or battle gauge), 2 name only.
    fn place_state(&mut self, rec: usize) -> u8 {
        if self.battle_view(rec) {
            return 1;
        }
        if !self.sim().friendly(rec) || self.mem[rec + 0x0A] & 0x10 == 0 {
            return 2;
        }
        match planet::kind(self.mem[rec + 8]) {
            0 | 3 => 0,
            1 => 2,
            _ => 1,
        }
    }

    pub(crate) fn map_compose(&mut self, assets: &mut Assets, fb: &mut [u8]) -> Palette {
        if self.mapst.view.len() == VIEW {
            fb[..VIEW].copy_from_slice(&self.mapst.view);
        }
        self.draw_troop_objs(assets, fb);
        self.mapst.pal.clone()
    }

    /// Message box and info panel, drawn on the screen over the map and panel.
    pub(crate) fn map_overlays(&mut self, assets: &mut Assets, fb: &mut [u8]) {
        if let Some(d) = &self.density {
            for y in d.y.max(0)..(d.y + 0x6C).min(152) {
                for x in d.x.max(0)..(d.x + 0xAA).min(320) {
                    let p = (y * 320 + x) as usize;
                    if p < d.img.len() {
                        fb[p] = d.img[p];
                    }
                }
            }
        }
        if self.mapst.msgbox {
            // ds:194A = (10,10)-(190,64), fill [194A+9], outline [194A+8], big font F0.
            let (x0, y0, x1, y1) = (10, 10, 190, 64);
            fill(fb, x0, y0, x1, y1, self.mem[0x1953]);
            rect(fb, x0, y0, x1 - 1, y1 - 1, self.mem[0x1952]);
            let mut t = World::cmd_text(assets, 0xE2);
            // e2e3: the rallied-troop count over the last 3 digits.
            if let Some(end) = t.iter().rposition(|b| b.is_ascii_digit()) {
                let n = self.mem[0x28] as u32;
                let s = format!("{n:>3}");
                if end >= 2 {
                    t[end - 2..=end].copy_from_slice(s.as_bytes());
                }
            }
            text_lines(fb, &assets.font, x0 + 10, y0 + 8, &t, 0xF0, 0);
        }
        if let Some(p) = self.mapst.popup.clone() {
            let (x0, y0, x1, y1) = p.rect;
            let bg = self.mem[0x1671];
            fill(fb, x0, y0, x1, y1, bg);
            rect(fb, x0, y0, x1 - 1, y1 - 1, self.mem[0x1670]);
            let font = &assets.font;
            let ty = self.place_type_text(assets, p.rec);
            text(fb, font, x0 + 12, y0 + 4, &ty, 0x9A, bg);
            let name = self.place_name(assets, p.rec);
            text(fb, font, x0 + 4, y0 + 13, &name, 0x96, bg);
            let onmap = assets.sheet("ONMAP.HSQ");
            let small = &assets.font2;
            let mut y = y0 + 0x0D;
            if p.state == 0 && self.mem[0x0A] & 0x20 != 0 {
                y += 0x0A;
                text(fb, small, x0 + 4, y, &World::cmd_text(assets, 0x6C), 0x90, bg);
                y += 7;
                if self.mem[p.rec + 0x0A] & 0x20 != 0 {
                    let v = self.mem[p.rec + 0x1B] as u32;
                    let s = format!("{:>3}", v);
                    text(fb, small, x0 + 0x20, y - 7, s.as_bytes(), 0x90, bg);
                    // sub_1617A: +1B copies of ONMAP 0x75 from x + 4 to the right edge.
                    if let Some(on) = &onmap {
                        if let Some(sp) = on.sprites.get(0x75) {
                            let (w, n0) = (sp.width as i32 + 2, v as i32);
                            let avail = x1 - (x0 + 4);
                            let (mut step, mut n) = (w, n0);
                            if n > 0 && avail / n < w {
                                step = (avail - w) / n;
                                if step < 2 {
                                    step = 2;
                                    n = (avail - w) / 2;
                                }
                            }
                            for i in 0..n.max(0) {
                                sp.draw(fb, 320, x0 + 4 + i * step, y, DrawOpts::default());
                            }
                        }
                    }
                } else {
                    text(fb, small, x0 + 0x0A, y, &World::cmd_text(assets, 0x6D), 0x90, bg);
                }
            }
            if p.state != 2 {
                y += 0x0C;
                if self.battle_view(p.rec) {
                    text(fb, small, x0 + 4, y, &World::cmd_text(assets, 0x4C), 0x9A, bg);
                    // sub_160AC: the gauge sprite (the 8-bit wrap of sub_160F8 is kept).
                    let r = self.sim().gauge(p.rec);
                    let spr = 0x8E + ((r as u32 + 15) >> 5) as usize;
                    if let Some(on) = &onmap {
                        if let Some(sp) = on.sprites.get(spr) {
                            sp.draw(fb, 320, x0 + 0x2F, y + 6, DrawOpts::default());
                        }
                    }
                } else {
                    text(fb, small, x0 + 4, y, &World::cmd_text(assets, 0x6E), 0x9A, bg);
                    let items: Vec<u8> = (0..7).map(|i| self.mem[p.rec + 0x14 + i]).collect();
                    self.place_equipment(assets, fb, &items, x0 + 4, y + 8, y1, x1);
                }
            }
        }
    }

    pub(crate) fn map_key(&mut self, assets: &mut Assets, audio: &mut Audio, k: Key) {
        match k {
            // Keyboard equivalents of the troop icons: `t` left-clicks the next own troop,
            // `r` right-clicks it (troop card), `f` clicks the density window footer.
            Key::Char('t') | Key::Char('r') => {
                let own: Vec<(i32, i32)> = self.tui.objs.iter().filter(|o| o.flags & 0x40 == 0 && self.mem[o.owner + 3] < 0x80).map(|o| ((o.x0 + o.x1) / 2, (o.y0 + o.y1) / 2)).collect();
                if own.is_empty() {
                    return;
                }
                self.tui.kb = (self.tui.kb + 1) % own.len();
                let (x, y) = own[self.tui.kb];
                self.map_mouse(assets, audio, x, y, k == Key::Char('t'), k == Key::Char('r'));
            }
            Key::Char('f') => {
                if let Some(d) = &self.density {
                    let (x, y) = (d.x + 40, d.y + 0x64);
                    self.density_click(assets, x, y);
                }
            }
            Key::Up => self.map_scroll(assets, 0x8829),
            Key::Down => self.map_scroll(assets, 0x882E),
            Key::Right => self.map_scroll(assets, 0x8824),
            Key::Left => self.map_scroll(assets, 0x881F),
            Key::Char('c') => self.map_recentre(assets),
            Key::Char('g') => self.globe_enter(assets),
            Key::Tab => {
                if self.mapst.icons.is_empty() {
                    return;
                }
                self.mapst.kb = (self.mapst.kb + 1) % self.mapst.icons.len();
                let (_, x, y) = self.mapst.icons[self.mapst.kb];
                self.map_click(assets, x, y);
            }
            Key::Enter => {
                if let Some(&(_, x, y)) = self.mapst.icons.get(self.mapst.kb) {
                    self.map_click(assets, x, y);
                }
            }
            Key::Esc => self.map_deselect(),
            _ => {}
        }
    }
}

impl World {
    /// Place equipment columns in the info panel (sub_17E3D, horizontal icon rows).
    fn place_equipment(&self, assets: &mut Assets, fb: &mut [u8], counts: &[u8], x: i32, y: i32, bottom: i32, right: i32) {
        if counts.iter().all(|&c| c == 0) {
            text(fb, &assets.font2, x + 0x0C, y + 5, &World::cmd_text(assets, 0x69), 0x96, 0);
            return;
        }
        let Some(on) = assets.sheet("ONMAP.HSQ") else { return };
        let mut dx = x;
        for (k, &n) in counts.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let spr = self.mem[0x192F + k] as usize;
            let Some(s) = on.sprites.get(spr) else { continue };
            let (w, h) = (s.width as i32 & 0xFFF, s.height as i32);
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
                s.draw(fb, 320, dx, y + i * step, DrawOpts { clip: Some((0, 0, right, bottom)), ..Default::default() });
            }
            dx += w + 1;
        }
    }

    // ------------------------------------------------------------------ SEE SPICE DENSITY

    /// 53F1: open / close the density window.
    pub(crate) fn density_toggle(&mut self, assets: &mut Assets) {
        if self.density.take().is_some() {
            self.mem[0x46EB] &= !0x40;
            self.dirty = true;
            return;
        }
        self.mapst.msgbox = false;
        self.map_deselect();
        self.tui.card = None;
        let (x, y) = (self.get16(0x11C1) as i32, self.get16(0x11C3) as i32);
        let centre = (self.get16(0x197C), self.get16(0x197E) as i16);
        self.density = Some(Density { mode: 0, x, y, centre, img: vec![], icons: vec![], hover: 0 });
        self.density_render(assets);
    }

    /// sub_157E5: colour table of the regions.
    fn density_table(&mut self, mode: u8) -> [u8; 256] {
        let mut t = [0x70u8; 256];
        let mut di = 0x100;
        while self.mem[di] != 0xFF {
            if self.mem[di + 0x0A] & 0x80 == 0 {
                let reg = self.mem[di + 0x10] as usize;
                if mode == 0 {
                    t[reg] = if self.mem[di + 0x0A] & 0x40 != 0 {
                        0x50 + (self.mem[di + 0x12] >> 4)
                    } else if self.mem[0x2942] & 2 != 0 {
                        0x78
                    } else {
                        0x75
                    };
                } else {
                    let (bl, ch, cl) = self.place_occupation(di);
                    t[reg] = occupation_pattern(bl, ch, cl);
                }
            }
            di += 0x1C;
        }
        t
    }
    /// Callback seg000:5728 over sub_16639: troops per class (spice, military, ecology).
    fn place_occupation(&mut self, di: usize) -> (u8, u8, u8) {
        let (mut bl, mut ch, mut cl) = (0u8, 0u8, 0u8);
        self.sim().walk_with_moving(di, |s, si| {
            if s.b(si + 3) >= 0x80 {
                return;
            }
            let job = s.b(si + 3) & 0x0F;
            if job == 2 {
                return;
            }
            match job >> 2 {
                0 => bl += 1,
                1 => ch += 1,
                _ => cl += 1,
            }
        });
        (bl, ch, cl)
    }

    /// sub_1542F: render the window (region map, icons, footer, view rectangle).
    pub(crate) fn density_render(&mut self, assets: &mut Assets) {
        let Some(d) = self.density.as_ref() else { return };
        let (wx, wy, mode, (lon_c, mut row_c)) = (d.x, d.y, d.mode, d.centre);
        let mut img = vec![0u8; 320 * 152];
        if let Some(on) = assets.sheet("ONMAP.HSQ") {
            if let Some(s) = on.sprites.get(0x8D) {
                s.draw(&mut img, 320, wx, wy, DrawOpts::default());
            }
        }
        // sub_1B6C3, small-map branch: W = 160, H = 89 rows of MAP2.
        let (w, h) = (160i32, 89i32);
        let half = (h - 1) / 2;
        let lim = (0x56 - half) as i16;
        if row_c.abs() >= lim {
            row_c = if row_c < 0 { -lim } else { lim };
        }
        let start = row_c as i32 - half;
        let mut buf = vec![0u8; 200 * (h as usize + 1)];
        for k in 0..h {
            let r = start + k;
            let ra = r.unsigned_abs().min(98) as usize;
            let mut off = self.get16(planet::TABLAT + 8 * ra);
            if r < 0 {
                off = off.wrapping_neg();
            }
            let bx = self.get16(planet::TABLAT + 8 * ra + 2) as i32 * 2;
            if bx == 0 {
                continue;
            }
            let base = (planet::MAP_BASE as u16).wrapping_add(off) as usize;
            let c = ((lon_c as u32 * bx as u32) >> 16) as i32;
            let (mut di, mut dx) = (k as usize * 200, w);
            if bx < dx {
                di += ((dx - bx) / 2) as usize;
                dx = bx;
            }
            let mut ax = c - dx / 2;
            if ax < 0 {
                ax += bx;
            }
            for i in 0..dx {
                let col = (ax + i) % bx;
                buf[di + i as usize] = self.map2.get(base + col as usize).copied().unwrap_or(0);
            }
        }
        // DNVGA cs:2441.
        let mut table = self.density_table(mode);
        const POLE: [i32; 18] = [2, 4, 6, 8, 11, 14, 16, 19, 22, 25, 28, 32, 36, 41, 47, 54, 65, 75];
        let (ox, oy) = (wx + 5, wy + 7);
        for k in 0..h {
            let r = start + k;
            let ar = r.abs();
            let row = &buf[k as usize * 200..];
            let below = |i: usize| buf.get((k as usize + 1) * 200 + i).copied().unwrap_or(0);
            let put = |img: &mut [u8], x: i32, c: u8| {
                let p = ((oy + k) * 320 + ox + x) as usize;
                if p < img.len() {
                    img[p] = c;
                }
            };
            let (mut s0, mut n, mut kk) = (0i32, w, 0i32);
            if ar >= 70 {
                kk = POLE[(ar - 70).min(17) as usize];
                s0 = kk;
                n = w - 2 * kk;
                let left: Vec<u8> = if kk >= 4 { std::iter::repeat(0).take((kk - 4) as usize).chain([0x1C, 0x19, 0x18, 0x17]).collect() } else { vec![0x18, 0x17] };
                if n < 0 {
                    for x in 0..w {
                        put(&mut img, x, 0);
                    }
                    continue;
                }
                for (i, &c) in left.iter().enumerate() {
                    put(&mut img, i as i32, c);
                    put(&mut img, w - 1 - i as i32, c);
                }
            }
            for i in 0..n {
                let si = (s0 + i) as usize;
                let c = row[si];
                let same = if i < n - 1 { c == row[si + 1] && c == below(si) } else { c == below(si) };
                let v = if same { table[c as usize] } else { 0x70 };
                let px = if mode == 0 { v } else if v == 0x70 { 0x70 } else { 0x71 + (v & 3) };
                put(&mut img, kk + i, px);
            }
            if mode != 0 {
                for t in table.iter_mut() {
                    if *t != 0x70 {
                        *t = t.rotate_left(2);
                    }
                }
            }
        }
        // Projection sub_1B647 (cl = 0) and the place icons (ICONES 0x3A + kind).
        let (dcf6, dcf8) = (wx + 5 + w / 2, wy + 7 + half);
        let proj = |m: &[u8], lon: u16, row: i16| -> (i32, i32) {
            let ra = (row as i32).unsigned_abs().min(98) as usize;
            let l = u16::from_le_bytes([m[planet::TABLAT + 8 * ra + 2], m[planet::TABLAT + 8 * ra + 3]]).wrapping_mul(2) as i16 as i32;
            let dl = lon.wrapping_sub(lon_c) as i16 as i32;
            (((dl * l) >> 16) + dcf6, row as i32 - row_c as i32 + dcf8)
        };
        let clip = (wx + 5, wy + 7, wx + 0xA5, wy + 0x60);
        let mut icons = vec![];
        if let Some(ic) = assets.sheet("ICONES.HSQ") {
            for i in 0..planet::loc_count(&self.mem) {
                let rec = planet::LOCATIONS + planet::LOC_SIZE * i;
                if self.mem[rec + 0x0A] & 0x80 != 0 {
                    continue;
                }
                let (x, y) = proj(&self.mem, self.get16(rec + 2), self.get16(rec + 4) as i16);
                if !(x >= clip.0 && x < clip.2 && y >= clip.1 && y < clip.3) {
                    continue;
                }
                icons.push((rec, x, y));
                let app = self.mem[rec + 8];
                let mut spr = 0x3A + planet::kind(app);
                if app < 0x20 && self.distance(rec) > self.get16(0x1176) {
                    spr += 5;
                }
                if let Some(s) = ic.sprites.get(spr) {
                    let (sw, sh) = (s.width as i32, s.height as i32);
                    s.draw(&mut img, 320, x - (sw >> 1), y - (sh >> 1), DrawOpts { clip: Some(clip), ..Default::default() });
                }
            }
            // sub_162F2: the current troop (or place) marker ICONES 0x36.
            let id = self.mem[0x1954];
            if id != 0 {
                let si = crate::sim::troop_addr(id);
                let (lon, row) = if self.mem[si + 3] & 0x40 != 0 {
                    (self.get16(si + 6), self.get16(si + 8) as i16)
                } else {
                    let p = self.get16(si + 4) as usize;
                    (self.get16(p + 2), self.get16(p + 4) as i16)
                };
                let (x, y) = proj(&self.mem, lon, row);
                if let Some(s) = ic.sprites.get(0x36) {
                    s.draw(&mut img, 320, x - 13, y - s.height as i32, DrawOpts { clip: Some(clip), ..Default::default() });
                }
            }
        }
        // Footer (sub_15605 / sub_1563E).
        fill(&mut img, wx + 6, wy + 0x62, wx + 0xA5, wy + 0x6B, 0xF5);
        let label = World::cmd_text(assets, if mode == 0 { 0x65 } else { 0x68 });
        let hover = self.density.as_ref().map(|d| d.hover).unwrap_or(0);
        if hover != 0 {
            let mut t = self.place_type_text(assets, hover);
            t.extend(self.place_name(assets, hover));
            text(&mut img, &assets.font2, wx + 6, wy + 0x62, &t, 0xFE, 0xF5);
        } else {
            text(&mut img, &assets.font2, wx + 6, wy + 0x62, &label, 0xFE, 0xF5);
            if mode == 0 {
                text(&mut img, &assets.font2, wx + 6 + 0x53, wy + 0x62, b"-", 0xFE, 0xF5);
                text(&mut img, &assets.font2, wx + 6 + 0x53 + 0x41, wy + 0x62, b"+", 0xFE, 0xF5);
                if let Some(on) = assets.sheet("ONMAP.HSQ") {
                    for (spr, x) in [(0x80usize, wx + 0x5F), (0x81, wx + 0x9B)] {
                        if let Some(s) = on.sprites.get(spr) {
                            s.draw(&mut img, 320, x, wy + 0x63, DrawOpts::default());
                        }
                    }
                }
            }
        }
        // The main-map area (80 x 40 cells), dashed 0x5555, colour 0xFB.
        let (cx, cy) = proj(&self.mem, self.get16(0x197C), self.get16(0x197E) as i16);
        let (rx0, ry0) = ((cx - 0x28).clamp(wx + 5, wx + 5 + 0x9F), (cy - 0x14).clamp(wy + 7, wy + 7 + 0x58));
        let (rx1, ry1) = ((cx + 0x27).clamp(wx + 5, wx + 5 + 0x9F), (cy + 0x13).clamp(wy + 7, wy + 7 + 0x58));
        if rx1 > rx0 && ry1 > ry0 {
            let mut pat = 0x5555u16;
            let mut dot = |img: &mut [u8], x: i32, y: i32| {
                pat = pat.rotate_left(1);
                if pat & 1 != 0 {
                    img[(y * 320 + x) as usize] = 0xFB;
                }
            };
            for x in rx0..=rx1 {
                dot(&mut img, x, ry0);
            }
            for y in ry0..=ry1 {
                dot(&mut img, rx1, y);
            }
            for x in (rx0..=rx1).rev() {
                dot(&mut img, x, ry1);
            }
            for y in (ry0..=ry1).rev() {
                dot(&mut img, rx0, y);
            }
        }
        self.mem[0x46EB] = 0xC0;
        if let Some(d) = &mut self.density {
            d.img = img;
            d.icons = icons;
        }
        self.dirty = true;
    }

    /// Mouse in the window (loc_15923): close box, footer (mode toggle), a place.
    pub(crate) fn density_click(&mut self, assets: &mut Assets, x: i32, y: i32) -> bool {
        let Some(d) = &self.density else { return false };
        let (wx, wy) = (d.x, d.y);
        if !(x >= wx && x < wx + 0xAA && y >= wy && y < wy + 0x6C) {
            return false;
        }
        if y < wy + 7 {
            if x < wx + 10 {
                self.density_toggle(assets);
            }
            return true;
        }
        if y >= wy + 7 + 0x59 {
            if let Some(d) = &mut self.density {
                d.mode ^= 0xFF;
                d.mode &= 1;
            }
            self.density_render(assets);
            return true;
        }
        let best = d.icons.iter().map(|&(r, ix, iy)| (r, (ix - x).abs() + (iy - y).abs())).min_by_key(|v| v.1);
        if let Some((rec, dd)) = best {
            if dd < 9 {
                self.density = None;
                self.mem[0x46EB] &= !0x40;
                self.set16(0x197C, self.get16(rec + 2));
                self.set16(0x197E, self.get16(rec + 4));
                self.map_redraw(assets);
            }
        }
        true
    }
}

/// sub_1586E: stripe pattern of the troop occupation.
fn occupation_pattern(bl: u8, ch: u8, cl: u8) -> u8 {
    match (bl != 0, ch != 0, cl != 0) {
        (false, false, false) => 0x00,
        (true, false, false) => 0x55,
        (false, true, false) => 0xAA,
        (true, true, false) => {
            if bl == ch {
                0x66
            } else if bl < ch {
                0x9A
            } else {
                0x65
            }
        }
        (false, false, true) => 0xFF,
        (true, false, true) => {
            if bl == cl {
                0x77
            } else if bl < cl {
                0xDF
            } else {
                0x75
            }
        }
        (false, true, true) => {
            if ch == cl {
                0xBB
            } else if ch < cl {
                0xEF
            } else {
                0xBA
            }
        }
        (true, true, true) => {
            if bl < ch {
                if ch < cl {
                    0xDE
                } else {
                    0x9B
                }
            } else if bl < cl {
                0x7B
            } else {
                0x67
            }
        }
    }
}
