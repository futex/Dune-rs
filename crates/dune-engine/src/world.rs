//! In-game world: locations, rooms, characters, the UI panel and dialogue.
//!
//! The game state lives in `mem`, a mirror of the original data segment seeded from
//! DNCDPRG.EXE, so addresses from the reversing notes apply directly:
//! * ds:4 place (dl = room, dh = place type, 0x20 = palace), ds:6 where (0x80 = indoors | location<<8)
//!   outdoors: ds:4 = longitude, ds:6 = (sub-row << 8) | row (docs/re/travel.md §0.2)
//! * ds:2 clock, ds:10 party mask, ds:12 present mask, ds:2A story phase
//! * ds:0FD8 + 16c character records {place, where, handler, ...}
//! * ds:13C4 room tables (5 bytes: room byte, exits N/E/S/W)
//! * ds:0100 location table, ds:4948 TABLAT, ds:197C/197E map centre (dune_formats::planet)
//!
//! The screens other than rooms live in `map_screen.rs` (SEE DUNE MAP), `globe_screen.rs`
//! (the globe) and `travel.rs` (desert walking, ornithopter, worms).
//! See docs/re/sal.md, ui.md, travel.md, globe.md, animations.md and gameflow.md.

use crate::{
    assets::Assets,
    audio::Audio,
    gfx::{Screen, W},
    input::{Input, Key},
    intro::{play_voc, start_music, stop_all},
    panel::{self, Panel},
    transition::Transition,
};
use dune_formats::{
    dialogue::{Condit, Dialogue},
    palette::Palette,
    planet,
    sal::{bank_for_room_byte, draw_room, sal_room, Marker, Sal, VIEW_H, VIEW_W},
    sprite::{DrawOpts, SpriteSheet},
    text::Font,
};
use std::rc::Rc;

pub(crate) const ROOM_TABLE: usize = 0x13C4;
pub(crate) const CHAR_RECORDS: usize = 0x0FD8;
const SCALE: [u16; 8] = [256, 288, 320, 352, 384, 448, 512, 640];

// COMMAND string indices (0-based = text id - 1).
const S_SEE_MAP: usize = 151;
const S_MIRROR: usize = 152;
const S_MIXER: usize = 157;
const S_TALK: usize = 158;
const S_COME: usize = 144;
const S_STAY: usize = 145;
const S_STOP: usize = 147;
const S_WHAT: usize = 148;
const S_NAMES: usize = 119;

// Panel colours (from the room bank palette, 0xF0..0xFF).
const C_TEXT: u8 = 0xFA;
const C_DISABLED: u8 = 0xF5;
const C_BAR: u8 = 0xF3;
const C_DAY_BG: u8 = 0xF1;

pub(crate) const VIEW: usize = VIEW_W * VIEW_H;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Cmd {
    SeeMap,
    Mixer,
    Mirror,
    /// Open the talk menu for character c.
    Character(usize),
    Talk(usize),
    Come(usize),
    Stay(usize),
    What(usize),
    StopTalking,
    Restart,
    LoadMenu,
    SaveMenu,
    Load(u8),
    Save(u8),
    CancelSlots,
    ExitGame,
    LookAway,
    // Map screen (ds:20F2, 20DA, 20E6).
    ExitMaps,
    ContactTroops,
    GiveOrders,
    SpiceDensity,
    TakeOrniMaps,
    FindProspectors,
    GoThereOrni,
    GoThereWorm,
    CancelPopup,
    // Rooms / desert.
    TakeOrni,
    CallWorm,
    WaitEvening,
    WaitMorning,
    MassiveAttack,
    FightDay,
    // Troops (troop_ui.rs) and the story glue (war_ui.rs).
    Leader(usize),
    Overpower,
    GiveOrdersRoom,
    WorkForMe,
    Ord(u8),
    Argue(u8),
    ViewMessages(bool),
    Message(usize),
    MessagesCancel,
    // Destination screen / flight.
    CancelDest,
    Skip,
    BackToStart,
    Nearest,
    ChangeDest,
    IgnoreWarning,
    // Globe (ds:204A, 2062).
    ExitGlobe,
    SeeResults,
    StandardVision,
    SeeMapArea,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum View {
    Room,
    Mirror,
    Slots,
}

/// Which screen owns the view area.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Scr {
    Room,
    Map,
    Globe,
    Dest,
    Flight,
}

pub(crate) struct Line {
    /// Dynamic label (save slots); otherwise COMMAND string `text`.
    pub label: Option<Vec<u8>>,
    pub text: usize,
    pub cmd: Cmd,
    pub enabled: bool,
    /// Text id flag 0x8000: drawn highlighted.
    pub hl: bool,
}

impl Line {
    pub fn new(text_id: u16, cmd: Cmd, enabled: bool) -> Line {
        Line { label: None, text: text_id as usize - 1, cmd, enabled, hl: false }
    }
}

pub(crate) struct Talk {
    pub pages: Vec<Vec<u8>>,
    pub page: usize,
    pub timer: f32,
    /// Work to do once the line has been said.
    pub after: crate::talk::After,
}

pub struct World {
    pub mem: Vec<u8>,
    pub(crate) sal: Option<Sal>,
    pub(crate) sal_kind: usize,
    pub(crate) entry: [u8; 5],
    /// The room / desert view (320x152), clean (sub_1C412 copy).
    pub(crate) view: Vec<u8>,
    pub(crate) palette: Palette,
    /// Character standing at each drawn marker: (c, x, y) for hit tests.
    figures: Vec<(usize, i32, i32)>,
    pub(crate) menu: Vec<Line>,
    pub(crate) talk_menu_for: Option<usize>,
    pub(crate) hover: Option<usize>,
    pub(crate) dirty: bool,
    pub quit: bool,
    pub(crate) message: Option<(Vec<u8>, f32)>,
    pub(crate) dialogue: Option<Dialogue>,
    pub(crate) condit: Option<Condit>,
    pub(crate) talk: Option<Talk>,
    /// Talking close-up drawn over the room view.
    pub(crate) closeup: Option<crate::face::FaceAnim>,
    /// Timer ticks (200.3 Hz) since the world started.
    pub(crate) ticks: f64,
    pub(crate) view_mode: View,
    /// Map buffer (MAP.HSQ, 0xC5FC bytes): bits 5..4 are saved, bit 6 marks places.
    pub(crate) map: Vec<u8>,
    pub(crate) map2: Vec<u8>,
    /// Code-segment variables cs:00AA..014B (saved as-is).
    cs_vars: Vec<u8>,
    /// Directory where dune37s<N>.sav live (next to DUNE.DAT).
    save_dir: std::path::PathBuf,
    pub new_game: bool,

    pub(crate) scr: Scr,
    pub(crate) panel: Panel,
    pub(crate) drv: planet::DriverTables,
    /// Running transition (the screen shows its frame buffer and DAC).
    pub(crate) trans: Option<Transition>,
    /// Effect to use for the next draw (sub_1C108).
    pub(crate) effect: Option<u8>,
    /// Tick counter (ds:CE7A).
    pub(crate) now: u64,
    /// ds:46DB: ticks left in the current clock slot.
    slot_timer: u32,
    /// Day/night palette fade (task 0x3916) and the current sky colours.
    pub(crate) fade: Option<crate::sky::Fade>,
    pub(crate) sky_cur: Palette,
    /// Sietch water ripple (task 0xC0B6) and the screen it draws on.
    pub(crate) ripple: Option<crate::sky::Ripple>,
    pub(crate) ripple_fb: Vec<u8>,
    /// Map screen state.
    pub(crate) mapst: crate::map_screen::MapState,
    pub(crate) globe: Option<crate::globe_screen::GlobeState>,
    pub(crate) dest: Option<crate::travel::Dest>,
    pub(crate) flight: Option<crate::travel::Flight>,
    /// Blocking sequence (animations that the original runs synchronously).
    pub(crate) seq: Option<crate::travel::Seq>,
    /// Auto-repeat of a held zone with flag 0x40 (every 50 ticks).
    repeat: Option<(usize, u64)>,
    /// sub_1D7B7: last (tick >> 6) seen.
    blink_last: u64,
    /// Medallion ramp target (ds:E8: 0..10), one step per 8 ticks.
    pub(crate) medallion_to: u8,
    medallion_acc: u32,
    /// ds:DC5A: tick of the last input.
    pub(crate) idle_since: u64,
    /// Where the load/save slot menu returns.
    slots_return: Scr,
    /// Game over text (ds:46D9).
    pub(crate) game_over: Option<u8>,
    pub(crate) mouse: (i32, i32),
    last_mouse: (i32, i32),
    /// Palette of the last map screen (ONMAP / ORNYPAN banks), whose colours 16..55 stay
    /// in the DAC shadow afterwards (used by the flight view's small map).
    pub(crate) screen_pal: Option<Palette>,
    /// The next transition goes through a black screen first (collapse in the desert).
    pub(crate) via_black: bool,
    /// Work to do once the running transition is over.
    pub(crate) after_trans: Option<crate::travel::After>,
    /// cs:65B4 vegetation LFSR (sim.rs).
    pub(crate) lfsr: u16,
    /// COMMAND strings (text ids n = entry n - 1).
    pub(crate) cmds: Vec<Vec<u8>>,
    /// Troop layer of the map and the orders (troop_ui.rs).
    pub(crate) tui: crate::troop_ui::TroopUi,
    /// ARGUE / ACCEPT / REFUSE menu pushed (Some(smuggler)).
    pub(crate) argue: Option<bool>,
    /// Routine run after the talk (ds:227E).
    pub(crate) deferred: Option<crate::talk::After>,
    /// Speaker, phrase and bank of the last line (WHAT ?).
    pub(crate) last_line: Option<(usize, u16, u8)>,
    /// A report told as a vision (speaker).
    pub(crate) vision: Option<usize>,
    pub(crate) icons_dirty: bool,
    /// War mode: the ATTACK scene.
    pub(crate) attack: Option<crate::war_ui::AttackScene>,
    pub(crate) movie: Option<crate::war_ui::Movie>,
    pub(crate) busy_until: u64,
    /// sub_10FD9 in progress (ds:46DA).
    pub(crate) skipping: bool,
    /// Report-event mosaic pulse already shown.
    pub(crate) pulse: Option<u8>,
    /// Communications-room message list.
    pub(crate) msg_menu: Option<Vec<Line>>,
    /// MOVE TROOP prompt in the contact window.
    pub(crate) prompt: Option<Vec<u8>>,
    pub(crate) search_saved: Option<(u8, u16)>,
    pub(crate) ending_skip: bool,
    pub(crate) ending_done: bool,
    /// SEE SPICE DENSITY window.
    pub(crate) density: Option<crate::map_screen::Density>,
    /// MASSIVE ATTACK flashes left and the next one.
    pub(crate) flashes: u8,
    pub(crate) next_flash: u64,
    /// Desert harvester (task 0x4BB9): origin, frame, tick accumulator, clean view.
    pub(crate) harvester: Option<crate::troop_ui::Harvester>,
    /// Sound effect requested where no audio handle is at hand.
    pub(crate) pending_voc: Option<&'static str>,
    /// "Others..." page of the command lines (ds:DCE4 / 0x10).
    pub(crate) menu_page: usize,
    /// The ending script's speakers still to come.
    pub(crate) ending_queue: Vec<usize>,
    pub(crate) ending_pending: bool,
}

fn sal_kind(dh: u8) -> usize {
    match dh {
        0..=0x1F => 0,
        0x20 => 1,
        0x21..=0x27 => 2,
        _ => 3,
    }
}

const SAL_FILES: [&str; 4] = ["SIET.SAL", "PALACE.SAL", "VILG.SAL", "HARK.SAL"];
const LOCATION_HNM: [&str; 4] = ["SIET.HNM", "PALACE.HNM", "PALACE.HNM", "PALACE.HNM"];

/// Glyph blitter (driver k=7): set bits -> fg, clear bits -> bg unless bg == 0.
pub(crate) fn text(fb: &mut [u8], font: &Font, x: i32, y: i32, s: &[u8], fg: u8, bg: u8) -> i32 {
    let mut cx = x;
    for &c in s {
        let c = if c >= 0x80 { b'@' } else { c };
        let w = font.widths[c as usize] as i32;
        for row in 0..font.height {
            let bits = font.glyph_row(c, row);
            let py = y + row as i32;
            if !(0..200).contains(&py) {
                continue;
            }
            for col in 0..w.min(8) {
                let px = cx + col;
                if !(0..320).contains(&px) {
                    continue;
                }
                let p = py as usize * 320 + px as usize;
                if bits & (0x80 >> col) != 0 {
                    fb[p] = fg;
                } else if bg != 0 {
                    fb[p] = bg;
                }
            }
        }
        cx += w;
    }
    cx
}

/// Multi-line text: CR (0x0D) starts a new line 7 (small) / 10 (big) pixels lower.
pub(crate) fn text_lines(fb: &mut [u8], font: &Font, x: i32, y: i32, s: &[u8], fg: u8, bg: u8) -> (i32, i32) {
    let step = if font.height == 7 { 7 } else { 10 };
    let (mut cy, mut ex) = (y, x);
    for (i, l) in s.split(|&b| b == 0x0D).enumerate() {
        if i > 0 {
            cy += step;
        }
        ex = text(fb, font, x, cy, l, fg, bg);
    }
    (ex, cy)
}

pub(crate) fn fill(fb: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    for y in y0.max(0)..y1.min(200) {
        for x in x0.max(0)..x1.min(320) {
            fb[y as usize * 320 + x as usize] = c;
        }
    }
}

/// sub_1C560: rectangle outline, inclusive corners.
pub(crate) fn rect(fb: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let mut put = |x: i32, y: i32| {
        if (0..320).contains(&x) && (0..200).contains(&y) {
            fb[y as usize * 320 + x as usize] = c;
        }
    };
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

impl World {
    pub fn new(assets: &mut Assets, audio: &mut Audio) -> World {
        stop_all(audio);
        start_music(assets, audio, "ARRAKIS", 0);
        let mut w = World::bare(assets);
        // Start of game (loc_10292 / start): palace ornithopter, sub_108F0(0x200A, 0x0180), clock = 2.
        w.mem[0x0115] = w.mem[0x0115].wrapping_add(1);
        w.set16(2, 2);
        // start: sub_11860 -> loc_1186B sets ds:FB = 1; the prologue clears ds:227D.
        w.mem[0xFB] = 1;
        w.mem[0x227D] = 0;
        // The three RNG seeds come from the BIOS tick count at start-up.
        let seed = std::env::var("DUNE_SEED").ok().and_then(|v| u16::from_str_radix(&v, 16).ok()).unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_millis() / 55) as u16).unwrap_or(0x1234)
        });
        for a in [0xD824, 0xD826, 0xD828] {
            w.set16(a, seed);
        }
        let start = std::env::var("DUNE_START_PLACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0x200A);
        // Debug: DUNE_START_LOC=n (decimal) starts in location n instead of the palace.
        let loc = std::env::var("DUNE_START_LOC").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        w.enter_place_record(loc);
        // Debug: DUNE_POKE=addr:val[,addr:val...] (hex) patches ds: bytes before the first room.
        if let Ok(p) = std::env::var("DUNE_POKE") {
            for kv in p.split(',') {
                if let Some((a, v)) = kv.split_once(':') {
                    if let (Ok(a), Ok(v)) = (usize::from_str_radix(a, 16), u8::from_str_radix(v, 16)) {
                        w.mem[a & 0xFFFF] = v;
                    }
                }
            }
        }
        w.enter(assets, start);
        // start: the game writes slot 0 (restart point) when a new game begins.
        if std::env::var("DUNE_START_PLACE").is_err() {
            w.save_slot(0);
        }
        w
    }

    /// Renders a place as the intro scenes do (sub_108F0 + sub_137B2) without starting a
    /// game: returns the 320x152 view, its palette and the characters' marker positions
    /// (character, x, y). `present` is ds:12 (characters standing in the room).
    pub fn render_place(assets: &mut Assets, place: u16, where_: u16, present: u16, clock: u16) -> (Vec<u8>, Palette, Vec<(usize, i32, i32)>) {
        let mut w = World::bare(assets);
        w.set16(2, clock);
        w.enter_place_record((where_ >> 8) as usize);
        w.set16(6, where_);
        w.enter(assets, place);
        w.set16(0x12, present);
        w.set16(0x10, 0);
        w.render_room(assets);
        (w.view.clone(), w.palette.clone(), w.figures.clone())
    }

    /// World state from the EXE image, no music, no room entered.
    fn bare(assets: &mut Assets) -> World {
        let mut mem = assets.exe.as_ref().map(|e| e[crate::assets::EXE_DS..].to_vec()).unwrap_or_default();
        mem.resize(0x10000, 0);
        let mut map = assets.dat.read("MAP.HSQ").unwrap_or_default();
        map.resize(dune_formats::save::MAP_SIZE, 0);
        let map2 = assets.dat.read("MAP2.HSQ").unwrap_or_default();
        // sub_100D1 + sub_10169.
        if let Ok(t) = assets.dat.read("TABLAT.BIN") {
            planet::init_tables(&mut mem, &t);
        }
        if mem[0x100] != 0 {
            planet::init_locations(&mut mem, &mut map, &map2, true);
            let mut lfsr = 1u16;
            crate::sim::Sim::new(&mut mem, &mut map, &mut lfsr).init_troops();
        }
        let panel = Panel::new(&mem);
        let cmds = assets.commands().map(|t| (0..400).map(|i| t.get(i).to_vec()).collect()).unwrap_or_default();
        World {
            sal: None,
            sal_kind: usize::MAX,
            entry: [0; 5],
            view: vec![0; VIEW],
            palette: Palette::black(),
            figures: vec![],
            menu: vec![],
            talk_menu_for: None,
            hover: None,
            dirty: true,
            quit: false,
            message: None,
            dialogue: assets.dat.read("DIALOGUE.HSQ").ok().and_then(Dialogue::parse),
            condit: assets.dat.read("CONDIT.HSQ").ok().and_then(Condit::parse),
            talk: None,
            closeup: None,
            ticks: 0.0,
            view_mode: View::Room,
            map,
            map2,
            cs_vars: assets.exe.as_ref().and_then(|e| e.get(0x200 + 0xAA..0x200 + 0xAA + 0xA2)).map(|b| b.to_vec()).unwrap_or(vec![0; 0xA2]),
            save_dir: assets.dat_dir.clone(),
            new_game: true,
            scr: Scr::Room,
            panel,
            drv: planet::DriverTables::new(assets.dat.read("DNVGA.HSQ").unwrap_or_default()),
            trans: None,
            effect: None,
            now: 0x1000,
            slot_timer: 12000,
            fade: None,
            sky_cur: Palette::black(),
            ripple: None,
            ripple_fb: vec![0; VIEW],
            mapst: Default::default(),
            globe: None,
            dest: None,
            flight: None,
            seq: None,
            repeat: None,
            blink_last: 0,
            medallion_to: 10,
            medallion_acc: 0,
            idle_since: 0,
            slots_return: Scr::Room,
            game_over: None,
            mouse: (0, 0),
            last_mouse: (0, 0),
            screen_pal: None,
            via_black: false,
            after_trans: None,
            lfsr: 1,
            cmds,
            tui: Default::default(),
            argue: None,
            deferred: None,
            last_line: None,
            vision: None,
            icons_dirty: false,
            attack: None,
            movie: None,
            busy_until: 0,
            skipping: false,
            pulse: None,
            msg_menu: None,
            prompt: None,
            search_saved: None,
            ending_skip: false,
            ending_done: false,
            density: None,
            harvester: None,
            pending_voc: None,
            flashes: 0,
            next_flash: 0,
            menu_page: 0,
            ending_queue: vec![],
            ending_pending: false,
            mem,
        }
    }

    /// sub_108F0 part: ds:114E / ds:1150 / ds:8 / ds:9 for location index i.
    pub(crate) fn enter_place_record(&mut self, i: usize) {
        if i == 0 || i > 70 {
            return;
        }
        let a = planet::loc_addr(i);
        self.set16(0x114E, a as u16);
        self.set16(0x1150, a as u16);
        self.mem[8] = self.mem[a + 8];
        self.mem[9] = i as u8;
        self.set16(6, ((i as u16) << 8) | 0x80);
    }

    fn slot_path(&self, n: u8) -> std::path::PathBuf {
        self.save_dir.join(format!("dune37s{n}.sav"))
    }

    fn snapshot(&mut self) -> dune_formats::save::SaveGame {
        use dune_formats::save::*;
        if let Some(d) = &self.dialogue {
            let n = d.data.len().min(DS_AA76);
            self.mem[0xAA76..0xAA76 + n].copy_from_slice(&d.data[..n]);
        }
        SaveGame {
            time: self.clock(),
            map_bits: pack_map(&self.map),
            cs_vars: self.cs_vars.clone(),
            ds_aa76: self.mem[0xAA76..0xAA76 + DS_AA76].to_vec(),
            ds_low: self.mem[..DS_LOW].to_vec(),
        }
    }

    fn save_slot(&mut self, n: u8) -> bool {
        let bytes = self.snapshot().to_bytes();
        std::fs::write(self.slot_path(n), bytes).is_ok()
    }

    pub(crate) fn load_slot(&mut self, assets: &mut Assets, n: u8) -> bool {
        use dune_formats::save::*;
        let Some(sg) = std::fs::read(self.slot_path(n)).ok().and_then(|f| SaveGame::parse(&f)) else { return false };
        self.mem[..DS_LOW].copy_from_slice(&sg.ds_low);
        self.mem[0xAA76..0xAA76 + DS_AA76].copy_from_slice(&sg.ds_aa76);
        if let Some(d) = &mut self.dialogue {
            let len = d.data.len().min(DS_AA76);
            d.data[..len].copy_from_slice(&sg.ds_aa76[..len]);
        }
        unpack_map(&sg.map_bits, &mut self.map);
        // The place bits (bit 6) are not saved: mark the cells again (sub_10169).
        let map2 = std::mem::take(&mut self.map2);
        planet::init_locations(&mut self.mem, &mut self.map, &map2, false);
        self.map2 = map2;
        self.cs_vars = sg.cs_vars;
        self.set16(2, sg.time);
        self.closeup = None;
        self.talk = None;
        self.view_mode = View::Room;
        self.scr = Scr::Room;
        self.globe = None;
        self.dest = None;
        self.flight = None;
        self.seq = None;
        self.fade = None;
        self.mem[0x46EB] = 0;
        self.mem[0xFB] = 1;
        self.mem[0x11CA] = 0;
        self.panel.set_left(&self.mem.clone(), panel::LEFT_DEFAULT);
        let place = self.place();
        self.sal_kind = usize::MAX;
        if self.mem[0x11C9] & 3 != 0 {
            // Saved in flight: resume the journey (ds:4727 is part of the saved state).
            crate::travel::resume_flight(self, assets);
        } else if !self.indoors() {
            self.redraw_room(assets);
        } else {
            self.enter(assets, place);
        }
        true
    }

    /// Slot label (sub_1B2C4): "Log n: DAY dd / time" for slots 1-2, fixed text for 3-4.
    fn slot_label(&self, assets: &Assets, n: u8) -> (Vec<u8>, bool) {
        let cmd = assets.commands();
        let base = cmd.as_ref().map(|t| t.get(0x10E + n as usize - 1).to_vec()).unwrap_or_default();
        let Ok(f) = std::fs::read(self.slot_path(n)) else { return (base, false) };
        if n > 2 || f.len() < 2 {
            return (base, true);
        }
        let t = u16::from_le_bytes([f[0], f[1]]);
        let day = ((t + 3) >> 4) + 1;
        let mut s = base;
        if let Some(p) = s.iter().position(|&b| b == b'/') {
            let d = format!("{day:2}");
            if p >= 3 {
                s[p - 3..p - 1].copy_from_slice(d.as_bytes());
            }
            let times = cmd.as_ref().map(|t| t.get(278).to_vec()).unwrap_or_default();
            let slot = (t & 15) as usize;
            if let Some(tm) = times.get(slot * 10..slot * 10 + 10) {
                s.truncate(p + 2);
                s.extend_from_slice(tm);
            }
        }
        (s, true)
    }

    fn open_mirror(&mut self, assets: &mut Assets) {
        let Some(m) = assets.sheet("MIRROR.HSQ") else { return };
        let mut pal = self.palette.clone();
        m.apply_palette(&mut pal);
        let mut bg = vec![0u8; VIEW];
        for i in [0usize, 1] {
            if let Some(s) = m.sprites.get(i) {
                s.draw(&mut bg, VIEW_W, 0, 0, DrawOpts::default());
            }
        }
        let now = self.ticks as u64;
        let (idx, variant) = crate::face::bank_and_variant(0x2D, &self.mem);
        if let Some(bank) = assets.resource_name(idx + 2).and_then(|n| assets.dat.read(&n).ok()).and_then(dune_formats::face::FaceBank::parse) {
            bank.sheet.apply_palette(&mut pal);
            let params = crate::face::FaceParams {
                character: 0x2D, variant, f0: 0, shift: 0, mouth_rect: None, words: 1, story_phase: self.mem[0x2A], seed: self.rng_seed(),
            };
            let mut anim = crate::face::FaceAnim::new(bank, params, &bg, now);
            if let Some(s) = m.sprites.get(2) {
                s.draw(&mut anim.back, VIEW_W, 0, 0, DrawOpts::default());
            }
            self.closeup = Some(anim);
        }
        self.palette = pal;
        self.view_mode = View::Mirror;
        self.build_menu();
    }

    fn close_mirror(&mut self, assets: &mut Assets) {
        self.closeup = None;
        self.view_mode = View::Room;
        self.build_menu();
        self.render_room(assets);
    }

    pub(crate) fn get16(&self, a: usize) -> u16 {
        u16::from_le_bytes([self.mem[a], self.mem[a + 1]])
    }
    pub(crate) fn set16(&mut self, a: usize, v: u16) {
        self.mem[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }
    pub fn place(&self) -> u16 {
        self.get16(4)
    }
    pub(crate) fn clock(&self) -> u16 {
        self.get16(2)
    }
    pub(crate) fn indoors(&self) -> bool {
        self.mem[6] == 0x80
    }
    pub(crate) fn in_transit(&self) -> bool {
        self.mem[0x11C9] & 3 != 0
    }

    /// sub_1407E: the current map position.
    pub(crate) fn cur_pos(&self) -> (u16, i16) {
        if self.indoors() {
            let si = self.get16(0x114E) as usize;
            (self.get16(si + 2), self.get16(si + 4) as i16)
        } else {
            (self.get16(4), self.mem[6] as i8 as i16)
        }
    }

    /// COMMAND text without the assets.
    pub(crate) fn cmd_t(&self, id: u16) -> Vec<u8> {
        let id = (id & 0x3FFF) as usize;
        if id == 0 {
            return vec![];
        }
        self.cmds.get(id - 1).cloned().unwrap_or_default()
    }

    /// Text id n (COMMAND entry n - 1), with the id flags masked off.
    pub(crate) fn cmd_text(assets: &Assets, id: u16) -> Vec<u8> {
        let id = (id & 0x3FFF) as usize;
        if id == 0 {
            return vec![];
        }
        assets.commands().map(|t| t.get(id - 1).to_vec()).unwrap_or_default()
    }

    /// sub_162A6: COMMAND[b0] + ' ' / '-' + COMMAND[b1 + 0x0C].
    pub(crate) fn place_name(&self, assets: &Assets, rec: usize) -> Vec<u8> {
        let (b0, b1) = (self.mem[rec] as u16, self.mem[rec + 1] as u16);
        let mut s = World::cmd_text(assets, b0);
        s.push(if b1 < 3 { b' ' } else { b'-' });
        s.extend(World::cmd_text(assets, b1 + 0x0C));
        s
    }

    /// sub_16231 + 0x44: "Sietch: " / "Palace: " / "Village: " / "Fort: ".
    pub(crate) fn place_type_text(&self, assets: &Assets, rec: usize) -> Vec<u8> {
        let k = match planet::kind(self.mem[rec + 8]) {
            4 => 1,
            k => k,
        };
        World::cmd_text(assets, 0x44 + k as u16)
    }

    /// sub_17F27: equipment available at a place into ds:46FE..4704 (46FF = ornithopters).
    pub(crate) fn equipment_here(&mut self, rec: usize) {
        if rec < 0x100 {
            for i in 0..7 {
                self.mem[0x46FE + i] = 0;
            }
            return;
        }
        for i in 0..7 {
            self.mem[0x46FE + i] = self.mem[rec + 0x14 + i];
        }
        let mut t = self.mem[rec + 9];
        let mut guard = 0;
        while t != 0 && guard < 100 {
            let si = 0x8AA + 0x1B * (t as usize - 1);
            let mut al = self.mem[si + 0x19];
            let mut di = 0;
            while al != 0 && di < 7 {
                if al & 0x80 != 0 {
                    self.mem[0x46FE + di] = self.mem[0x46FE + di].saturating_sub(1);
                }
                al <<= 1;
                di += 1;
            }
            t = self.mem[si + 1];
            guard += 1;
        }
    }

    fn room_entry(&self, place: u16) -> Option<[u8; 5]> {
        let (dl, dh) = ((place & 0xFF) as usize, (place >> 8) as usize);
        let ptr = self.get16(ROOM_TABLE + 2 * dh) as usize;
        self.mem.get(ptr + dl.checked_sub(1)? * 5..ptr + dl * 5)?.try_into().ok()
    }

    fn char_place(&self, c: usize) -> (u16, u16) {
        (self.get16(CHAR_RECORDS + 16 * c), self.get16(CHAR_RECORDS + 16 * c + 2))
    }

    /// Characters whose record matches ds:4/ds:6.
    fn present(&self) -> Vec<usize> {
        let (p, w) = (self.place(), self.get16(6));
        (0..16).filter(|&c| self.char_place(c) == (p, w)).collect()
    }

    /// Followers (ds:10) move with Paul (sub_136D3 / sub_140C3).
    pub(crate) fn move_followers(&mut self) {
        let party = self.get16(0x10);
        let (p, here) = (self.place(), self.get16(6));
        for c in 0..16 {
            if party & (1 << c) != 0 {
                self.set16(CHAR_RECORDS + 16 * c, p);
                self.set16(CHAR_RECORDS + 16 * c + 2, here);
            }
        }
    }

    /// Enters room `place` of the current location (sub_14057 + loc_12DBF).
    pub fn enter(&mut self, assets: &mut Assets, place: u16) {
        let Some(entry) = self.room_entry(place) else {
            self.say(b"(this exit is not implemented yet)");
            return;
        };
        let old_dl = self.place() as u8;
        self.set16(4, place);
        self.move_followers();
        self.mem[0xD] = old_dl;
        self.mem[0xB] = place as u8;
        self.entry = entry;
        self.redraw_room(assets);
        // sub_14057: the Harkonnen palace room 2 once the shield is down: the ending.
        self.ending_check(assets);
    }

    /// loc_12DBF: SAL load, menu/compass, room view, transition 0x10 when the
    /// day/night flag changes, else a cut.
    pub(crate) fn redraw_room(&mut self, assets: &mut Assets) {
        self.scr = Scr::Room;
        self.mem[0x47A4] = 0;
        if self.indoors() {
            self.place_troop_chars();
        }
        let mask = self.present().iter().fold(0u16, |m, &c| m | 1 << c);
        self.set16(0x12, mask);
        if self.indoors() {
            if let Some(e) = self.room_entry(self.place()) {
                self.entry = e;
            }
            let kind = sal_kind((self.place() >> 8) as u8);
            if kind != self.sal_kind {
                self.sal = assets.dat.read(SAL_FILES[kind]).ok().and_then(|d| Sal::parse(&d));
                self.sal_kind = kind;
            }
        }
        self.talk_menu_for = None;
        self.view_mode = View::Room;
        self.compass();
        self.panel.set_left(&self.mem.clone(), panel::LEFT_DEFAULT);
        self.build_menu();
        let was = self.mem[0x46DF];
        self.render_room(assets);
        let now = self.mem[0x46DF];
        let prev = std::mem::replace(&mut self.mem[0x46E0], now);
        if prev != now && self.effect.is_none() {
            self.effect = Some(0x10);
        }
        let _ = was;
        if self.indoors() && self.mem[0x4774] == 0 {
            self.medallion_to = 10;
        }
        if self.indoors() {
            self.shipment_check();
        }
        self.dirty = true;
    }

    /// sub_12FFB: compass zones 12..17.
    pub(crate) fn compass(&mut self) {
        let mem = self.mem.clone();
        if self.mem[0x2B] != 0 || self.game_over.is_some() {
            self.panel.set_compass(&mem, panel::COMPASS_HIDDEN);
            return;
        }
        if self.in_transit() {
            let p = if self.mem[0x11CA] == 0 && self.mem[0x11CB] != 0 { panel::COMPASS_FLIGHT } else { panel::COMPASS_HIDDEN };
            self.panel.set_compass(&mem, p);
            return;
        }
        self.panel.set_compass(&mem, panel::COMPASS_ROOM);
        let (dl, dh) = (self.place() as u8, (self.place() >> 8) as u8);
        if self.indoors() && dh != 0x21 {
            let mut z17 = if self.get16(0x114E) == 0x100 { 0x80 } else { 0x20 };
            let mut spr = 0x21;
            if dl == 1 {
                spr = 0x22;
                z17 = 0x20;
            }
            self.panel.z[12].sprite = spr;
            self.panel.z[17].state = z17;
            self.mem[panel::COMPASS_ROOM + 5 * 14 + 8] = z17;
            for k in 1..=4 {
                let e = self.entry[k];
                self.panel.z[12 + k].state = if e != 0 && (e as i8) >= -5 { 0x80 } else { 0x20 };
            }
        } else {
            self.panel.z[12].sprite = 0x23;
            for k in 1..=4 {
                self.panel.z[12 + k].sprite = 0x1C + k as u16;
                self.panel.z[12 + k].state = 0x80;
                self.panel.z[12 + k].flags = 0;
            }
        }
    }

    /// Default room menu (sub_12EFB + sub_13090), the talk menu, or the screen's menu.
    pub(crate) fn build_menu(&mut self) {
        self.menu.clear();
        match self.scr {
            Scr::Map => return self.map_menu(),
            Scr::Globe => return self.globe_menu(),
            Scr::Dest => {
                self.menu.push(Line::new(0xA3, Cmd::CancelDest, true));
                return;
            }
            Scr::Flight => return crate::travel::flight_menu(self),
            Scr::Room => {}
        }
        if let Some(n) = self.game_over {
            let _ = n;
            for (id, cmd) in [(0xBA, Cmd::Restart), (0xB4, Cmd::LoadMenu), (0xBB, Cmd::ExitGame)] {
                self.menu.push(Line::new(id, cmd, true));
            }
            return;
        }
        if let Some(smug) = self.argue {
            let _ = smug;
            // Menu ds:1FFE.
            for (id, a) in [(0xF1, 3u8), (0xEF, 1), (0xF0, 2)] {
                self.menu.push(Line::new(id, Cmd::Argue(a), true));
            }
            self.menu.push(Line::new(0x95, Cmd::What(0), true));
            return;
        }
        if let Some(m) = &self.msg_menu {
            for l in m {
                self.menu.push(Line { label: l.label.clone(), text: l.text, cmd: l.cmd, enabled: l.enabled, hl: false });
            }
            return;
        }
        if self.view_mode == View::Mirror {
            for (text, cmd) in [(185, Cmd::Restart), (179, Cmd::LoadMenu), (178, Cmd::SaveMenu), (186, Cmd::ExitGame), (156, Cmd::LookAway)] {
                self.menu.push(Line { label: None, text, cmd, enabled: true, hl: false });
            }
            return;
        }
        if let Some(c) = self.talk_menu_for {
            self.menu.push(Line { label: None, text: S_TALK, cmd: Cmd::Talk(c), enabled: true, hl: false });
            let l2 = self.talk_line2(c);
            self.menu.push(l2);
            let _ = (S_STAY, S_COME);
            self.menu.push(Line { label: None, text: S_WHAT, cmd: Cmd::What(c), enabled: true, hl: false });
            self.menu.push(Line { label: None, text: S_STOP, cmd: Cmd::StopTalking, enabled: true, hl: false });
            return;
        }
        let worm_ok = self.mem[0x2A] >= 0x4F;
        if self.indoors() {
            self.menu.push(Line { label: None, text: S_SEE_MAP, cmd: Cmd::SeeMap, enabled: true, hl: false });
            let (dl, loc) = (self.place() & 0xFF, self.get16(6) >> 8);
            if dl == 1 {
                if self.mem[0x2B] != 0 {
                    self.menu.push(Line::new(0x9A, Cmd::MassiveAttack, true));
                    self.menu.push(Line::new(0x9B, Cmd::FightDay, true));
                    self.menu.push(Line::new(0xA8, Cmd::CallWorm, worm_ok));
                } else {
                    let rec = self.get16(0x114E) as usize;
                    self.equipment_here(rec);
                    let ok = self.mem[0x46FF] != 0;
                    self.menu.push(Line::new(0xA7, Cmd::TakeOrni, ok));
                }
            }
            let comm = self.comm_lines();
            self.menu.extend(comm);
            if (self.place() >> 8) == 0x20 && loc == 1 && dl == 9 {
                self.menu.push(Line { label: None, text: S_MIRROR, cmd: Cmd::Mirror, enabled: true, hl: false });
            }
            self.menu.push(Line { label: None, text: S_MIXER, cmd: Cmd::Mixer, enabled: true, hl: false });
            if self.mem[0x11C9] == 0 {
                let present = self.present();
                let lines = self.char_lines(&present);
                self.menu.extend(lines);
                let _ = S_NAMES;
            }
        } else {
            self.menu.push(Line::new(0x98, Cmd::SeeMap, true));
            self.menu.push(Line::new(0xA8, Cmd::CallWorm, worm_ok));
            if self.clock() & 15 < 11 {
                self.menu.push(Line::new(0xA5, Cmd::WaitEvening, true));
            } else {
                self.menu.push(Line::new(0xA6, Cmd::WaitMorning, true));
            }
            self.menu.push(Line::new(0x9E, Cmd::Mixer, true));
        }
        self.menu_page = 0;
    }

    /// sub_1D397: the visible lines (None = "  Others...", 4 lines per page).
    pub(crate) fn shown_lines(&self) -> Vec<Option<usize>> {
        let n = self.menu.len();
        if n <= 5 {
            return (0..n).map(Some).collect();
        }
        let start = (self.menu_page * 4).min(n);
        let mut v: Vec<Option<usize>> = (start..(start + 4).min(n)).map(Some).collect();
        v.push(None);
        v
    }

    /// A command line clicked (D423 / D429 for "Others...").
    fn menu_line(&mut self, assets: &mut Assets, audio: &mut Audio, i: usize) {
        match self.shown_lines().get(i).copied() {
            Some(Some(j)) => {
                if let Some(l) = self.menu.get(j).filter(|l| l.enabled) {
                    let cmd = l.cmd;
                    self.run(assets, audio, cmd);
                }
            }
            Some(None) => {
                self.menu_page = if (self.menu_page + 1) * 4 < self.menu.len() { self.menu_page + 1 } else { 0 };
                self.dirty = true;
            }
            None => {}
        }
    }

    /// Slot assignment (sub_13D83): character for each marker, in file order.
    fn assign_slots(&self, nslots: usize) -> Vec<Option<usize>> {
        if nslots == 0 {
            return vec![];
        }
        let mut slots = vec![None; nslots];
        let present = self.get16(0x12);
        let standing = self.get16(0x10) ^ present;
        let base = (self.mem[0xC5] & 0x0F) as usize;
        for c in 0..16 {
            if standing & present & (1 << c) == 0 {
                continue;
            }
            let s = (c + base) % nslots;
            if slots[s].is_none() {
                slots[s] = Some(c);
            } else if let Some(free) = slots.iter().position(|x| x.is_none()) {
                slots[free] = Some(c);
            }
        }
        slots.reverse();
        slots
    }

    /// sub_1388D: day/night palette for a sky view (ds:46DF = 1). Sets the colours at
    /// once, or retargets a running fade (48 steps).
    pub(crate) fn sky_palette(&mut self, assets: &Assets, pal: &mut Palette) {
        self.mem[0x46DF] = 1;
        let dn = self.mem[0x22E3] != 0;
        let idx = crate::sky::sky_index(assets, self.clock());
        let ranges = crate::sky::ranges(dn);
        if let Some(f) = &mut self.fade {
            if self.mem[0x46D6] != idx || f.dn != dn {
                let mut target = self.sky_cur.clone();
                crate::sky::apply(assets, &mut target, idx, dn);
                f.target = target;
                f.dn = dn;
                f.n = 0x30;
                self.mem[0x46D6] = idx;
            }
            for (first, n) in ranges {
                pal.0[first..first + n].copy_from_slice(&self.sky_cur.0[first..first + n]);
            }
        } else {
            self.mem[0x46D6] = idx;
            crate::sky::apply(assets, pal, idx, dn);
            for (first, n) in ranges {
                self.sky_cur.0[first..first + n].copy_from_slice(&pal.0[first..first + n]);
            }
        }
    }

    /// sub_138E1: new clock slot in a sky view -> start the 64-step fade (task 0x3916).
    pub(crate) fn sky_slot_change(&mut self, assets: &Assets) {
        if self.mem[0x46DF] == 0 {
            return;
        }
        let idx = crate::sky::sky_index(assets, self.clock());
        if self.mem[0x46D6] == idx {
            return;
        }
        let dn = self.mem[0x22E3] != 0;
        self.mem[0x46D6] = idx;
        let mut target = self.sky_cur.clone();
        crate::sky::apply(assets, &mut target, idx, dn);
        match &mut self.fade {
            Some(f) => {
                f.target = target;
                f.dn = dn;
                f.n = 0x40;
            }
            None => self.fade = Some(crate::sky::Fade { target, dn, n: 0x40, acc: 0 }),
        }
    }

    pub(crate) fn render_room(&mut self, assets: &mut Assets) {
        self.mem[0x46DF] = 0;
        self.mem[0x22E3] = 1;
        self.figures.clear();
        self.view.fill(0);
        let base_kind = if self.sal_kind == usize::MAX { 1 } else { self.sal_kind };
        let mut pal = assets.hnm(LOCATION_HNM[base_kind]).map(|h| h.palette).unwrap_or_default();
        self.harvester = None;
        if !self.indoors() {
            // Desert (sub_137EB): backdrop + silhouette + harvester.
            self.mem[0x47A4] |= 1;
            self.draw_backdrop(assets, &mut pal);
            self.desert_harvester(assets, &mut pal);
            self.palette = pal;
            self.ripple = None;
            self.ripple_fb.copy_from_slice(&self.view);
            self.dirty = true;
            return;
        }
        if self.mem[0x2B] != 0 {
            // War mode: the ATTACK scene replaces the room (sub_12DB1 -> loc_12DD3).
            let mut p = pal.clone();
            if self.war_view(assets, &mut p) {
                self.palette = p;
                self.ripple = None;
                self.figures.clear();
                self.ripple_fb.copy_from_slice(&self.view);
                self.dirty = true;
                return;
            }
        }
        self.attack = None;
        let place = self.place();
        let dl = place as u8;
        let mut v = self.entry[0];
        if place == 0x2005 || place == 0x1005 {
            if place == 0x1005 {
                self.mem[0x22E3] = 0;
            }
            self.sky_palette(assets, &mut pal);
            crate::sky::draw_bands(assets, &mut self.view);
        } else if dl == 1 {
            if (place >> 8) == 0x21 {
                v = self.mem[self.get16(0x114E) as usize];
            }
            self.draw_backdrop(assets, &mut pal);
        }
        let bank = bank_for_room_byte(v).and_then(|b| assets.sheet(b));
        if let Some(b) = &bank {
            b.apply_palette(&mut pal);
        }
        let pers: Option<Rc<SpriteSheet>> = assets.sheet("PERS.HSQ");
        if let Some(p) = &pers {
            p.apply_palette(&mut pal);
        }
        let room = self.sal.as_ref().and_then(|s| s.rooms.get(sal_room(v))).cloned();
        if let Some(room) = room {
            let slots = self.assign_slots(room.nslots as usize);
            let mut figures = vec![];
            let hide_people = self.mem[0x47A4] & 0x81 != 0;
            draw_room(&mut self.view, &room, bank.as_deref(), |i, m: &Marker, fb| {
                if hide_people {
                    return;
                }
                let Some(Some(c)) = slots.get(i) else { return };
                let Some(p) = &pers else { return };
                let opts = DrawOpts {
                    hflip: m.hflip,
                    vflip: false,
                    scale: SCALE[m.scale as usize],
                    pal_override: m.pal,
                    clip: Some((0, 0, VIEW_W as i32, VIEW_H as i32)),
                };
                let (x, y) = (m.x as i32, m.y as i32);
                for k in [2 * c, 2 * c + 1] {
                    if let Some(s) = p.sprites.get(k) {
                        s.draw(fb, VIEW_W, x, y, opts);
                    }
                }
                figures.push((*c, x, y));
            });
            self.figures = figures;
        }
        self.ripple = None;
        if self.mem[0x46DF] != 0 && dl == 1 {
            // Parked ornithopters (seg000:3A51): ORNY bank, one per orni, each +70,+10.
            if self.mem[0x4731] != 0xFF && self.mem[0x46FF] != 0 && self.mem[0x47A4] & 0x81 == 0 {
                self.mem[0x4731] = 0;
                if let Some(orny) = assets.sheet("ORNY.HSQ") {
                    let (mut x, mut y) = crate::travel::orni_base(self);
                    self.set16(0x472D, (x + 0xC) as u16);
                    self.set16(0x472F, (y + 8) as u16);
                    for _ in 0..self.mem[0x46FF] {
                        crate::travel::draw_orni_parts(&mut self.view, &orny, x, y, 0);
                        x += 0x46;
                        y += 0x0A;
                    }
                }
                self.sky_palette(assets, &mut pal);
            }
        } else if dl == 4 && (place >> 8) < 0x20 {
            self.ripple = Some(crate::sky::Ripple::new());
        }
        self.ripple_fb.copy_from_slice(&self.view);
        self.palette = pal;
        self.dirty = true;
    }

    /// sub_1380C: exterior / desert backdrop (with the SKYDN day palette).
    fn draw_backdrop(&mut self, assets: &mut Assets, pal: &mut Palette) {
        self.mem[0x22E3] = 1;
        self.sky_palette(assets, pal);
        if let Some(name) = crate::travel::backdrop_resource(self, assets).and_then(|r| assets.resource_name(r)) {
            if let Some(b) = assets.sheet(&name) {
                b.apply_palette(pal);
                if let Some(s) = b.sprites.first() {
                    s.draw(&mut self.view, VIEW_W, 0, 0, DrawOpts::default());
                }
            }
        }
    }

    pub(crate) fn say(&mut self, s: &[u8]) {
        self.message = Some((s.to_vec(), 4.0));
        self.dirty = true;
    }

    /// Compass: exits are N/E/S/W. 0 = none, 01..7F = room, 80..FA = locked, FB..FF = leave.
    fn exit_open(&self, dir: usize) -> bool {
        let v = self.entry[1 + dir];
        v != 0 && (v as i8) >= -5
    }

    /// Compass handlers seg000:3F15..3F24 (bp = dir + 1).
    pub(crate) fn go(&mut self, assets: &mut Assets, audio: &mut Audio, dir: usize) {
        if !self.indoors() {
            crate::travel::walk(self, assets, audio, dir);
            return;
        }
        if !self.exit_open(dir) {
            return;
        }
        match self.entry[1 + dir] {
            v @ 0xFB..=0xFF => crate::travel::leave_place(self, assets, audio, v),
            v => {
                // First visit of the place: flag 0x10 (sietches counted in ds:25).
                let loc = self.get16(0x114E) as usize;
                if loc >= 0x100 && self.mem[loc + 0x0A] & 0x10 == 0 {
                    self.mem[loc + 0x0A] |= 0x10;
                    if (self.place() >> 8) < 0x20 {
                        self.mem[0x25] = self.mem[0x25].wrapping_add(1);
                    }
                }
                self.enter(assets, (self.place() & 0xFF00) | v as u16)
            }
        }
    }

    /// TALK to character c (unused: see talk.rs).
    #[allow(dead_code)]
    fn talk_to(&mut self, assets: &mut Assets, audio: &mut Audio, c: usize) {
        let (Some(d), Some(cd)) = (&mut self.dialogue, &self.condit) else { return };
        let Some(r) = d.pick(c, cd, &self.mem) else {
            self.say(b"...");
            return;
        };
        let first_time = !r.said();
        d.mark_said(&r);
        let first = d.first_phrase(c);
        match r.action() {
            11 if first_time => self.mem[0x2A] = self.mem[0x2A].wrapping_add(1),
            12 => self.mem[0x2A] = (self.mem[0x2A] & 0xFC).wrapping_add(4),
            14 => self.mem[0xC2] = self.mem[0xC2].wrapping_add(1),
            _ => {}
        }
        let bank = if c < 6 { 1 } else { 2 };
        let text = assets.phrases(bank).map(|t| t.get(r.phrase as usize - 1).to_vec()).unwrap_or_default();
        let text = expand_substitutions(&text, assets);
        let pages = text.split(|&b| b == 0xFE).map(|p| p.to_vec()).collect();
        self.talk = Some(Talk { pages, page: 0, timer: 0.0, after: crate::talk::After::None });
        let letter = (b'A' + c.min(14) as u8) as char;
        let indoors = if self.indoors() && self.place() & 0xFF != 1 { 'I' } else { 'O' };
        audio.stop();
        let lips = play_voc(assets, audio, &format!("P{letter}\\P{letter}{:03X}{indoors}.VOC", r.phrase - first + 1));
        self.medallion_to = 0;
        self.open_closeup(assets, c, &text, lips.as_deref());
        self.dirty = true;
    }

    /// sub_13AF9 + loc_193AA: zoomed backdrop behind the speaker, then the animated face.
    pub(crate) fn open_closeup(&mut self, assets: &mut Assets, c: usize, text: &[u8], lips: Option<&[u8]>) {
        let now = self.ticks as u64;
        let speaker = self.figures.iter().find(|f| f.0 == c).map(|f| (f.1, f.2));
        let (idx, variant) = crate::face::bank_and_variant(c, &self.mem);
        let Some(bank) = assets.resource_name(idx + 2).and_then(|n| assets.dat.read(&n).ok()).and_then(dune_formats::face::FaceBank::parse) else {
            return;
        };
        self.mem[0x47A4] |= 0x80;
        // A vision (report told by an absent speaker): the face over VIS.HSQ.
        let vision = self.vision.is_some();
        if !vision {
            self.render_room(assets);
        }
        let backdrop = match speaker {
            Some((x, y)) if self.mem[0x2B] == 0 && !vision => crate::face::zoom_backdrop(&self.view, x, y),
            _ => self.view.clone(),
        };
        let mouth_rect = (idx < 17)
            .then(|| assets.exe_words(0x27FA + 8 * idx, 4))
            .flatten()
            .map(|w| (w[0], w[1], w[2], w[3]));
        let words = text.split(|&b| b == b' ' || b == 0xFE).filter(|w| !w.is_empty()).count() as i16;
        let params = crate::face::FaceParams {
            character: c,
            variant,
            f0: if c == 0x0C && self.mem[0x10A7] & 0x10 != 0 { 0x0A } else { 0 },
            shift: 0,
            mouth_rect,
            words,
            story_phase: self.mem[0x2A],
            seed: self.rng_seed(),
        };
        bank.sheet.apply_palette(&mut self.palette);
        let mut anim = crate::face::FaceAnim::new(bank, params, &backdrop, now);
        if c < 0x10 {
            anim.speak(lips, now);
        }
        self.closeup = Some(anim);
    }

    /// The game seeds its RNG from the BIOS tick count; any value is faithful.
    pub(crate) fn rng_seed(&self) -> u16 {
        (self.ticks as u64 as u16) ^ 0xA5C3
    }

    pub(crate) fn run(&mut self, assets: &mut Assets, audio: &mut Audio, cmd: Cmd) {
        match cmd {
            Cmd::SeeMap | Cmd::ExitMaps => self.see_dune_map(assets, audio),
            Cmd::Mixer => self.say(b"Music: AdLib (OPL2 emulation)."),
            Cmd::Mirror => self.open_mirror(assets),
            Cmd::LookAway => self.close_mirror(assets),
            Cmd::ExitGame => self.quit = true,
            Cmd::Restart => {
                if self.restart(assets) {
                    return;
                }
                self.game_over = None;
                if !self.load_slot(assets, 0) {
                    self.say(b"No restart save (dune37s0.sav).");
                }
            }
            Cmd::LoadMenu | Cmd::SaveMenu => {
                let load = cmd == Cmd::LoadMenu;
                self.slots_return = self.scr;
                self.menu.clear();
                for n in 1..=(if load { 4 } else { 2 }) {
                    let (label, exists) = self.slot_label(assets, n);
                    let c = if load { Cmd::Load(n) } else { Cmd::Save(n) };
                    self.menu.push(Line { label: Some(label), text: 0, cmd: c, enabled: !load || exists, hl: false });
                }
                self.menu.push(Line { label: None, text: 162, cmd: Cmd::CancelSlots, enabled: true, hl: false });
                self.view_mode = View::Slots;
            }
            Cmd::CancelSlots => {
                self.view_mode = if self.closeup.is_some() { View::Mirror } else { View::Room };
                self.build_menu();
            }
            Cmd::Save(n) => {
                let ok = self.save_slot(n);
                self.view_mode = if self.closeup.is_some() { View::Mirror } else { View::Room };
                self.build_menu();
                self.say(if ok { b" SAVE SUCCESSFUL" } else { b" *** SAVE ERROR " });
            }
            Cmd::Load(n) => {
                if self.scr == Scr::Globe {
                    self.globe_close();
                }
                self.game_over = None;
                self.mem[0x46D9] = 0;
                self.movie = None;
                if !self.load_slot(assets, n) {
                    self.say(b" *** LOAD ERROR ");
                }
            }
            Cmd::Character(c) => {
                if c == 0x0E {
                    self.select_fremen();
                } else {
                    self.talk_select(c);
                    self.talk_menu_for = Some(c);
                    self.build_menu();
                }
            }
            Cmd::Leader(k) => self.select_leader(k),
            Cmd::Talk(_) => self.talk_next(assets, audio),
            Cmd::What(_) => self.talk_what(assets, audio),
            Cmd::Come(c) => {
                self.mem[0x47A5] = 0xFF;
                self.talk_sub(assets, audio, c, 5, crate::talk::After::Come(c));
            }
            Cmd::Stay(c) => {
                self.mem[0x47A5] = 0xFF;
                self.talk_sub(assets, audio, c, 6, crate::talk::After::Stay(c));
            }
            Cmd::StopTalking => {
                if self.talk_menu_for == Some(0x0F) {
                    let si = self.get16(0x4758 + 2 * self.mem[0x476C] as usize) as usize;
                    self.sim().contact_end_story(si);
                }
                self.talk_menu_for = None;
                self.argue = None;
                self.build_menu();
            }
            Cmd::WorkForMe => self.work_for_me(assets, audio),
            Cmd::GiveOrdersRoom => self.give_orders_room(assets, audio),
            Cmd::Overpower => self.overpower(assets, audio),
            Cmd::Argue(a) => self.argue_answer(assets, audio, a),
            Cmd::Ord(n) => self.order(assets, audio, n),
            Cmd::ViewMessages(new) => self.view_messages(assets, audio, new),
            Cmd::Message(k) => self.view_message(assets, audio, k),
            Cmd::MessagesCancel => {
                self.msg_menu = None;
                self.build_menu();
            }
            Cmd::ContactTroops | Cmd::GiveOrders => self.troop_contact(assets, audio),
            Cmd::SpiceDensity => self.density_toggle(assets),
            Cmd::FindProspectors => self.find_prospectors(assets, audio),
            Cmd::MassiveAttack => self.massive_attack(assets),
            Cmd::FightDay => self.fight_day(assets),
            Cmd::TakeOrniMaps => crate::travel::take_orni_maps(self, assets, audio),
            Cmd::TakeOrni => crate::travel::take_orni(self, assets, audio),
            Cmd::CallWorm => crate::travel::call_worm(self, assets, audio),
            Cmd::GoThereOrni => crate::travel::go_there(self, assets, audio, false),
            Cmd::GoThereWorm => crate::travel::go_there(self, assets, audio, true),
            Cmd::CancelPopup => self.map_cancel_popup(),
            Cmd::WaitEvening | Cmd::WaitMorning => crate::travel::wait(self, assets, cmd == Cmd::WaitEvening),
            Cmd::CancelDest => crate::travel::cancel_dest(self, assets),
            Cmd::Skip => crate::travel::skip(self, assets, audio),
            Cmd::BackToStart => crate::travel::back_to_start(self, assets),
            Cmd::Nearest => crate::travel::nearest(self, assets),
            Cmd::ChangeDest => crate::travel::change_dest(self, assets, audio),
            Cmd::IgnoreWarning => crate::travel::ignore_warning(self),
            Cmd::ExitGlobe => self.globe_exit(assets),
            Cmd::SeeResults => self.globe_results(assets, true),
            Cmd::StandardVision => self.globe_results(assets, false),
            Cmd::SeeMapArea => {}
        }
        self.dirty = true;
    }

    /// One 200.3 Hz tick of the int-8 handler and the periodic task list.
    fn tick(&mut self, assets: &mut Assets, audio: &mut Audio) {
        self.now += 1;
        if self.movie.is_some() {
            self.movie_tick(assets, audio);
            return;
        }
        // Clock (seg000:EF6A).
        self.slot_timer = self.slot_timer.saturating_sub(1);
        if self.slot_timer == 0 {
            // ds:146E = 12000 ticks; DUNE_SLOT_TICKS shortens it for headless captures.
            self.slot_timer = std::env::var("DUNE_SLOT_TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(12000);
            let t = self.clock().wrapping_add(1);
            self.set16(2, t);
            self.new_slot(assets);
        }
        if let Some(t) = &mut self.trans {
            t.tick();
            if t.done {
                self.trans = None;
                self.dirty = true;
                self.after_transition(assets, audio);
            }
            return;
        }
        if self.seq.is_some() {
            crate::travel::seq_tick(self, assets, audio);
            if self.seq.as_ref().map(|s| s.blocks_tasks()).unwrap_or(false) {
                return;
            }
        }
        self.tasks(assets, audio);
    }

    /// Per-slot logic (sub_11B23): sun-dial, day/night fade, the simulation.
    pub(crate) fn new_slot(&mut self, assets: &mut Assets) {
        self.sky_slot_change(assets);
        self.slot_sim(assets);
        if self.scr == Scr::Room && !self.indoors() && !self.in_transit() {
            // The WAIT FOR EVENING / MORNING line follows the slot.
            self.build_menu();
        }
        self.dirty = true;
    }

    fn after_transition(&mut self, assets: &mut Assets, audio: &mut Audio) {
        if self.scr == Scr::Map {
            self.map_after_enter(assets);
        }
        if self.effect.is_none() {
            if let Some(a) = self.after_trans.take() {
                crate::travel::run_after(self, assets, audio, a);
            }
        }
    }

    fn tasks(&mut self, assets: &mut Assets, audio: &mut Audio) {
        // Task 0x3916: day/night fade, 16 ticks per step.
        if let Some(f) = &mut self.fade {
            f.acc += 1;
            if f.acc >= 16 {
                f.acc = 0;
                if self.mem[0x46DF] == 0 {
                    self.fade = None;
                } else {
                    let mut cur = self.sky_cur.clone();
                    let go_on = f.step(&mut cur);
                    for (first, n) in crate::sky::ranges(f.dn) {
                        self.sky_cur.0[first..first + n].copy_from_slice(&cur.0[first..first + n]);
                        self.palette.0[first..first + n].copy_from_slice(&cur.0[first..first + n]);
                        if let Some(fl) = &mut self.flight {
                            fl.pal.0[first..first + n].copy_from_slice(&cur.0[first..first + n]);
                        }
                    }
                    if !go_on {
                        self.fade = None;
                    }
                    self.dirty = true;
                }
            }
        }
        // Task 0xC0B6: sietch water ripple, 12 ticks (not while a mirror/talk menu is up).
        if self.scr == Scr::Room && self.closeup.is_none() {
            if let Some(r) = &mut self.ripple {
                r.acc += 1;
                if r.acc >= 12 {
                    r.acc = 0;
                    if r.step(&mut self.ripple_fb, &self.view) {
                        play_voc(assets, audio, "SN4.HSQ");
                    }
                    self.dirty = true;
                }
            }
        }
        // sub_117E6 / sub_1181E: medallion ramp, one step per 8 ticks.
        let e8 = self.mem[0xE8];
        let target = if self.in_transit() && self.medallion_to > e8 { e8 } else { self.medallion_to };
        if e8 != target {
            self.medallion_acc += 1;
            if self.medallion_acc >= 8 {
                self.medallion_acc = 0;
                self.mem[0xE8] = if e8 < target { e8 + 1 } else { e8 - 1 };
                self.dirty = true;
            }
        }
        // sub_1D7B7: follower-portrait blink every 64 ticks.
        let b = (self.now >> 6) & 0xFF;
        if b != self.blink_last {
            self.blink_last = b;
            for k in 0..2 {
                if self.mem[0x2222 + k] != 0 {
                    self.mem[0x2222 + k] -= 1;
                    self.dirty = true;
                }
            }
        }
        match self.scr {
            Scr::Map => {
                self.map_tick(assets);
                self.troop_anim_tick();
            }
            Scr::Globe => self.globe_tick(assets),
            Scr::Dest => crate::travel::dest_tick(self, assets),
            Scr::Room => {
                self.harvester_tick(assets);
                self.attack_tick();
                self.massive_tick(assets);
            }
            _ => {}
        }
        crate::travel::journey_tick(self, assets, audio);
    }

    /// Handler of a clicked panel zone (sub_1D6B7 dispatch on the handler word).
    fn zone_click(&mut self, assets: &mut Assets, audio: &mut Audio, z: usize) {
        let h = self.panel.z[z].handler;
        match h {
            0x3F15 | 0x3F1A | 0x3F1F | 0x3F24 => {
                if self.view_mode == View::Room && self.talk.is_none() {
                    self.go(assets, audio, (h as usize - 0x3F15) / 5)
                }
            }
            0xB8C6 => self.globe_enter(assets),
            0x5B05 => self.map_recentre(assets),
            0x8829 | 0x8824 | 0x882E | 0x881F => self.map_scroll(assets, h),
            0xBC81 => self.globe_exit(assets),
            0xB9B9 | 0xB9C0 | 0xB9CC | 0xB9D3 | 0xBA9E => self.globe_zone(assets, h),
            0x4AD0 | 0x4AD7 => crate::travel::turn(self, if h == 0x4AD0 { 0xFC } else { 4 }),
            0x4F09 => crate::travel::resume(self),
            _ => {}
        }
    }

    pub fn update(&mut self, dt: f32, input: &Input, assets: &mut Assets, audio: &mut Audio, screen: &mut Screen) {
        self.mouse = input.mouse;
        let before = self.ticks as u64;
        self.ticks += dt as f64 * crate::video::TICK_HZ as f64;
        for _ in before..self.ticks as u64 {
            self.tick(assets, audio);
            if let Some(f) = &mut self.closeup {
                if f.tick(self.ticks as u64) {
                    self.dirty = true;
                }
            }
        }
        // The main loop stores sub_1E3CC() in ds:0000 on every pass.
        let r = self.sim().rnd();
        self.set16(0, r);
        if self.trans.is_some() {
            self.show_trans(screen);
            return;
        }
        if input.any() {
            self.idle_since = self.now;
            self.pulse = None;
        }
        self.game_over_check(assets, audio);
        if self.movie.is_some() {
            if input.any() && matches!(self.movie, Some(crate::war_ui::Movie::Ending { .. })) {
                self.ending_skip = true;
            }
            self.dirty = true;
            self.finish_frame(assets, screen);
            return;
        }
        if self.now < self.busy_until {
            self.finish_frame(assets, screen);
            return;
        }
        if self.seq.as_ref().map(|s| s.blocks_input()).unwrap_or(false) {
            self.finish_frame(assets, screen);
            return;
        }
        if let Some(t) = &mut self.talk {
            t.timer += dt;
            let skip = input.clicked || input.pressed(Key::Space) || input.pressed(Key::Enter);
            if skip || (t.timer > 4.0 && audio.queued_secs() <= 0.0) {
                t.page += 1;
                t.timer = 0.0;
                if t.page >= t.pages.len() {
                    let after = t.after;
                    self.talk = None;
                    self.closeup = None;
                    self.mem[0x47A4] &= !0x80;
                    if skip {
                        audio.stop();
                    }
                    if self.scr == Scr::Room && self.vision.is_none() && self.movie.is_none() && self.game_over.is_none() {
                        self.render_room(assets);
                    }
                    self.medallion_to = 10;
                    self.run_after(assets, audio, after);
                    if self.talk.is_none() {
                        self.build_menu();
                    }
                }
                self.dirty = true;
            }
            self.finish_frame(assets, screen);
            return;
        }
        self.keys(input, assets, audio);
        self.mouse_input(input, assets, audio);
        self.events_tick(assets, audio);
        if let Some(v) = self.pending_voc.take() {
            play_voc(assets, audio, v);
        }
        if self.icons_dirty && self.scr == Scr::Map {
            self.icons_dirty = false;
            self.map_redraw(assets);
        }
        if let Some((_, t)) = &mut self.message {
            *t -= dt;
            if *t <= 0.0 {
                self.message = None;
                self.dirty = true;
            }
        }
        self.finish_frame(assets, screen);
    }

    fn keys(&mut self, input: &Input, assets: &mut Assets, audio: &mut Audio) {
        for &k in &input.keys {
            if self.trans.is_some() {
                break;
            }
            if let Key::Char(c @ '1'..='5') = k {
                let i = (c as u8 - b'1') as usize;
                self.menu_line(assets, audio, i);
                continue;
            }
            if k == Key::Char('q') {
                self.quit = true;
                continue;
            }
            match self.scr {
                Scr::Map => self.map_key(assets, audio, k),
                Scr::Globe => self.globe_key(assets, k),
                Scr::Dest => crate::travel::dest_key(self, assets, audio, k),
                Scr::Flight => crate::travel::flight_key(self, k),
                Scr::Room => match k {
                    Key::Up | Key::Right | Key::Down | Key::Left if self.view_mode != View::Room => {}
                    Key::Up => self.go(assets, audio, 0),
                    Key::Right => self.go(assets, audio, 1),
                    Key::Down => self.go(assets, audio, 2),
                    Key::Left => self.go(assets, audio, 3),
                    Key::Esc if self.talk_menu_for.is_some() => self.run(assets, audio, Cmd::StopTalking),
                    _ => {}
                },
            }
        }
    }

    fn mouse_input(&mut self, input: &Input, assets: &mut Assets, audio: &mut Audio) {
        let (mx, my) = input.mouse;
        // Move handlers run only when the mouse moved (or on a click).
        let moved = std::mem::replace(&mut self.last_mouse, (mx, my)) != (mx, my) || input.clicked || input.right_clicked;
        // Command lines (zones 7..11): hover and click.
        let hover = if (92..228).contains(&mx) && (159..199).contains(&my) { Some(((my - 159) / 8) as usize) } else { None };
        let shown = self.shown_lines();
        let hover = hover.filter(|&i| match shown.get(i) {
            Some(Some(j)) => self.menu.get(*j).map(|l| l.enabled).unwrap_or(false),
            Some(None) => true,
            None => false,
        });
        if hover != self.hover {
            self.hover = hover;
            self.dirty = true;
        }
        match self.scr {
            _ if !moved => {}
            Scr::Map => self.map_mouse(assets, audio, mx, my, input.clicked && hover.is_none(), input.right_clicked),
            Scr::Globe => self.globe_mouse(assets, mx, my, input.clicked && hover.is_none()),
            Scr::Dest => crate::travel::dest_mouse(self, assets, audio, mx, my, input.clicked && hover.is_none()),
            _ => {}
        }
        if input.clicked {
            if let Some(i) = hover {
                self.menu_line(assets, audio, i);
                return;
            }
            if let Some(z) = self.panel.hit(mx, my, 0, 18).or_else(|| self.panel.hit(mx, my, 21, 23)) {
                if self.panel.z[z].flags & 0x40 != 0 {
                    self.repeat = Some((z, self.now + 50));
                }
                self.zone_click(assets, audio, z);
                return;
            }
            if self.scr == Scr::Room && my < 152 && self.view_mode == View::Room {
                // Figure hit box: 32x80 from the marker (sub_19285).
                if let Some(&(c, _, _)) = self.figures.iter().find(|&&(_, x, y)| mx > x && mx <= x + 32 && my > y && my <= y + 80) {
                    self.run(assets, audio, Cmd::Character(c));
                } else if self.mem[0xB] == 1 && self.mem[5] != 0x21 && self.mem[0x2B] == 0 {
                    // Exterior room: a click in the view acts like the North arrow.
                    self.go(assets, audio, 0);
                }
            }
        } else if input.held {
            // Auto-repeat (flag 0x40) every 50 ticks while held on the same zone.
            if let Some((z, next)) = self.repeat {
                if self.panel.hit(mx, my, 12, 18) == Some(z) {
                    if self.now >= next {
                        self.repeat = Some((z, self.now + 50));
                        self.zone_click(assets, audio, z);
                    }
                } else {
                    self.repeat = None;
                }
            }
        } else {
            self.repeat = None;
        }
    }

    fn finish_frame(&mut self, assets: &mut Assets, screen: &mut Screen) {
        if self.dirty {
            self.dirty = false;
            let (fb, pal) = self.compose(assets);
            match self.effect.take() {
                Some(e) if self.via_black => {
                    // sub_1C0AD + effect: to a cleared screen first, then to the new one.
                    self.via_black = false;
                    let black = vec![0u8; 320 * 200];
                    self.trans = Some(Transition::new(e, &screen.fb, &screen.pal, &black, &screen.pal, 0));
                    self.effect = Some(e);
                    self.dirty = true;
                    self.show_trans(screen);
                }
                Some(e) => {
                    self.trans = Some(Transition::new(e, &screen.fb, &screen.pal, &fb, &pal, 0));
                    self.show_trans(screen);
                }
                None => {
                    screen.fb.copy_from_slice(&fb);
                    screen.pal = pal;
                }
            }
        }
    }

    fn show_trans(&self, screen: &mut Screen) {
        if let Some(t) = &self.trans {
            screen.fb.copy_from_slice(&t.fb);
            screen.pal = t.dac.clone();
        }
    }

    /// Builds the full 320x200 screen for the current state.
    fn compose(&mut self, assets: &mut Assets) -> (Vec<u8>, Palette) {
        if let Some((v, p)) = self.seq.as_ref().and_then(|s| s.full_view()) {
            return (v.to_vec(), p.clone());
        }
        if let Some((v, p)) = self.movie_frame() {
            return (v, p);
        }
        let mut fb = vec![0u8; 320 * 200];
        let pal = match self.scr {
            Scr::Room => {
                if let Some(a) = &self.attack {
                    fb[..VIEW].copy_from_slice(&a.view);
                } else if self.ripple.is_some() {
                    fb[..VIEW].copy_from_slice(&self.ripple_fb);
                } else {
                    fb[..VIEW].copy_from_slice(&self.view);
                }
                if let Some(f) = &self.closeup {
                    fb[..VIEW].copy_from_slice(&f.back);
                }
                self.palette.clone()
            }
            Scr::Map => self.map_compose(assets, &mut fb),
            Scr::Globe => self.globe_compose(assets, &mut fb),
            Scr::Dest => crate::travel::dest_compose(self, assets, &mut fb),
            Scr::Flight => crate::travel::flight_compose(self, assets, &mut fb),
        };
        if let Some(seq) = &self.seq {
            if let Some(v) = seq.view() {
                fb[..VIEW].copy_from_slice(v);
            }
        }
        self.draw_panel(assets, &mut fb);
        match self.scr {
            Scr::Map => {
                self.map_overlays(assets, &mut fb);
                self.troop_overlays(assets, &mut fb);
            }
            Scr::Dest => crate::travel::dest_overlays(self, assets, &mut fb),
            Scr::Flight => crate::travel::flight_overlays(self, assets, &mut fb),
            _ => {}
        }
        let pal = match &self.seq {
            Some(s) => s.palette().cloned().unwrap_or(pal),
            None => pal,
        };
        (fb, pal)
    }

    /// The panel (B23F order): zones 0..17, menu text, portraits, sun-dial, ornaments.
    pub(crate) fn draw_panel(&self, assets: &mut Assets, fb: &mut [u8]) {
        fill(fb, 0, 152, 320, 200, 0);
        let Some(ic) = assets.sheet("ICONES.HSQ") else { return };
        let put = |fb: &mut [u8], i: usize, x: i32, y: i32| {
            if let Some(s) = ic.sprites.get(i) {
                s.draw(fb, W, x, y, DrawOpts::default());
            }
        };
        for i in 0..12 {
            self.panel.draw_zone(i, fb, &ic);
        }
        if (3..=5).contains(&self.panel.z[2].sprite) {
            fill(fb, 254, 162, 296, 193, 0xF0);
        }
        for i in 12..18 {
            self.panel.draw_zone(i, fb, &ic);
        }
        // Ornaments of sub_11797 (straddling the view; the flight video covers y < 152).
        let flying = self.scr == Scr::Flight;
        let orn_clip = if flying { (0, 152, 320, 200) } else { (0, 0, 320, 200) };
        if let Some(s) = ic.sprites.get(15) {
            s.draw(fb, W, 126, 148, DrawOpts { clip: Some(orn_clip), ..Default::default() });
        }
        if let Some(s) = ic.sprites.get(16 + self.mem[0xE8].min(10) as usize) {
            s.draw(fb, W, 150, 137, DrawOpts { clip: Some(orn_clip), ..Default::default() });
        }
        if self.panel.z[1].sprite == 0 {
            // Follower portraits (zones 21/22, sub_1D763 + blink sub_1D7B7).
            for (k, x) in [35, 58].into_iter().enumerate() {
                let c = self.mem[0x1152 + k];
                let blank = self.mem[0x2222 + k] & 1 != 0;
                if c == 0xFF || blank {
                    put(fb, 64, x, 182);
                } else {
                    put(fb, 0x41 + (c as i8 as i32).max(0) as usize, x, 182);
                }
            }
            // Clock window: sun/moon per slot (ds:1E7E), then the day number.
            let t = self.clock();
            let slot = (t & 15) as usize;
            let tab = assets.exe_words(0x1E7E + 8 * slot, 4).unwrap_or(vec![0; 4]);
            let clip = Some((6, 181, 30, 190));
            for (k, spr) in [(0usize, 74usize), (2, 75)] {
                if tab[k] != 0 {
                    if let Some(s) = ic.sprites.get(spr) {
                        s.draw(fb, W, tab[k] as i32, tab[k + 1] as i32, DrawOpts { clip, ..Default::default() });
                    }
                }
            }
            let day = ((t + 3) >> 4) % 365 + 1;
            let digits = [day / 100, day / 10 % 10, day % 10];
            let mut s = vec![];
            for (i, &d) in digits.iter().enumerate() {
                let lead = i < 2 && digits[..=i].iter().all(|&x| x == 0);
                s.push(if lead { b' ' } else { b'0' + d as u8 });
            }
            s.push(b' ');
            let x = if day >= 100 { 11 } else if day >= 10 { 9 } else { 7 };
            text(fb, &assets.font2, x, 190, &s, C_TEXT, C_DAY_BG);
        }
        self.draw_menu(assets, fb);
    }

    /// sub_1D397 / sub_1D48A: the five command lines (or a talk page / message).
    fn draw_menu(&self, assets: &Assets, fb: &mut [u8]) {
        let small = &assets.font2;
        let cmd = assets.commands();
        let override_text = match (&self.talk, &self.message) {
            (Some(_), _) if self.tui.contact.is_some() && self.scr == Scr::Map => None,
            (Some(t), _) => t.pages.get(t.page).cloned(),
            (None, Some((m, _))) => Some(m.clone()),
            _ => None,
        };
        if let Some(txt) = override_text {
            for (k, l) in small.wrap(&txt, 130).iter().enumerate().take(5) {
                let y = 160 + 8 * k as i32;
                fill(fb, 93, y, 227, y + 7, C_BAR);
                text(fb, small, 93, y, &[b" ", &l[..]].concat(), C_TEXT, C_BAR);
            }
            return;
        }
        let shown = self.shown_lines();
        let others = Line::new(0xA0, Cmd::StopTalking, true);
        for (k, j) in shown.iter().enumerate() {
            let line = match j {
                Some(j) => &self.menu[*j],
                None => &others,
            };
            let y = 160 + 8 * k as i32;
            let hovered = self.hover == Some(k) || line.hl;
            let (fg, bg) = match (line.enabled, hovered) {
                (true, true) => (C_BAR, C_TEXT),
                (true, false) => (C_TEXT, C_BAR),
                (false, _) => (C_DISABLED, C_BAR),
            };
            let s = match &line.label {
                Some(l) => l.clone(),
                None => cmd.as_ref().map(|t| t.get(line.text).to_vec()).unwrap_or_default(),
            };
            let s = [b" ", &s[..]].concat();
            let end = text(fb, small, 93, y, &s, fg, bg);
            fill(fb, end, y, 227, y + 7, bg);
        }
    }
}

/// `80 hi lo` inserts COMMAND string (hi<<8|lo) (place names); other bytes 0x81..0xFD are
/// runtime words whose table is not reversed yet, so they are dropped.
fn expand_substitutions(s: &[u8], assets: &Assets) -> Vec<u8> {
    let cmd = assets.commands();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            0x80 if i + 2 < s.len() => {
                let id = (s[i + 1] as usize) << 8 | s[i + 2] as usize;
                if let Some(t) = &cmd {
                    let w = t.get(id);
                    out.extend_from_slice(w.strip_prefix(b" ").unwrap_or(w));
                }
                i += 3;
            }
            0x81..=0xFD => i += 1,
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}
