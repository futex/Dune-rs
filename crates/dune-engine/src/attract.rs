//! The attract intro (interpreter `sub_10580`, records at cs:0337), the credits and the
//! narrated prologue (`sub_10309`, `sub_1021C`), ported from DNCDPRG.EXE.
//! See docs/re/gameflow.md §2-3 and docs/re/animations.md §0-2, §4.3, §5.
//!
//! The port runs the game's own machine model on the 200.3 Hz timer clock:
//! * a back buffer (DBD6), a scratch buffer (DBDE) and the visible screen (`Screen::fb`);
//! * the driver palette buffers: shadow, target, dirty flags; `Screen::pal` is the DAC;
//! * the drawing origin cs:01A3 (24 lines during the intro records);
//! * the periodic task scheduler `sub_1D9D2` (one pass per input poll, i.e. per tick in
//!   every wait loop; no passes during transitions or busy waits);
//! * sprite banks with the "last bank whose palette was applied" rule (ds:DBB4);
//! * the two RNGs (`sub_1E3CC` -> ds:0000 at every poll, `sub_1E3B7`).
//! Waits on the music position (w0/w2) use the OPL player's position; holds (w5) only run
//! when no music plays, exactly like `sub_1DDF0`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use dune_formats::{
    face::FaceBank,
    globe::Globe,
    hnm::{Hnm, Picture},
    sprite::{DrawOpts, Sprite, SpriteSheet},
    voc,
};

use crate::{
    assets::Assets,
    audio::Audio,
    face::{bank_and_variant, FaceAnim, FaceParams},
    gfx::Screen,
    input::{Input, Key},
    intro::{start_music_ctl, stop_all, Outcome},
    transition::{PalState, Transition},
    video::TICK_HZ,
    world::World,
};

const W: usize = 320;
const H: usize = 200;
const PIX: usize = W * H;
const VIEW_H: usize = 152;
/// The view rectangle ds:1470.
const VIEW: (i32, i32, i32, i32) = (0, 0, 320, 152);

fn u16le(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn cols(b: &[u8]) -> Vec<[u8; 3]> {
    b.chunks_exact(3).map(|c| [c[0] & 63, c[1] & 63, c[2] & 63]).collect()
}

/// Palette chunk list (`first, count, rgb*` ... `FF FF`) written with k=2 (shadow + dirty).
/// Bank palettes (`sub_1C1AA`) skip a chunk that sets only colour 0; HNM headers end at
/// the first 0xFF.
fn k2_chunks(pal: &mut PalState, c: &[u8], hnm: bool) {
    let mut p = 0;
    while p + 2 <= c.len() {
        let (first, count) = (c[p], c[p + 1]);
        if first == 0xFF && (count == 0xFF || hnm) {
            break;
        }
        p += 2;
        if !hnm && first == 0 && count == 1 {
            p += 3;
            continue;
        }
        let n = if count == 0 { 256 } else { count as usize };
        let end = (p + 3 * n).min(c.len());
        pal.set(first as usize, &cols(&c[p..end]));
        p += 3 * n;
    }
}

/// A sprite bank: the raw resource (palette chunks, raw entries) plus decoded sprites.
pub(crate) struct Bank {
    raw: Vec<u8>,
    sheet: Option<Rc<SpriteSheet>>,
}

impl Bank {
    pub(crate) fn load(assets: &Assets, idx: usize) -> Option<Bank> {
        let name = assets.resource_name(idx)?;
        let raw = assets.dat.read(&name).ok()?;
        let sheet = SpriteSheet::parse(&raw).map(Rc::new);
        Some(Bank { raw, sheet })
    }

    pub(crate) fn entry(&self, i: usize) -> Option<&[u8]> {
        let d = &self.raw;
        let t = u16le(d, 0)? as usize;
        let n = u16le(d, t)? as usize / 2;
        if i >= n {
            return None;
        }
        let o = t + u16le(d, t + 2 * i)? as usize;
        let e = if i + 1 < n { t + u16le(d, t + 2 * i + 2)? as usize } else { d.len() };
        d.get(o..e.max(o).min(d.len()))
    }

    fn chunks(&self) -> &[u8] {
        let t = u16le(&self.raw, 0).unwrap_or(2) as usize;
        self.raw.get(2..t).unwrap_or(&[])
    }

    pub(crate) fn sprite(&self, i: usize) -> Option<&Sprite> {
        self.sheet.as_ref()?.sprites.get(i)
    }

    /// The sprite header words: width word (with its flag bits) and height.
    pub(crate) fn dims(&self, i: usize) -> (i32, i32) {
        match self.entry(i) {
            Some(e) if e.len() >= 3 => ((u16le(e, 0).unwrap_or(0) & 0x0FFF) as i32, e[2] as i32),
            _ => (0, 0),
        }
    }

    /// Palette data of entry `i` (6-byte header skipped).
    pub(crate) fn pal_entry(&self, i: usize, bytes: usize) -> Vec<[u8; 3]> {
        self.entry(i).and_then(|e| e.get(6..(6 + bytes).min(e.len()))).map(cols).unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TaskId {
    /// 0x70C: foreground HNM step (period 0).
    Hnm,
    /// 0x3916: day/night palette fade (16).
    SkyFade,
    /// 0x826: sunrise palette task (9 or 16).
    Sunrise,
    /// 0xC0B6: sietch water ripple (12).
    Ripple,
    /// 0xB45: sietch attack scene (3).
    Attack,
    /// 0xB9AE: progressive globe (1).
    Globe,
    /// 0x99BE: talking face (16).
    Face,
    /// 0xA7C2: lip-sync (0).
    Lips,
}

struct Task {
    period: u64,
    accum: u64,
    id: TaskId,
    serial: u32,
}

struct HnmPlay {
    hnm: Hnm,
    num: u8,
    flags: u16,
    next: usize,
    /// ds:DBE8 (frames shown, 1 after the open).
    count: u16,
    /// ds:DC22.
    last: u64,
    period: u64,
    sd_first: bool,
    sd_rate: u32,
    sd_last: f32,
}

impl HnmPlay {
    fn done(&self) -> bool {
        self.next >= self.hnm.frame_count()
    }
}

/// Digital sound effect with Creative repeat blocks (types 6/7) and the "break loop"
/// call of the driver (CT-VOICE fn 12, ds:3999).
#[derive(Default)]
struct Sfx {
    rate: u32,
    body: Vec<u8>,
    post: Vec<u8>,
    repeats: u32,
    looping: bool,
}

impl Sfx {
    fn play(&mut self, au: &mut Audio, data: &[u8]) {
        if !voc::is_voc(data) || data.len() < 26 {
            return;
        }
        let mut p = u16le(data, 20).unwrap_or(26) as usize;
        let (mut pre, mut body, mut post) = (Vec::new(), Vec::new(), Vec::new());
        let mut seg = 0; // 0 pre, 1 body, 2 post
        let mut rate = 11111;
        let mut count = 0u32;
        while p + 4 <= data.len() {
            let ty = data[p];
            if ty == 0 {
                break;
            }
            let len = data[p + 1] as usize | (data[p + 2] as usize) << 8 | (data[p + 3] as usize) << 16;
            let b = &data[p + 4..(p + 4 + len).min(data.len())];
            let out = match seg {
                0 => &mut pre,
                1 => &mut body,
                _ => &mut post,
            };
            match ty {
                1 if b.len() >= 2 => {
                    rate = 1_000_000 / (256 - b[0] as u32);
                    out.extend_from_slice(&b[2..]);
                }
                2 => out.extend_from_slice(b),
                3 if b.len() >= 3 => out.extend(std::iter::repeat(0x80).take(u16le(b, 0).unwrap_or(0) as usize + 1)),
                6 if b.len() >= 2 => {
                    count = u16le(b, 0).unwrap_or(0) as u32;
                    seg = 1;
                }
                7 => seg = 2,
                _ => {}
            }
            p += 4 + len;
        }
        self.rate = rate;
        let looped = seg != 0 && !body.is_empty();
        pre.extend_from_slice(&body);
        au.play_u8(&pre, rate);
        self.looping = looped;
        self.repeats = if count == 0xFFFF { u32::MAX } else { count };
        self.body = body;
        self.post = post;
    }

    /// Keeps a looping effect fed (called every tick).
    fn feed(&mut self, au: &mut Audio) {
        if self.looping && au.queued_secs() < 0.05 {
            if self.repeats == 0 {
                self.looping = false;
                au.play_u8(&self.post, self.rate);
            } else {
                if self.repeats != u32::MAX {
                    self.repeats -= 1;
                }
                au.play_u8(&self.body, self.rate);
            }
        }
    }

    fn break_loop(&mut self, au: &mut Audio) {
        if self.looping {
            self.looping = false;
            au.play_u8(&self.post, self.rate);
        }
    }
}

#[derive(Clone, Copy)]
struct Obj {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    sprite: u16,
    vel: u16,
    flags: u8,
    frac: u16,
}

/// ds:4856..485D.
#[derive(Default)]
struct Attack {
    s0: i8,
    s1: i8,
    s2: u8,
    s3: u8,
    s4: i16,
    s6: i8,
    s7: u8,
}

/// Shared machine + scene state for the intro and the prologue.
pub struct Core {
    now: u64,
    last_pass: u64,
    acc: f64,
    back: Vec<u8>,
    scratch: Vec<u8>,
    pal: PalState,
    /// cs:01A3 / 320.
    origin: usize,
    cur_bank: i32,
    pal_bank: i32,
    banks: HashMap<usize, Option<Rc<Bank>>>,
    d824: u16,
    d826: u16,
    ds0: u16,
    /// EXE data segment image (static tables, initial variables).
    mem: Vec<u8>,
    tasks: Vec<Task>,
    serial: u32,
    music: Option<Arc<AtomicU32>>,
    music_fade: Option<Arc<AtomicU32>>,
    track: u8,
    sfx: Sfx,
    trans: Option<Transition>,
    hnm: Option<HnmPlay>,
    face: Option<FaceAnim>,
    face_char: usize,
    /// ds:47D1 bit 7 (face animation enabled).
    face_on: bool,
    lips_none: bool,
    /// ds:46D6 / 46D7 / 46DF.
    sky_d6: u8,
    sky_d7: u8,
    sky_df: u8,
    rip_r: i32,
    rip_s: i32,
    rip_mode: u8,
    attack: Option<Attack>,
    objs: Vec<Obj>,
    globe: Option<Globe>,
    map: Vec<u8>,
    /// ds:4 / ds:12.
    place: u16,
    present: u16,
    /// Input seen but not yet consumed by a poll: Some(is_esc).
    pending: Option<bool>,
}

impl Core {
    fn new(assets: &Assets, screen: &Screen) -> Core {
        let mut mem = assets.exe.as_ref().map(|e| e[crate::assets::EXE_DS..].to_vec()).unwrap_or_default();
        mem.resize(0x10000, 0);
        let place = u16::from_le_bytes([mem[4], mem[5]]);
        let mut pal = PalState::new();
        pal.shadow = screen.pal.clone();
        pal.target = screen.pal.clone();
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u16).unwrap_or(0x1234);
        Core {
            now: 0,
            last_pass: 0,
            acc: 0.0,
            back: vec![0; PIX],
            scratch: vec![0; PIX],
            pal,
            origin: 0,
            cur_bank: -1,
            pal_bank: -1,
            banks: HashMap::new(),
            d824: seed,
            d826: seed ^ 0x5A5A,
            ds0: 0,
            mem,
            tasks: Vec::new(),
            serial: 0,
            music: None,
            music_fade: None,
            track: 0,
            sfx: Sfx::default(),
            trans: None,
            hnm: None,
            face: None,
            face_char: 0,
            face_on: false,
            lips_none: false,
            sky_d6: 0,
            sky_d7: 0,
            sky_df: 0,
            rip_r: 8,
            rip_s: 1,
            rip_mode: 0,
            attack: None,
            objs: Vec::new(),
            globe: None,
            map: Vec::new(),
            place,
            present: 0,
            pending: None,
        }
    }

    fn w16(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.mem[off], self.mem[off + 1]])
    }

    // ---------------------------------------------------------------- RNG, input, tasks

    /// sub_1E3CC.
    fn rnd(&mut self) -> u16 {
        let p = self.d826 as u32 * 0xCBD1;
        self.d826 = (p as u16).wrapping_add(1);
        (self.d826 >> 8) | ((((p >> 16) & 0xFF) as u16) << 8)
    }

    /// sub_1E3B7.
    fn rnd2(&mut self, mask: u16) -> u16 {
        let p = self.d824 as u32 * 0xE56D;
        self.d824 = (p as u16).wrapping_add(1);
        ((self.d824 >> 8) | ((((p >> 16) & 0xFF) as u16) << 8)) & mask
    }

    fn note_input(&mut self, input: &Input) {
        if self.pending.is_none() {
            if input.pressed(Key::Esc) {
                self.pending = Some(true);
            } else if input.any() {
                self.pending = Some(false);
            }
        }
    }

    /// sub_1DD63: input check; otherwise ds:0 = rnd() and one scheduler pass.
    /// Returns Some(esc) when input is pending.
    fn poll(&mut self, a: &mut Assets, au: &mut Audio, scr: &mut Screen) -> Option<bool> {
        if let Some(e) = self.pending {
            return Some(e);
        }
        self.ds0 = self.rnd();
        self.pass(a, au, scr);
        None
    }

    fn add_task(&mut self, id: TaskId, period: u64) {
        if self.tasks.len() < 20 {
            self.serial += 1;
            self.tasks.push(Task { period, accum: 0, id, serial: self.serial });
        }
    }

    fn remove_task(&mut self, id: TaskId) {
        if let Some(p) = self.tasks.iter().position(|t| t.id == id) {
            self.tasks.remove(p);
        }
    }

    /// sub_1D9D2.
    fn pass(&mut self, a: &mut Assets, au: &mut Audio, scr: &mut Screen) {
        let elapsed = self.now - self.last_pass;
        self.last_pass = self.now;
        let mut i = 0;
        let mut guard = 0;
        while i < self.tasks.len() && guard < 64 {
            guard += 1;
            let t = &mut self.tasks[i];
            let acc = t.accum + elapsed;
            if acc >= t.period {
                if t.period != 0 {
                    t.accum = acc % t.period;
                }
                let (id, serial) = (t.id, t.serial);
                self.run_task(id, a, au, scr);
                if let Some(p) = self.tasks.iter().position(|t| t.serial == serial) {
                    i = p + 1;
                }
            } else {
                t.accum = acc;
                i += 1;
            }
        }
    }

    fn run_task(&mut self, id: TaskId, a: &mut Assets, au: &mut Audio, scr: &mut Screen) {
        match id {
            TaskId::Hnm => {
                if self.hnm_step(au, scr, false) {
                    let f = self.hnm.as_ref().map(|h| h.flags).unwrap_or(0);
                    if f & 0x30 == 0x10 {
                        // sub_14B16: back lines 0..151 -> screen line 24.
                        scr.fb[24 * W..(24 + VIEW_H) * W].copy_from_slice(&self.back[..VIEW_H * W]);
                    }
                }
            }
            TaskId::SkyFade => self.sky_fade_task(scr),
            TaskId::Sunrise => self.sunrise_task(a, scr),
            TaskId::Ripple => self.ripple_task(a, au, scr),
            TaskId::Attack => self.attack_task(a, scr),
            TaskId::Globe => self.globe_task(scr),
            TaskId::Face => {
                if self.face_on {
                    if let Some(f) = &mut self.face {
                        if f.face_step() {
                            self.sync_face(scr);
                        }
                    }
                }
            }
            TaskId::Lips => {
                let now = self.now;
                let mut ended = true;
                let mut changed = false;
                if let Some(f) = &mut self.face {
                    changed = f.lip_step(now);
                    if self.lips_none && au.queued_secs() <= 0.0 {
                        f.stop_speech();
                    }
                    ended = !f.is_speaking();
                }
                if changed {
                    self.sync_face(scr);
                }
                if ended {
                    self.remove_task(TaskId::Lips);
                }
            }
        }
    }

    /// sub_10911: kill the scene tasks at the start of a record / page.
    fn kill_tasks(&mut self, au: &mut Audio, scr: &mut Screen) {
        self.remove_task(TaskId::Ripple);
        self.remove_task(TaskId::Globe);
        // sub_10B21: break the effect loop, stop the attack task, empty the object list.
        self.sfx.break_loop(au);
        self.remove_task(TaskId::Attack);
        self.objs.clear();
        // sub_19985: finish the face's 8-frame block (shown), then sub_198E6 (stop).
        if self.face_on {
            let mut changed = false;
            if let Some(f) = &mut self.face {
                changed = f.finish_block();
            }
            if changed {
                self.sync_face(scr);
            }
        }
        if let Some(f) = &self.face {
            if f.is_speaking() {
                // The voice file is closed (no more streaming).
                au.stop();
            }
        }
        self.face = None;
        self.remove_task(TaskId::Face);
        self.remove_task(TaskId::Lips);
        self.sky_d7 = 0;
        self.remove_task(TaskId::Hnm);
        self.remove_task(TaskId::SkyFade);
        self.remove_task(TaskId::Sunrise);
    }

    // ---------------------------------------------------------------- banks and drawing

    fn bank(&mut self, a: &Assets, idx: usize) -> Option<Rc<Bank>> {
        self.banks.entry(idx).or_insert_with(|| Bank::load(a, idx).map(Rc::new)).clone()
    }

    /// sub_1C13E: select a bank; its palette goes to the shadow when it differs from DBB4.
    fn select_bank(&mut self, a: &Assets, idx: usize) -> Option<Rc<Bank>> {
        let b = self.bank(a, idx);
        if self.cur_bank != idx as i32 {
            self.cur_bank = idx as i32;
            if self.pal_bank != idx as i32 {
                self.pal_bank = idx as i32;
                if let Some(b) = &b {
                    k2_chunks(&mut self.pal, b.chunks(), false);
                }
            }
        }
        b
    }

    fn cur(&mut self, a: &Assets) -> Option<Rc<Bank>> {
        if self.cur_bank < 0 {
            return None;
        }
        self.bank(a, self.cur_bank as usize)
    }

    /// Absolute clip rectangle from a view-relative one.
    fn clip_abs(&self, c: Option<(i32, i32, i32, i32)>) -> (i32, i32, i32, i32) {
        let o = self.origin as i32;
        match c {
            Some((x0, y0, x1, y1)) => (x0.max(0), (y0 + o).max(0), x1.min(W as i32), (y1 + o).min(H as i32)),
            None => (0, 0, W as i32, H as i32),
        }
    }

    /// sub_1C22F / sub_1C30D: sprite `w` (index | 0x4000 h-flip | 0x2000 v-flip) of the
    /// current bank at (x, y) + origin into the back buffer.
    fn put(&mut self, a: &Assets, w: u16, x: i32, y: i32, clip: Option<(i32, i32, i32, i32)>) {
        let Some(b) = self.cur(a) else { return };
        let Some(s) = b.sprite((w & 0x1FF) as usize) else { return };
        let o = DrawOpts { hflip: w & 0x4000 != 0, vflip: w & 0x2000 != 0, clip: Some(self.clip_abs(clip)), ..Default::default() };
        s.draw(&mut self.back, W, x, y + self.origin as i32, o);
    }

    /// sub_1C343: centred on (x, y), clipped to the view.
    fn put_centred(&mut self, a: &Assets, i: u16, x: i32, y: i32) {
        let Some(b) = self.cur(a) else { return };
        let (w, h) = b.dims(i as usize);
        self.put(a, i, x - (w >> 1), y - (h >> 1), Some(VIEW));
    }

    /// sub_1C21B / sub_1C32F: sprite list `{w, x, y}* FFFF` at ds:off.
    fn list(&mut self, a: &Assets, off: usize, clip: Option<(i32, i32, i32, i32)>) {
        let mut p = off;
        for _ in 0..64 {
            let w = self.w16(p);
            if w == 0xFFFF {
                break;
            }
            let (x, y) = (self.w16(p + 2) as i16 as i32, self.w16(p + 4) as i16 as i32);
            self.put(a, w, x, y, clip);
            p += 6;
        }
    }

    /// sub_1C2F2: full-screen picture = sprite 0 of a bank at (0, 0).
    fn still(&mut self, a: &Assets, idx: usize) {
        self.select_bank(a, idx);
        self.put(a, 0, 0, 0, None);
    }

    /// sub_1C0AD: clear the draw target (back buffer).
    fn clear_back(&mut self) {
        self.back.fill(0);
    }

    fn view_range(&self) -> std::ops::Range<usize> {
        self.origin * W..((self.origin + VIEW_H) * W).min(PIX)
    }

    /// Fill a view-relative rectangle with a colour (DNVGA fn 0x11B family).
    fn fill(&mut self, r: (i32, i32, i32, i32), c: u8) {
        let (x0, y0, x1, y1) = self.clip_abs(Some(r));
        for y in y0..y1 {
            for x in x0..x1 {
                self.back[y as usize * W + x as usize] = c;
            }
        }
    }

    /// sub_1C412: back buffer -> scratch.
    fn save_scratch(&mut self) {
        self.scratch.copy_from_slice(&self.back);
    }

    /// sub_1C4DD: view rect back -> screen.
    fn view_to_screen(&self, scr: &mut Screen) {
        let r = self.view_range();
        scr.fb[r.clone()].copy_from_slice(&self.back[r]);
    }

    fn sync_face(&mut self, scr: &mut Screen) {
        if let Some(f) = &self.face {
            let r = self.view_range();
            let n = r.len();
            self.back[r.clone()].copy_from_slice(&f.back[..n]);
            scr.fb[r].copy_from_slice(&f.back[..n]);
        }
    }

    // ---------------------------------------------------------------- music, sound

    fn music_pos(&self) -> Option<u32> {
        let p = self.music.as_ref()?.load(Ordering::Relaxed);
        (p != u32::MAX).then_some(p)
    }

    /// True while the music has not reached `pos` (sub_1DE0C); false without music.
    fn music_before(&self, pos: u16) -> bool {
        match self.music_pos() {
            Some(p) => (p as u16) < pos,
            None => false,
        }
    }

    /// sub_1AEB7.
    fn music_stop(&mut self, au: &mut Audio) {
        au.set_music(None);
        self.music = None;
        self.track = 0;
    }

    /// sub_1AD95: start track `t` (resource 0xA4 + t) unless it is the current one.
    fn music_track(&mut self, a: &Assets, au: &mut Audio, t: u8) {
        if t == self.track {
            return;
        }
        self.music_stop(au);
        const NAMES: [&str; 11] = ["", "SEKENCE", "WATER", "WORMSUIT", "WORMINTR", "WARSONG", "MORNING", "SIETCHM", "BAGDAD", "ARRAKIS", "CRYOMUS"];
        let name = a
            .resource_name(0xA4 + t as usize)
            .map(|n| n.trim_end_matches(".HSQ").to_string())
            .unwrap_or_else(|| NAMES.get(t as usize).unwrap_or(&"").to_string());
        let m = start_music_ctl(a, au, &name, 0);
        self.music = m.as_ref().map(|m| m.0.clone());
        self.music_fade = m.map(|m| m.1);
        self.track = t;
    }

    /// sub_1ADF8: music driver fade (entry +12) to volume `v` (min 4) over `dur`.
    fn music_fade(&self, v: u8, dur: u16) {
        if let Some(f) = &self.music_fade {
            f.store(0x8000_0000 | (v.max(4) as u32) << 16 | dur as u32, Ordering::Relaxed);
        }
    }

    /// sub_1AB15: sound effect n (resource 0xAE + n).
    fn sfx_play(&mut self, a: &Assets, au: &mut Audio, n: usize) {
        let Some(name) = a.resource_name(0xAE + n) else { return };
        if let Ok(d) = a.dat.read(&name) {
            au.stop();
            self.sfx.play(au, &d);
        }
    }

    fn digital_playing(&self, au: &Audio) -> bool {
        self.sfx.looping || au.queued_secs() > 0.0
    }

    // ---------------------------------------------------------------- HNM

    /// sub_1CA1B: open HNM `num` (resource 0xD2 + num) into the back buffer, `y_add` lines
    /// down (DBDA += 0x1E0 for 24), decode + draw frame 1, start its audio.
    fn hnm_open(&mut self, a: &Assets, au: &mut Audio, num: u8, y_add: usize) {
        self.hnm = None;
        let idx = 0xD2 + num as usize;
        let Some(name) = a.resource_name(idx) else { return };
        let flags = self.w16(0x31FF + 2 * idx);
        let flags = self.w16(flags as usize);
        let Ok(raw) = a.dat.read(&name) else { return };
        let hs = u16le(&raw, 0).unwrap_or(2) as usize;
        if let Some(c) = raw.get(2..hs) {
            k2_chunks(&mut self.pal, c, true);
        }
        let Some(hnm) = Hnm::parse(raw) else { return };
        let mut h = HnmPlay {
            hnm,
            num,
            flags,
            next: 0,
            count: 0,
            last: self.now,
            period: (flags >> 8).max(1) as u64,
            sd_first: true,
            sd_rate: 11111,
            sd_last: 0.0,
        };
        if h.hnm.frame_count() > 0 {
            let f = h.hnm.frame(0);
            if let Some(p) = &f.picture {
                draw_pic(&mut self.back, p, self.origin, y_add, num >= 0x19);
            }
            if let Some(sd) = f.sound {
                feed_sd(&mut h, sd, au);
            }
            h.next = 1;
            h.count = 1;
        }
        self.hnm = Some(h);
    }

    /// sub_1CA60: show the next frame if it is due. Returns true when a frame was shown.
    fn hnm_step(&mut self, au: &mut Audio, scr: &mut Screen, to_screen: bool) -> bool {
        let now = self.now;
        let Some(h) = &mut self.hnm else { return false };
        if h.done() {
            return false;
        }
        let f = h.hnm.frame(h.next);
        if f.sound.is_some() {
            // The frame waits until the buffer it will fill has been played.
            if au.queued_secs() > h.sd_last + 0.002 {
                return false;
            }
        } else if now - h.last < h.period {
            return false;
        }
        h.last = now;
        h.next += 1;
        h.count += 1;
        if let Some(p) = &f.palette {
            k2_chunks(&mut self.pal, p, true);
            self.pal.apply(&mut scr.pal);
        }
        if let Some(p) = &f.picture {
            let buf = if to_screen { &mut scr.fb } else { &mut self.back };
            draw_pic(buf, p, self.origin, 0, h.num >= 0x19);
        }
        if let Some(sd) = f.sound {
            feed_sd(h, sd, au);
        }
        true
    }

    fn hnm_done(&self) -> bool {
        self.hnm.as_ref().map(|h| h.done()).unwrap_or(true)
    }

    fn hnm_count(&self) -> u16 {
        self.hnm.as_ref().map(|h| h.count).unwrap_or(0)
    }

    // ---------------------------------------------------------------- rooms, faces

    /// sub_108F0 + sub_137B2 + sub_1C412: render a place into the view and save it.
    fn room(&mut self, a: &mut Assets, dx: u16, bx: u16) {
        self.place = dx;
        self.sky_df = 0;
        let (view, pal, _) = World::render_place(a, dx, bx, self.present, self.w16(2));
        let r = self.view_range();
        let n = r.len();
        self.back[r].copy_from_slice(&view[..n]);
        self.pal.set(0, &pal.0);
        // The renderer selected its own banks.
        self.cur_bank = -1;
        self.pal_bank = -1;
        self.save_scratch();
    }

    /// sub_109C7 + sub_1978E: character `c`'s face over the current back buffer.
    fn face(&mut self, a: &Assets, c: usize, shift: u16, words: i16) {
        let (idx, variant) = bank_and_variant(c, &self.mem);
        let Some(bank) = self.select_bank(a, idx + 2) else { return };
        let Some(fb) = FaceBank::parse(bank.raw.clone()) else { return };
        let mouth_rect = (idx < 17).then(|| {
            let o = 0x27FA + 8 * idx;
            (self.w16(o), self.w16(o + 2), self.w16(o + 4), self.w16(o + 6))
        });
        let seed = self.rnd2(0xFFFF);
        let params = FaceParams {
            character: c,
            variant,
            f0: if c == 0x0C && self.mem[0x10A7] & 0x10 != 0 { 0x0A } else { 0 },
            shift,
            mouth_rect,
            words,
            story_phase: self.mem[0x2A],
            seed,
        };
        let r = self.view_range();
        let anim = FaceAnim::new(fb, params, &self.back[r.clone()], self.now);
        let n = r.len();
        self.back[r].copy_from_slice(&anim.back[..n]);
        self.face = Some(anim);
        self.face_char = c;
        self.remove_task(TaskId::Face);
        self.add_task(TaskId::Face, 16);
    }

    /// 0798: ds:4780 = 0x190, sub_19EFD (speak with lip-sync).
    fn speak(&mut self, a: &Assets, au: &mut Audio, scr: &mut Screen) {
        let c = self.face_char;
        if self.face.is_none() || c >= 26 {
            return;
        }
        let l = (b'A' + c as u8) as char;
        let Some(v) = a.dat.read(&format!("P{l}\\P{l}190I.VOC")).ok().and_then(|d| voc::Voc::parse(&d)) else { return };
        au.stop();
        au.play_u8(&v.pcm, v.sample_rate);
        self.lips_none = v.lipsync.is_none();
        let now = self.now;
        if c < 0x10 {
            if let Some(f) = &mut self.face {
                f.speak(v.lipsync.as_deref(), now);
            }
            self.sync_face(scr);
        }
        self.remove_task(TaskId::Lips);
        self.add_task(TaskId::Lips, 0);
    }

    // ---------------------------------------------------------------- palette tasks

    /// sub_10802: SUNRS bank without its palette, entry `bl` -> colours 128..207, list 1500.
    fn sunrise_setup(&mut self, a: &Assets, bl: u8) {
        self.pal_bank = 0x2E;
        let Some(b) = self.select_bank(a, 0x2E) else { return };
        self.sky_d6 = bl;
        self.pal.set(128, &b.pal_entry(bl as usize, 0xF0));
        self.sky_d7 = 0;
        self.list(a, 0x1500, None);
    }

    /// loc_1391D: one k=39 step over colours 73..223 (ds:22E3 = 1, panel skipped: 227D = 1).
    fn sky_step(&mut self, scr: &mut Screen) {
        let n = self.sky_d7 as i32;
        self.pal.step(&mut scr.pal, 73, 151, n);
        self.sky_d7 = self.sky_d7.wrapping_sub(1);
        if self.sky_d7 == 0 {
            self.remove_task(TaskId::SkyFade);
        }
    }

    /// Task 0x3916.
    fn sky_fade_task(&mut self, scr: &mut Screen) {
        if self.sky_df == 0 {
            self.sky_d7 = 0;
            self.remove_task(TaskId::SkyFade);
            return;
        }
        self.sky_step(scr);
    }

    /// Task 0x826.
    fn sunrise_task(&mut self, a: &Assets, scr: &mut Screen) {
        if self.sky_d7 != 0 {
            return self.sky_step(scr);
        }
        let k = self.sky_d6.wrapping_add(1);
        if k == 0x0B {
            self.select_bank(a, 9);
            return;
        }
        if k == 0x0E {
            return;
        }
        self.sky_d7 = if k == 0x0D { 10 } else { 30 };
        self.pal_bank = 0x2E;
        if let Some(b) = self.select_bank(a, 0x2E) {
            self.sky_d6 = k;
            let e = b.pal_entry(k as usize, 0xF0);
            for (i, c) in e.iter().enumerate() {
                self.pal.target.0[128 + i] = *c;
            }
        }
        self.sky_step(scr);
    }

    /// loc_138A2 / sub_13971 / sub_139B9: SKYDN entry `bl` as the fade target (73..223).
    fn sky_target(&mut self, a: &Assets, bl: u8) {
        self.pal_bank = 0x29;
        if let Some(b) = self.select_bank(a, 0x29) {
            self.sky_d6 = bl;
            let e = b.pal_entry(bl as usize, 0x1C5);
            for (i, c) in e.iter().enumerate() {
                self.pal.target.0[73 + i] = *c;
            }
        }
    }

    // ---------------------------------------------------------------- ripple (§4.3)

    fn ripple_plot(&mut self, x: i32, y: i32, restore: bool, scr: &mut Screen) {
        let mut y = y;
        let d = 0x5B - y;
        if d >= 0 {
            y = 0x5B - (d >> 1);
        }
        if !(0x47..0x47 + 0x4E).contains(&y) || !(0..0x140).contains(&x) {
            return;
        }
        let p = (y as usize + self.origin) * W + x as usize;
        let at = |dx: usize, dy: usize| p + dy * W + dx;
        if restore {
            for dy in 0..4 {
                for dx in 0..4 {
                    let q = at(dx, dy);
                    if q < PIX && scr.fb[q] & 0x80 != 0 {
                        scr.fb[q] = self.back[q];
                    }
                }
            }
        } else {
            let all = (0..16).all(|k| {
                let q = at(k % 4, k / 4);
                q < PIX && scr.fb[q] & 0x80 != 0
            });
            let v = self.back[p];
            if all && v < 0xF0 {
                for k in 0..16 {
                    scr.fb[at(k % 4, k / 4)] = v;
                }
            }
        }
    }

    /// DNVGA 0x28EC: midpoint ellipse centred at (176, 91), one quadrant point per plot.
    fn ellipse(&mut self, r: i32, s: i32, restore: bool, scr: &mut Screen) {
        const M: i64 = 0xFFFF_FFFF;
        let a = ((r as i64) << 8) & M;
        let mut e = (2 * a + 512) & M;
        let mut d = (2 * a - 512) & M;
        let q = (a / s.max(1) as i64) & 0xFFFF;
        let mut k = ((q * q) >> 8) & M;
        let k2 = (2 * k) & M;
        let (mut xl, mut xr, mut yt, mut yb) = (0xB0 - r, 0xB0 + r, 0x5B, 0x5B);
        let mut plot = |me: &mut Core, xl: i32, xr: i32, yt: i32, yb: i32| {
            me.rip_mode = me.rip_mode.wrapping_add(1);
            let (x, y) = match me.rip_mode & 3 {
                0 => (xl, yb),
                1 => (xr, yb),
                2 => (xl, yt),
                _ => (xr, yt),
            };
            me.ripple_plot(x, y, restore, scr);
        };
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 10000 {
                return;
            }
            plot(self, xl, xr, yt, yb);
            let borrow = e < k;
            e = (e - k) & M;
            if !borrow {
                k = (k + k2) & M;
                yb += 1;
                yt -= 1;
                continue;
            }
            xl += 1;
            xr -= 1;
            let carry = e + d > M;
            e = (e + d) & M;
            d = (d - 512) & M;
            k = (k + k2) & M;
            if carry {
                yb += 1;
                yt -= 1;
                continue;
            }
            break;
        }
        loop {
            guard += 1;
            if guard > 20000 {
                return;
            }
            plot(self, xl, xr, yt, yb);
            xl += 1;
            xr -= 1;
            let carry = e + d > M;
            e = (e + d) & M;
            if carry {
                yb += 1;
                yt -= 1;
                e = (e - k) & M;
                k = (k + k2) & M;
            }
            d = (d - 512) & M;
            if d & 0x8000_0000 != 0 {
                break;
            }
        }
        plot(self, xl, xr, yt, yb);
    }

    /// Task 0xC0B6 (fn 0x15A effect 0x0C -> DNVGA 0x276C).
    fn ripple_task(&mut self, a: &Assets, au: &mut Audio, scr: &mut Screen) {
        let (r, s) = (self.rip_r, self.rip_s);
        if r != 8 {
            self.ellipse(r - 8, s - 1, true, scr);
        }
        self.ellipse(r, s, false, scr);
        let (nr, ns) = (r + 8, s + 1);
        if nr >= 0x212 {
            self.rip_r = 8;
            self.rip_s = 1;
        } else {
            self.rip_r = nr;
            self.rip_s = ns;
        }
        if nr == 0x18 {
            self.sfx_play(a, au, 4);
        }
    }

    // ---------------------------------------------------------------- attack scene (§2.7)

    /// sub_1C370: clear the rect, draw sprite `i` at its corner, replicate it over the rect.
    fn tile(&mut self, a: &Assets, i: u16, r: (i32, i32, i32, i32)) {
        self.fill(r, 0);
        self.put(a, i, r.0, r.1, None);
        let Some(b) = self.cur(a) else { return };
        let (w, h) = b.dims(i as usize);
        let (w, h) = ((w & 0x1FF).max(1), h.max(1));
        let o = self.origin as i32;
        for y in r.1..r.3 {
            for x in r.0..r.2 {
                let (sx, sy) = (r.0 + (x - r.0) % w, r.1 + (y - r.1) % h);
                let (d, s) = (((y + o) as usize) * W + x as usize, ((sy + o) as usize) * W + sx as usize);
                if d < PIX && s < PIX {
                    self.back[d] = self.back[s];
                }
            }
        }
    }

    fn attack_setup(&mut self, a: &Assets, au: &mut Audio) {
        self.select_bank(a, 0x2B);
        self.tile(a, 2, (0, 0, 320, 81));
        self.tile(a, 3, (0, 81, 320, 134));
        self.list(a, 0x11DD, None);
        self.attack = Some(Attack::default());
        self.remove_task(TaskId::Attack);
        self.add_task(TaskId::Attack, 3);
        self.save_scratch();
        self.sfx_play(a, au, 3);
    }

    /// sub_1C6AD: restore `r` from the scratch copy, redraw the objects over it, show it.
    fn redraw(&mut self, a: &Assets, r: (i32, i32, i32, i32), scr: &mut Screen) {
        let (x0, y0, x1, y1) = (r.0.max(VIEW.0), r.1.max(VIEW.1), r.2.min(VIEW.2), r.3.min(VIEW.3));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let o = self.origin as i32;
        for y in y0..y1 {
            let p = ((y + o) as usize) * W;
            self.back[p + x0 as usize..p + x1 as usize].copy_from_slice(&self.scratch[p + x0 as usize..p + x1 as usize]);
        }
        let objs: Vec<Obj> = self.objs.clone();
        for ob in objs.iter().filter(|ob| ob.flags & 0x80 == 0) {
            if ob.x0 < x1 && ob.y0 < y1 && ob.x1 > x0 && ob.y1 > y0 {
                self.put(a, ob.sprite, ob.x0, ob.y0, Some((x0, y0, x1, y1)));
            }
        }
        for y in y0..y1 {
            let p = ((y + o) as usize) * W;
            scr.fb[p + x0 as usize..p + x1 as usize].copy_from_slice(&self.back[p + x0 as usize..p + x1 as usize]);
        }
    }

    /// sub_1C60B: new object centred on (x, y) (not drawn).
    fn spawn(&mut self, a: &Assets, sprite: u16, x: i32, y: i32, vel: u16) {
        let Some(b) = self.cur(a) else { return };
        let (w, h) = b.dims(sprite as usize);
        let (x, y) = (x - (w >> 1), y - (h >> 1));
        self.objs.push(Obj { x0: x, y0: y, x1: x + w, y1: y + h, sprite, vel, flags: 0, frac: 0 });
    }

    /// sub_1C661: move object i by (dx, dy) and redraw the union of the old and new rects.
    fn move_obj(&mut self, a: &Assets, i: usize, dx: i32, dy: i32, scr: &mut Screen) {
        let o = &mut self.objs[i];
        let mut u = (o.x0, o.y0, o.x1, o.y1);
        o.x0 += dx;
        o.y0 += dy;
        o.x1 += dx;
        o.y1 += dy;
        if dx >= 0 {
            u.2 = o.x1;
        } else {
            u.0 = o.x0;
        }
        if dy >= 0 {
            u.3 = o.y1;
        } else {
            u.1 = o.y0;
        }
        self.redraw(a, u, scr);
    }

    /// sub_1C58A.
    fn remove_obj(&mut self, a: &Assets, i: usize, scr: &mut Screen) {
        if i >= self.objs.len() {
            return;
        }
        self.objs[i].flags |= 0x80;
        let o = self.objs[i];
        self.redraw(a, (o.x0, o.y0, o.x1, o.y1), scr);
        self.objs.remove(i);
    }

    /// seg000:0D0D: palette flash.
    fn flash(&mut self, a: &Assets, c: u8, scr: &mut Screen) {
        if c == 0x10 || c == 0x0A {
            let e = if c == 0x10 { 0x37 } else { 0x36 };
            if let Some(b) = self.select_bank(a, 0x2B) {
                self.pal.set(128, &b.pal_entry(e, 0x54));
                self.pal.apply(&mut scr.pal);
                for (k, col) in b.pal_entry(0x35, 0x54).iter().enumerate() {
                    self.pal.target.0[128 + k] = *col;
                }
            }
        } else {
            self.pal.step(&mut scr.pal, 128, 28, c as i32);
        }
    }

    /// seg000:0C3B.
    fn new_aircraft(&mut self, a: &Assets) {
        let mut st = self.attack.take().unwrap_or_default();
        st.s1 = st.s1.wrapping_sub(1);
        if st.s1 < 0 {
            if self.ds0 & 3 == 0 {
                st.s7 = 0x0B;
                if self.ds0 & 0x0C == 0 {
                    st.s7 = 0x11;
                }
            }
            let mut r = self.rnd();
            if self.mem[0x473A] != 0 {
                r &= !0x10;
            }
            let al = self.rnd2(7) as u8;
            st.s1 = al as i8;
            if al >= 4 {
                r |= 0x4000;
            }
            st.s2 = r as u8;
            st.s3 = (r >> 8) as u8;
        }
        st.s0 = 8;
        let (s2, s3) = (st.s2, st.s3);
        let (sprite, vel);
        if s2 & 0x10 == 0 {
            let t = (s3 & 6) as usize;
            vel = self.w16(0x15A2 + t);
            sprite = 4 + 2 * t as u16;
        } else {
            let mut al = s3 & 0x3F;
            let mut ah = s3 & 0xC0;
            if ah & 0x40 != 0 {
                let carry = self.ds0 & 0x8000 != 0;
                self.ds0 = self.ds0.rotate_left(1);
                if carry {
                    let cl: i8 = if ah & 0x80 != 0 { -10 } else { 10 };
                    let v = (al as i8).wrapping_add(cl);
                    al = v as u8;
                    if v < 0 {
                        ah ^= 0x80;
                        al = 0;
                    }
                    if al >= 0x40 {
                        al = 0x3F;
                        ah ^= 0x80;
                    }
                    st.s3 = ah | al;
                }
            }
            let (dx, by) = dir(al.wrapping_add(0xE0));
            vel = ((by as u8 as u16) << 8) | dx as u8 as u16;
            sprite = 0x14;
        }
        self.attack = Some(st);
        let k = (s2 & 0x0C) as usize;
        let (x, y) = (self.w16(0x1592 + k) as i16 as i32, self.w16(0x1594 + k) as i16 as i32);
        self.spawn(a, sprite, x, y, vel);
    }

    /// Task 0xB45 (every 3 ticks).
    fn attack_task(&mut self, a: &Assets, scr: &mut Screen) {
        let Some(mut st) = self.attack.take() else { return };
        st.s7 = st.s7.wrapping_sub(1);
        let s7 = st.s7;
        self.attack = Some(st);
        if (self.mem[0xEA] as i8) <= 0 && s7 <= 0x10 {
            self.flash(a, s7, scr);
        }
        if self.w16(0x479E) as u8 | self.mem[0x46EB] != 0 {
            return;
        }
        self.select_bank(a, 0x2B);
        let mut st = self.attack.take().unwrap_or_default();
        st.s4 = st.s4.wrapping_sub(1);
        if st.s4 < 0 {
            st.s6 = st.s6.wrapping_sub(1);
            if st.s6 < 0 {
                let r = self.rnd();
                st.s6 = (r & 0x7F) as i8;
                st.s4 = (r >> 8) as i16;
            } else {
                let r = self.rnd();
                let y = (r & 0x7F) as i32;
                if (0x30..0x60).contains(&y) {
                    let x = ((((r >> 7) & 1) << 8) | (r >> 8)) as i32;
                    if x < 320 {
                        self.spawn(a, 0x1C + (y & 7) as u16, x, y, 0);
                    }
                }
            }
        }
        st.s0 = st.s0.wrapping_sub(1);
        let spawn_new = st.s0 < 0;
        self.attack = Some(st);
        if spawn_new {
            self.new_aircraft(a);
        }
        // Object loop (a removal shifts the list but the cursor still advances).
        let n = self.objs.len();
        let mut i = 0;
        for _ in 0..n {
            if i >= self.objs.len() {
                i += 1;
                continue;
            }
            let o = self.objs[i];
            let mut s = o.sprite & 0xFF;
            let (mut mx, mut my) = ((o.vel & 0xFF) as u8 as i8 as i32, (o.vel >> 8) as u8 as i8 as i32);
            if s < 0x14 {
                let mut ax = o.sprite >> 2;
                for _ in 0..2 {
                    let c = self.ds0 >> 15;
                    self.ds0 = self.ds0.rotate_left(1);
                    ax = (ax << 1) | c;
                }
                s = ax;
            } else if s < 0x1C {
                let (sx, fx) = subpixel(o.vel as u8, o.frac as u8);
                let (sy, fy) = subpixel((o.vel >> 8) as u8, (o.frac >> 8) as u8);
                self.objs[i].frac = (fy as u16) << 8 | fx as u16;
                self.ds0 = self.ds0.rotate_left(3);
                s = 0x14 + (self.ds0 & 7);
                mx = sx;
                my = sy;
            } else {
                s += 1;
                if s > 0x2D {
                    self.remove_obj(a, i, scr);
                    i += 1;
                    continue;
                }
            }
            self.objs[i].sprite = s;
            self.move_obj(a, i, mx, my, scr);
            let o = self.objs[i];
            if !(0..0x140).contains(&o.x0) || o.x1 < 0 || o.y1 < 0 {
                self.remove_obj(a, i, scr);
            }
            i += 1;
        }
    }

    // ---------------------------------------------------------------- globe (§5)

    /// Task 0xB9AE: one globe line into the back buffer; on completion copy the rect
    /// (96,15)-(234,134) to the screen and rotate by one step.
    fn globe_task(&mut self, scr: &mut Screen) {
        let Some(g) = &mut self.globe else { return };
        if g.render_line(&mut self.back, &self.map) {
            for y in 15..134 {
                let p = y * W;
                scr.fb[p + 96..p + 234].copy_from_slice(&self.back[p + 96..p + 234]);
            }
            g.rotate(1);
        }
    }

    // ---------------------------------------------------------------- transitions, ticks

    /// sub_1C108 after the draw: start effect `fx` from the screen to the back buffer.
    fn start_transition(&mut self, fx: u8, scr: &mut Screen) -> bool {
        // fn 0x151 resets the ripple state.
        self.rip_r = 8;
        self.rip_s = 1;
        let t = Transition::start(fx, &scr.fb, &scr.pal, &self.back, self.pal.clone(), self.origin);
        scr.fb.copy_from_slice(&t.fb);
        scr.pal = t.dac.clone();
        if t.done {
            self.pal = t.pal;
            return true;
        }
        self.trans = Some(t);
        false
    }

    /// Advances the running transition; true when it is finished.
    fn transition_tick(&mut self, scr: &mut Screen) -> bool {
        let Some(t) = &mut self.trans else { return true };
        t.tick();
        scr.fb.copy_from_slice(&t.fb);
        scr.pal = t.dac.clone();
        if t.done {
            let t = self.trans.take().unwrap();
            self.pal = t.pal;
            return true;
        }
        false
    }

    /// Number of timer ticks to run for `dt` seconds.
    fn ticks_for(&mut self, dt: f32) -> u32 {
        self.acc += dt as f64 * TICK_HZ as f64;
        let n = self.acc.floor();
        self.acc -= n;
        (n as u32).min(64)
    }
}

/// sub_15198: unit vector (magnitude 0x20) for heading `al`.
pub(crate) fn dir(al: u8) -> (i16, i16) {
    let bl = al.wrapping_add(0x20);
    if bl & 0x7F >= 0x40 {
        let mut dx = 0x20i16;
        let mut a = al.wrapping_sub(0x40);
        if (bl as i8) < 0 {
            dx = -0x20;
            a = a.wrapping_sub(0x80).wrapping_neg();
        }
        (dx, a as i8 as i16)
    } else {
        let mut bx = -0x20i16;
        let mut a = al;
        if (bl as i8) < 0 {
            a = a.wrapping_sub(0x80).wrapping_neg();
            bx = 0x20;
        }
        (a as i8 as i16, bx)
    }
}

/// seg000:0CEA, one component: sign-magnitude velocity + 5-bit fraction.
pub(crate) fn subpixel(v: u8, frac: u8) -> (i32, u8) {
    let neg = (v as i8) < 0;
    let m = if neg { (v as i8).wrapping_neg() as u8 } else { v };
    let a = m.wrapping_add(frac);
    let step = (a >> 5) as i32;
    (if neg { -step } else { step }, a & 0x1F)
}

/// Decodes an `sd` chunk (the first one carries the VOC header) and queues it.
fn feed_sd(h: &mut HnmPlay, chunk: Vec<u8>, au: &mut Audio) {
    let pcm = if h.sd_first {
        h.sd_first = false;
        if voc::is_voc(&chunk) {
            let hdr = u16le(&chunk, 20).unwrap_or(26) as usize;
            if chunk.len() > hdr + 6 && chunk[hdr] == 1 {
                h.sd_rate = 1_000_000 / (256 - chunk[hdr + 4] as u32);
                chunk[hdr + 6..].to_vec()
            } else {
                Vec::new()
            }
        } else {
            chunk
        }
    } else {
        chunk
    };
    if !pcm.is_empty() {
        au.play_u8(&pcm, h.sd_rate);
        h.sd_last = pcm.len() as f32 / h.sd_rate as f32;
    }
}

/// Draws an HNM picture: full-screen frames go to line `y_add` without the origin; sprite
/// frames at (x, y) + origin; HNM >= 0x19 use the dot grid (pixel (i,j) -> (x+2i, y+2j)).
fn draw_pic(buf: &mut [u8], p: &Picture, origin: usize, y_add: usize, dot: bool) {
    let (w, h) = (p.width as usize, p.height as usize);
    if p.full {
        let w = w.min(W);
        for j in 0..h {
            let row = y_add + j;
            if row >= H {
                break;
            }
            buf[row * W..row * W + w].copy_from_slice(&p.pixels[j * p.width as usize..j * p.width as usize + w]);
        }
        return;
    }
    for j in 0..h {
        for i in 0..w {
            let c = p.pixels[j * w + i];
            if p.transparent && c == 0 {
                continue;
            }
            let (x, y) = if dot { (p.x as usize + 2 * i, p.y as usize + 2 * j) } else { (p.x as usize + i, p.y as usize + j) };
            let y = y + origin + y_add;
            if x < W && y < H {
                buf[y * W + x] = c;
            }
        }
    }
}

// ==================================================================== the attract interpreter

/// The 48 records of cs:0337 (fallback when the EXE is missing).
const RECORDS: [[u16; 6]; 48] = [
    [0x0000, 0x061C, 0x0000, 0x003A, 0x0625, 0x0001],
    [0x0000, 0xC0AD, 0x0000, 0x003A, 0x0F66, 0x0001],
    [0x0000, 0x064D, 0x0000, 0x0030, 0x0661, 0x0001],
    [0x0000, 0x0658, 0x006F, 0x0030, 0x0661, 0x0001],
    [0x0000, 0x0F66, 0x00A8, 0xFFFF, 0x0F66, 0x0001],
    [0x0000, 0x0678, 0x0000, 0x003A, 0x0684, 0x0001],
    [0x0000, 0xCEFC, 0x0000, 0x003A, 0xCF1B, 0x0001],
    [0x0000, 0xC0AD, 0x0000, 0x003A, 0x0F66, 0x0001],
    [0x0000, 0x069E, 0x0000, 0x0036, 0x0F66, 0x0190],
    [0x0000, 0x0F66, 0x0090, 0x0030, 0x06AA, 0x0190],
    [0x0000, 0x0F66, 0x010C, 0xFFFF, 0x06BD, 0x0001],
    [0x0000, 0x07FD, 0x0000, 0x003A, 0x085D, 0x04B0],
    [0x0148, 0x06CE, 0x014E, 0x0010, 0x0704, 0x1900],
    [0x024B, 0x0972, 0x024E, 0x0010, 0x0F66, 0x0085],
    [0x0000, 0x098A, 0x0258, 0x0010, 0x0F66, 0x0085],
    [0x0000, 0x0995, 0x0262, 0x0010, 0x0798, 0x0258],
    [0x02CA, 0x0771, 0x02CF, 0x0010, 0x0798, 0x0258],
    [0x032E, 0x07EE, 0x0330, 0x0010, 0x0F66, 0x00C8],
    [0x033C, 0x09A5, 0x0344, 0x0010, 0x0F66, 0x0085],
    [0x0000, 0x06D3, 0x034E, 0x0010, 0x0704, 0x2648],
    [0x04BE, 0x077C, 0x04C0, 0x0010, 0x0F66, 0x0085],
    [0x0000, 0x0788, 0x04D0, 0xFFFF, 0x078D, 0x0258],
    [0x050C, 0x07A3, 0x0510, 0x0010, 0x0798, 0x0258],
    [0x054C, 0x07C6, 0x0550, 0x0010, 0x0798, 0x0258],
    [0x0588, 0x0868, 0x058F, 0x0010, 0x087B, 0x0320],
    [0x05AC, 0x0886, 0x05AE, 0x0030, 0x0798, 0x0258],
    [0x05EE, 0x09AD, 0x05F0, 0x0030, 0x0798, 0x0258],
    [0x0614, 0x0F66, 0x0000, 0xFFFF, 0x08B6, 0x0032],
    [0x061E, 0x0ACD, 0x061E, 0x0010, 0x0F66, 0x07D0],
    [0x064C, 0x06D8, 0x0000, 0x0010, 0x06FC, 0x0C80],
    [0x06C6, 0x0740, 0x06C8, 0x003A, 0x0F66, 0x0190],
    [0x0000, 0x075A, 0x06EE, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0752, 0x06FE, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x073C, 0x070E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0756, 0x072E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x075E, 0x073E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0737, 0x074E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0747, 0x076E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x07E0, 0x077E, 0x0010, 0x0F66, 0x04B0],
    [0x07AC, 0x06EA, 0x07AF, 0x0010, 0x0704, 0x03E8],
    [0x07CE, 0x074B, 0x07CE, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0711, 0x07DC, 0x003A, 0x071D, 0x0001],
    [0x0000, 0x076A, 0x081C, 0x003A, 0x0F66, 0x0190],
    [0x0000, 0x0762, 0x083E, 0x0010, 0x0F66, 0x0190],
    [0x0000, 0x0766, 0x084E, 0x0010, 0x0F66, 0x00C8],
    [0x0000, 0xC0AD, 0x085E, 0x003A, 0x0F66, 0x0001],
    [0x0000, 0x076A, 0x085E, 0x0036, 0x0F66, 0x0190],
    [0x0000, 0xC0AD, 0x08E0, 0x0038, 0x0F66, 0x0001],
];

fn read_records(assets: &Assets) -> Vec<[u16; 6]> {
    let Some(e) = &assets.exe else { return RECORDS.to_vec() };
    let mut v = Vec::new();
    let mut p = 0x200 + 0x337;
    while let Some(w0) = u16le(e, p) {
        if w0 == 0xFFFF || v.len() >= 64 {
            break;
        }
        let mut r = [0u16; 6];
        for (k, x) in r.iter_mut().enumerate() {
            *x = u16le(e, p + 2 * k).unwrap_or(0xFFFF);
        }
        v.push(r);
        p += 12;
    }
    if v.is_empty() {
        RECORDS.to_vec()
    } else {
        v
    }
}

/// Play routines that loop over ticks.
enum Play {
    /// 0625 / 0661: frames into the back buffer, full copy to the screen.
    BackCopy { music_stop: bool },
    /// 06BD / 06AA: frames straight on the screen (origin 0), until the end or frame `stop`.
    Screen { stop: Option<u16> },
    /// CF1B: IRULAN with subtitles.
    Irulan,
    /// 071D: VER.HNM + SN8.
    Ver,
    /// 078D: face zoom (sub_1C868) then speak.
    Zoom { list: Vec<u8>, i: usize, center: (i32, i32), wait: u32, busy: u32 },
    /// 08B6: BACK slide.
    Slide { x: i32, wait: u32 },
}

pub struct Intro {
    core: Option<Core>,
    recs: Vec<[u16; 6]>,
    rec: usize,
    stage: u8,
    play: Option<Play>,
    hold_until: u64,
    hold_sound: bool,
    subs: Vec<u16>,
    sub_ptr: usize,
}

impl Intro {
    pub fn new() -> Intro {
        Intro { core: None, recs: Vec::new(), rec: 0, stage: 0, play: None, hold_until: 0, hold_sound: false, subs: Vec::new(), sub_ptr: 0 }
    }

    pub fn update(&mut self, dt: f32, input: &Input, assets: &mut Assets, audio: &mut Audio, screen: &mut Screen) -> Outcome {
        if self.core.is_none() {
            self.core = Some(Core::new(assets, screen));
            self.recs = read_records(assets);
            // loc_10580: k=41, stop the music.
            let c = self.core.as_mut().unwrap();
            c.pal.save();
            c.music_stop(audio);
            screen.brightness = 256;
        }
        let c = self.core.as_mut().unwrap();
        c.note_input(input);
        let n = c.ticks_for(dt);
        for _ in 0..n {
            let c = self.core.as_mut().unwrap();
            c.now += 1;
            c.sfx.feed(audio);
            if let Some(esc) = self.tick(assets, audio, screen) {
                return self.abort(esc, audio, screen);
            }
        }
        Outcome::Running
    }

    /// loc_105FD.
    fn abort(&mut self, esc: bool, audio: &mut Audio, screen: &mut Screen) -> Outcome {
        stop_all(audio);
        screen.fb.fill(0);
        if let Some(c) = &mut self.core {
            c.trans = None;
            c.hnm = None;
            c.origin = 0;
        }
        if esc {
            Outcome::Skip
        } else {
            Outcome::Credits
        }
    }

    /// One timer tick of the interpreter. Returns Some(esc) on abort.
    fn tick(&mut self, a: &mut Assets, au: &mut Audio, scr: &mut Screen) -> Option<bool> {
        for _ in 0..64 {
            let r = self.recs[self.rec];
            let c = self.core.as_mut().unwrap();
            match self.stage {
                0 => {
                    if std::env::var_os("DUNE_INTRO_TRACE").is_some() {
                        eprintln!("record {:2} tick {:6} music {:?} {:04X?}", self.rec, c.now, c.music_pos(), r);
                    }
                    c.origin = 24;
                    self.stage = 1;
                }
                // w0: music sync.
                1 => {
                    if c.music_before(r[0]) {
                        return c.poll(a, au, scr);
                    }
                    self.stage = 2;
                }
                // sub_10911, k=41, setup w1 (drawing into the back buffer).
                2 => {
                    if std::env::var_os("DUNE_INTRO_TRACE").is_some() {
                        eprintln!("  setup {:04X} tick {:6} music {:?}", r[1], c.now, c.music_pos());
                    }
                    c.kill_tasks(au, scr);
                    c.pal.save();
                    self.setup(r[1], a, au, scr);
                    let c = self.core.as_mut().unwrap();
                    c.face_on = false;
                    c.remove_task(TaskId::Ripple);
                    self.stage = 3;
                }
                // w2: music sync.
                3 => {
                    if c.music_before(r[2]) {
                        return c.poll(a, au, scr);
                    }
                    self.stage = 4;
                }
                // w3: transition.
                4 => {
                    if (r[3] as i16) < 0 {
                        self.stage = 6;
                        continue;
                    }
                    self.stage = 5;
                    if !c.start_transition(r[3] as u8, scr) {
                        return None;
                    }
                }
                5 => {
                    if c.trans.is_some() && !c.transition_tick(scr) {
                        return None;
                    }
                    // sub_1C0F4 + sub_13A7C.
                    c.pal.apply(&mut scr.pal);
                    c.remove_task(TaskId::Ripple);
                    if c.place & 0xFF == 4 && (c.place >> 8) < 0x20 {
                        c.add_task(TaskId::Ripple, 12);
                    }
                    self.stage = 6;
                }
                // Poll, then the play routine w4.
                6 => {
                    if std::env::var_os("DUNE_INTRO_TRACE").is_some() {
                        eprintln!("  play  {:04X} tick {:6}", r[4], c.now);
                    }
                    c.face_on = true;
                    if let Some(e) = c.poll(a, au, scr) {
                        return Some(e);
                    }
                    self.stage = 8;
                    self.start_play(r[4], a, au, scr);
                    if self.play.is_some() {
                        self.stage = 7;
                        return None;
                    }
                }
                7 => match self.play_tick(a, au, scr) {
                    Ok(true) => {
                        self.play = None;
                        self.stage = 8;
                    }
                    Ok(false) => return None,
                    Err(e) => return Some(e),
                },
                // w5: hold (sub_1DDF0): nothing while music plays.
                8 => {
                    self.stage = 10;
                    if r[5] != 0 && c.music_pos().is_none() {
                        self.hold_sound = c.digital_playing(au);
                        self.hold_until = c.now + r[5] as u64;
                        self.stage = 9;
                    }
                }
                9 => {
                    let done = if self.hold_sound { !c.digital_playing(au) } else { c.now > self.hold_until };
                    if !done {
                        return c.poll(a, au, scr);
                    }
                    self.stage = 10;
                }
                _ => {
                    self.rec += 1;
                    if self.rec >= self.recs.len() {
                        // End of table: jump back to the start (k=41, stop the music).
                        self.rec = 0;
                        c.pal.save();
                        c.music_stop(au);
                    }
                    self.stage = 0;
                }
            }
        }
        None
    }

    /// Setup routines (w1), run with the screen redirected to the back buffer.
    fn setup(&mut self, w1: u16, a: &mut Assets, au: &mut Audio, _scr: &mut Screen) {
        let c = self.core.as_mut().unwrap();
        match w1 {
            0x061C => {
                c.music_stop(au);
                c.music_track(a, au, 6);
                c.hnm_open(a, au, 0x15, 0);
            }
            0xC0AD => c.clear_back(),
            0x064D => {
                c.music_track(a, au, 0x0A);
                c.hnm_open(a, au, 0x16, 0);
            }
            0x0658 => {
                c.clear_back();
                c.hnm_open(a, au, 0x17, 0);
            }
            0x0678 => {
                c.origin = 0;
                c.clear_back();
                c.hnm_open(a, au, 0x18, 0);
            }
            0xCEFC => {
                let lang = a.lang.max(1) as usize - 1;
                c.select_bank(a, 0x69 + lang);
                self.subs = (0..64).map(|k| c.w16(0x35A8 + 2 * k)).collect();
                self.sub_ptr = 0;
                c.origin = 0;
                c.clear_back();
                c.hnm_open(a, au, 0x19, 0);
            }
            0x069E => {
                c.origin = 0;
                c.hnm_open(a, au, 0x0F, 0);
            }
            0x07FD => {
                c.clear_back();
                c.sunrise_setup(a, 8);
            }
            0x06CE => c.hnm_open(a, au, 0x10, 24),
            0x06D3 => c.hnm_open(a, au, 0x11, 24),
            0x06D8 => {
                c.sky_df = 1;
                c.sky_d7 = 0x30;
                c.sky_target(a, 0x12);
                c.hnm_open(a, au, 0x12, 24);
            }
            0x06EA => {
                c.place = 2;
                c.hnm_open(a, au, 0x13, 24);
            }
            0x0711 => c.hnm_open(a, au, 0x0E, 24),
            0x0972 => {
                c.clear_back();
                c.present = 0;
                c.room(a, 0x2002, 0x180);
            }
            0x098A => {
                c.present = 2;
                c.room(a, 0x2004, 0x180);
            }
            0x0995 => {
                c.present = 0;
                c.room(a, 0x200A, 0x180);
                c.face(a, 0, 0, 0x1E);
            }
            0x0771 => {
                c.present = 0;
                c.room(a, 0x2004, 0x180);
                c.face(a, 1, 0, 0x1E);
            }
            0x07EE => {
                c.select_bank(a, 0x30);
                c.list(a, 0x1526, None);
                c.save_scratch();
                c.face(a, 0x2D, 0, 1);
            }
            0x09A5 => {
                c.clear_back();
                c.still(a, 0x62);
            }
            0x077C => {
                c.clear_back();
                c.room(a, 0x0803, 0x1080);
            }
            0x0788 => c.face(a, 7, 0, 0x1E),
            0x07A3 => {
                c.select_bank(a, 0x2E);
                c.list(a, 0x1500, None);
                c.put(a, 1, 0x54, 0x0B, None);
                c.save_scratch();
                c.face(a, 6, 0, 0x1E);
            }
            0x07C6 => {
                c.present = 0x100;
                c.room(a, 0x0802, 0x1080);
                c.present = 0;
                c.face(a, 5, 0, 0x1E);
            }
            0x0868 => {
                c.sunrise_setup(a, 0x0B);
                c.put(a, 7, 0x13, 0x4A, None);
            }
            0x0886 => {
                c.select_bank(a, 0x30);
                c.fill(VIEW, 0xDE);
                c.list(a, 0x154E, None);
                c.list(a, 0x155C, Some(VIEW));
                c.save_scratch();
                c.face(a, 0x0A, 0x3A, 0x1E);
            }
            0x09AD => {
                c.select_bank(a, 0x30);
                c.list(a, 0x153A, None);
                c.save_scratch();
                c.face(a, 9, 0x52, 0x1E);
            }
            0x0ACD => c.attack_setup(a, au),
            0x07E0 => {
                // sub_1ABDB: SN4 is only loaded here; the ripple task plays it.
                c.room(a, 0x0804, 0x1080);
            }
            0x0737 | 0x073C | 0x0740 | 0x0747 | 0x074B | 0x0752 | 0x0756 | 0x075A | 0x075E | 0x0762 | 0x0766 | 0x076A => {
                let (clear, idx) = match w1 {
                    0x0737 => (false, 0x56),
                    0x073C => (false, 0x57),
                    0x0740 => (true, 0x58),
                    0x0747 => (false, 0x59),
                    0x074B => (true, 0x5A),
                    0x0752 => (false, 0x5B),
                    0x0756 => (false, 0x5C),
                    0x075A => (false, 0x5D),
                    0x075E => (false, 0x5E),
                    0x0762 => (false, 0x5F),
                    0x0766 => (false, 0x60),
                    _ => (true, 0x61),
                };
                if clear {
                    c.clear_back();
                }
                c.still(a, idx);
            }
            _ => {}
        }
    }

    /// Play routines (w4): instant ones run here, looping ones set `self.play`.
    fn start_play(&mut self, w4: u16, a: &mut Assets, au: &mut Audio, scr: &mut Screen) {
        let c = self.core.as_mut().unwrap();
        self.play = match w4 {
            0x0625 => Some(Play::BackCopy { music_stop: true }),
            0x0661 => Some(Play::BackCopy { music_stop: false }),
            0x0684 | 0x06BD => {
                c.origin = 0;
                Some(Play::Screen { stop: None })
            }
            0x06AA => {
                c.origin = 0;
                Some(Play::Screen { stop: Some(0x56) })
            }
            0xCF1B => Some(Play::Irulan),
            0x071D => {
                c.sfx_play(a, au, 8);
                Some(Play::Ver)
            }
            0x085D => {
                c.add_task(TaskId::Sunrise, 9);
                None
            }
            0x087B => {
                c.add_task(TaskId::Sunrise, 16);
                None
            }
            0x06FC | 0x0704 => {
                if w4 == 0x06FC {
                    c.sky_d7 = 0x3F;
                    c.add_task(TaskId::SkyFade, 16);
                }
                c.add_task(TaskId::Hnm, 0);
                c.run_task(TaskId::Hnm, a, au, scr);
                None
            }
            0x0798 => {
                c.speak(a, au, scr);
                None
            }
            0x078D => {
                c.pal.apply(&mut scr.pal);
                // sub_1C868: no zoom while a voice plays or without a centre.
                let (idx, _) = bank_and_variant(c.face_char, &c.mem);
                let cen = if idx < 0x11 { (c.w16(0x27B6 + 4 * idx) as i32, c.w16(0x27B8 + 4 * idx) as i32) } else { (0, 0) };
                if c.face.is_none() || au.queued_secs() > 0.0 || cen == (0, 0) {
                    c.speak(a, au, scr);
                    None
                } else {
                    // ds:227D = 1: list ds:2792.
                    let list: Vec<u8> = (0..16).map(|k| c.mem[0x2792 + k]).take_while(|&b| b != 0).collect();
                    Some(Play::Zoom { list, i: 0, center: cen, wait: 0, busy: 0 })
                }
            }
            0x08B6 => {
                c.select_bank(a, 0x30);
                c.save_scratch();
                Some(Play::Slide { x: -0x60, wait: 0 })
            }
            _ => None,
        };
    }

    /// One tick of a looping play routine: Ok(true) when finished.
    fn play_tick(&mut self, a: &mut Assets, au: &mut Audio, scr: &mut Screen) -> Result<bool, bool> {
        let c = self.core.as_mut().unwrap();
        let Some(play) = &mut self.play else { return Ok(true) };
        match play {
            Play::BackCopy { music_stop } => {
                if let Some(e) = c.poll(a, au, scr) {
                    return Err(e);
                }
                if c.hnm_step(au, scr, false) {
                    scr.fb.copy_from_slice(&c.back);
                    if *music_stop && c.music_pos().map(|p| p >= 128).unwrap_or(false) && c.track != 0 {
                        c.music_stop(au);
                    }
                    if c.hnm_done() {
                        return Ok(true);
                    }
                }
                Ok(c.hnm.is_none())
            }
            Play::Screen { stop } => {
                c.hnm_step(au, scr, true);
                if let Some(e) = c.poll(a, au, scr) {
                    return Err(e);
                }
                Ok(match stop {
                    Some(n) => c.hnm_count() == *n || c.hnm_done(),
                    None => c.hnm_done(),
                })
            }
            Play::Irulan => {
                // ds:3622: on/off frame numbers.
                if let Some(&f) = self.subs.get(self.sub_ptr) {
                    if f <= c.hnm_count() {
                        let k = self.sub_ptr;
                        self.sub_ptr += 1;
                        if k % 2 == 0 {
                            if let Some(s) = c.cur(a).and_then(|b| b.sprite(k / 2).cloned()) {
                                s.draw(&mut scr.fb, W, 0, 190 + c.origin as i32, DrawOpts::default());
                            }
                        } else {
                            scr.fb[190 * W..].fill(0);
                        }
                    }
                }
                c.hnm_step(au, scr, true);
                if let Some(e) = c.poll(a, au, scr) {
                    return Err(e);
                }
                if c.hnm_done() {
                    // Close, stop the voice, restart MORNING (sub_1AD57).
                    c.hnm = None;
                    c.music_stop(au);
                    c.music_track(a, au, 6);
                    return Ok(true);
                }
                Ok(false)
            }
            Play::Ver => {
                if let Some(e) = c.poll(a, au, scr) {
                    return Err(e);
                }
                c.hnm_step(au, scr, false);
                // sub_14B16: back lines 0..151 -> screen line 24.
                scr.fb[24 * W..(24 + VIEW_H) * W].copy_from_slice(&c.back[..VIEW_H * W]);
                if c.hnm_count() == 11 {
                    c.sfx.break_loop(au);
                }
                Ok(c.hnm_done())
            }
            Play::Zoom { list, i, center, wait, busy } => {
                // No scheduler passes during the zoom.
                if *busy > 0 {
                    *busy -= 1;
                    return Ok(false);
                }
                if *wait > 0 {
                    *wait -= 1;
                    if *wait > 0 {
                        return Ok(false);
                    }
                }
                loop {
                    let Some(&b) = list.get(*i) else {
                        // Back to x1, then speak.
                        c.view_to_screen(scr);
                        c.speak(a, au, scr);
                        return Ok(true);
                    };
                    *i += 1;
                    if b & 0x80 != 0 {
                        *busy = 0x12C;
                        return Ok(false);
                    }
                    zoom_frame(c, b as usize, *center, scr);
                    *wait = c.mem[0xDBE6].max(6) as u32;
                    return Ok(false);
                }
            }
            Play::Slide { x, wait } => {
                if *wait > 0 {
                    *wait -= 1;
                    return Ok(false);
                }
                // Restore the view from the scratch copy, BACK 5 and 6 at (x, 13), show.
                let r = c.view_range();
                let s = c.scratch[r.clone()].to_vec();
                c.back[r].copy_from_slice(&s);
                c.put(a, 5, *x, 13, Some(VIEW));
                c.put(a, 6, *x, 13, Some(VIEW));
                c.view_to_screen(scr);
                *x += 0x20;
                *wait = 2;
                Ok(*x > 0)
            }
        }
    }
}

/// sub_1C8C1 + DNVGA 0x3A14: zoom kernel `f` of the back-buffer rect around `center`
/// to the screen view (152 lines at the origin).
fn zoom_frame(c: &Core, f: usize, center: (i32, i32), scr: &mut Screen) {
    let hw = c.w16(0x2796 + 4 * f) as i32;
    let hh = c.w16(0x2798 + 4 * f) as i32;
    let sx = (center.0 - hw).max(0) as usize;
    let sy = (center.1 - hh).max(0) as usize;
    // Destination column -> source column, destination row -> source row (None = not written).
    let (xm, ym): (Vec<usize>, Vec<Option<usize>>) = match f {
        1 => (group_map(320, 7, &[2, 1, 1, 1, 1, 1, 1]), group_map(152, 7, &[1, 1, 1, 1, 1, 1, 2]).into_iter().map(Some).collect()),
        2 => (group_map(320, 3, &[2, 1, 1]), group_map(152, 3, &[1, 1, 2]).into_iter().map(Some).collect()),
        3 => {
            let mut ym: Vec<Option<usize>> = group_map(150, 2, &[1, 2]).into_iter().map(Some).collect();
            ym.resize(152, None);
            (group_map(320, 2, &[2, 1]), ym)
        }
        4 => ((0..320).map(|x| x / 2).collect(), (0..152).map(|y| Some(y / 2)).collect()),
        5 => ((0..320).map(|x| x / 3).collect(), (0..152).map(|y| Some(y / 3)).collect()),
        6 => ((0..320).map(|x| x / 4).collect(), (0..152).map(|y| Some(y / 4)).collect()),
        _ => ((0..320).map(|x| x / 8).collect(), (0..152).map(|y| Some(y / 8)).collect()),
    };
    let o = c.origin;
    for (dy, sy_off) in ym.iter().enumerate() {
        let Some(sr) = sy_off else { continue };
        let srow = (o + sy + sr).min(H - 1) * W;
        let drow = (o + dy) * W;
        if drow + W > PIX {
            continue;
        }
        for (dx, &sc) in xm.iter().enumerate() {
            let s = srow + (sx + sc).min(W - 1);
            scr.fb[drow + dx] = c.back[s];
        }
    }
}

/// Maps `n` destination pixels to source offsets: groups of `g` source pixels where source
/// pixel k of a group is repeated `rep[k]` times.
fn group_map(n: usize, g: usize, rep: &[usize]) -> Vec<usize> {
    let mut v = Vec::with_capacity(n);
    let mut base = 0;
    'o: loop {
        for (k, &r) in rep.iter().enumerate().take(g) {
            for _ in 0..r {
                if v.len() >= n {
                    break 'o;
                }
                v.push(base + k);
            }
        }
        base += g;
    }
    v
}

// ==================================================================== credits + prologue

/// CREDITS.HNM (sub_10309), then the 8 narrated pages and XPLAIN9 (sub_1021C).
pub struct Prologue {
    core: Option<Core>,
    stage: u8,
    page: usize,
    until: u64,
}

impl Prologue {
    pub fn new(_assets: &Assets, _audio: &mut Audio) -> Prologue {
        Prologue { core: None, stage: 0, page: 0, until: 0 }
    }

    /// Returns true when finished.
    pub fn update(&mut self, dt: f32, input: &Input, assets: &mut Assets, audio: &mut Audio, screen: &mut Screen) -> bool {
        if self.core.is_none() {
            let mut c = Core::new(assets, screen);
            c.map = assets.dat.read("MAP.HSQ").unwrap_or_default();
            c.map.resize(dune_formats::save::MAP_SIZE.max(c.map.len()), 0);
            self.core = Some(c);
        }
        let c = self.core.as_mut().unwrap();
        c.note_input(input);
        let n = c.ticks_for(dt);
        for _ in 0..n {
            let c = self.core.as_mut().unwrap();
            c.now += 1;
            c.sfx.feed(audio);
            if self.tick(assets, audio, screen) {
                return true;
            }
        }
        false
    }

    fn finish(&mut self, audio: &mut Audio, screen: &mut Screen) -> bool {
        stop_all(audio);
        screen.fb.fill(0);
        true
    }

    /// One tick. Returns true when the prologue is over.
    fn tick(&mut self, a: &mut Assets, au: &mut Audio, scr: &mut Screen) -> bool {
        for _ in 0..32 {
            let c = self.core.as_mut().unwrap();
            match self.stage {
                // Credits: origin 24, clear, k=41 + 0x3A with the HNM open as draw routine.
                0 => {
                    au.stop();
                    c.origin = 24;
                    c.clear_back();
                    c.pal.save();
                    c.clear_back();
                    c.hnm_open(a, au, 0x14, 0);
                    self.stage = 1;
                    if !c.start_transition(0x3A, scr) {
                        return false;
                    }
                }
                1 => {
                    if c.trans.is_some() && !c.transition_tick(scr) {
                        return false;
                    }
                    // WORMSUIT (sub_1AD50).
                    c.music_stop(au);
                    c.music_track(a, au, 3);
                    self.stage = 2;
                }
                // Task 0xA16 until the end or input.
                2 => {
                    c.hnm_step(au, scr, true);
                    if c.hnm_done() {
                        self.stage = 3;
                        continue;
                    }
                    match c.poll(a, au, scr) {
                        Some(true) => return self.finish(au, scr),
                        Some(false) => {
                            c.pending = None;
                            self.stage = 3;
                        }
                        None => return false,
                    }
                }
                // sub_1021C: origin 0, WORMSUIT restarted.
                3 => {
                    c.hnm = None;
                    c.origin = 0;
                    c.music_stop(au);
                    c.music_track(a, au, 3);
                    self.page = 1;
                    self.stage = 4;
                }
                // Page: k=41 + 0x3A with the page drawing.
                4 => {
                    c.pal.save();
                    self.draw_page(a, au);
                    let c = self.core.as_mut().unwrap();
                    self.stage = 5;
                    if !c.start_transition(0x3A, scr) {
                        return false;
                    }
                }
                5 => {
                    if c.trans.is_some() && !c.transition_tick(scr) {
                        return false;
                    }
                    // sub_1ADE0 (music down to ds:289E = 0x8C), narrator PZ00xI (sub_1AB4F),
                    // then wait 0xFA0 ticks or input.
                    c.music_fade(0x8C, 0x64);
                    if let Some(v) = a.dat.read(&format!("PZ\\PZ{:03}I.VOC", self.page)).ok().and_then(|d| voc::Voc::parse(&d)) {
                        au.stop();
                        au.play_u8(&v.pcm, v.sample_rate);
                    }
                    c.face_on = true;
                    self.until = c.now + 0xFA0;
                    self.stage = 6;
                }
                6 => {
                    if c.now <= self.until {
                        match c.poll(a, au, scr) {
                            None => return false,
                            Some(true) => return self.finish(au, scr),
                            Some(false) => c.pending = None,
                        }
                    }
                    // sub_10911, stop the voice, sub_1ADED (music back to ds:2896 = 0xE6).
                    c.kill_tasks(au, scr);
                    au.stop();
                    c.music_fade(0xE6, 0x190);
                    self.page += 1;
                    self.stage = if self.page <= 8 { 4 } else { 7 };
                }
                // XPLAIN9 with effect 0x10.
                7 => {
                    c.clear_back();
                    c.still(a, 0x1B);
                    self.stage = 8;
                    if !c.start_transition(0x10, scr) {
                        return false;
                    }
                }
                8 => {
                    if c.trans.is_some() && !c.transition_tick(scr) {
                        return false;
                    }
                    self.until = c.now + 0xC8;
                    self.stage = 9;
                }
                9 | 11 => {
                    if c.now <= self.until {
                        match c.poll(a, au, scr) {
                            None => return false,
                            Some(_) => return self.finish(au, scr),
                        }
                    }
                    if self.stage == 9 {
                        // sub_138F1(bl): only if a sky view set ds:46DF and the slot differs.
                        if c.sky_df != 0 {
                            let bl = crate::intro::sky_index(a, c.w16(2)) as u8;
                            if c.sky_d6 != bl {
                                c.sky_target(a, bl);
                                let old = std::mem::replace(&mut c.sky_d7, 0x40);
                                if old == 0 {
                                    c.add_task(TaskId::SkyFade, 16);
                                }
                            }
                        }
                        c.sky_df = 1;
                        self.until = c.now + 0x4B0;
                        self.stage = 11;
                    } else {
                        c.sky_d7 = 0;
                        c.remove_task(TaskId::SkyFade);
                        c.sky_df = 0;
                        c.clear_back();
                        self.stage = 12;
                        if !c.start_transition(0x10, scr) {
                            return false;
                        }
                    }
                }
                _ => {
                    if c.trans.is_some() && !c.transition_tick(scr) {
                        return false;
                    }
                    return self.finish(au, scr);
                }
            }
        }
        false
    }

    /// Draw routine 02C1: clear, page picture (table cs:020A), COMMAND text 0x117 + page.
    fn draw_page(&mut self, a: &mut Assets, _au: &mut Audio) {
        let c = self.core.as_mut().unwrap();
        c.clear_back();
        match self.page {
            1 => stars(c, a, 0),
            2 | 6 => {
                stars(c, a, 0x20);
                // sub_1B8A7: GLOBDATA, set_view(ds:197C, ds:197E), bank FRESK.
                let tab = a.dat.read("TABLAT.BIN").unwrap_or_default();
                let gd = a.dat.read("GLOBDATA.HSQ").unwrap_or_default();
                c.globe = Globe::new(&tab, gd);
                let (lon, lat) = (c.w16(0x197C), c.w16(0x197E) as i16);
                if let Some(g) = &mut c.globe {
                    g.set_view(lon, lat);
                }
                c.select_bank(a, 1);
                // sub_1B85A: FRESK ring at (91, 20), full render (B977).
                c.put(a, 2, 0x5B, 0x14, None);
                if let Some(g) = &mut c.globe {
                    g.render(&mut c.back, &c.map);
                }
                // Bank STARS again (palette), task 0xB9AE period 1.
                c.select_bank(a, 0x2C);
                c.remove_task(TaskId::Globe);
                c.add_task(TaskId::Globe, 1);
            }
            3 => {
                // 094A: sub_138B4 (sky) + INTDS at (0, 60).
                c.sky_df = 1;
                let bl = crate::intro::sky_index(a, c.w16(2)) as u8;
                if c.sky_d7 == 0 {
                    c.pal_bank = 0x29;
                    if let Some(b) = c.select_bank(a, 0x29) {
                        c.sky_d6 = bl;
                        c.pal.set(73, &b.pal_entry(bl as usize, 0x1C5));
                    }
                }
                c.select_bank(a, 0x28);
                for band in 0..4u16 {
                    let mut x = 0;
                    while x < 0x140 {
                        c.put(a, band, x, band as i32 * 20, None);
                        x += 0x28;
                    }
                }
                c.select_bank(a, 0x2D);
                c.put(a, 0, 0, 0x3C, None);
            }
            4 | 7 => {
                c.select_bank(a, 0x30);
                c.list(a, 0x1526, None);
                c.save_scratch();
                c.face(a, 0x2D, 0, 1);
            }
            5 => {
                c.select_bank(a, 0x30);
                c.list(a, 0x153A, None);
                c.save_scratch();
                c.face(a, 9, 0x52, 0x1E);
            }
            _ => {
                c.clear_back();
                c.still(a, 0x61);
            }
        }
        // Text under the picture.
        c.pal.set(255, &[[63, 52, 30]]);
        if let Some(t) = a.commands() {
            let s = t.get(0x116 + self.page).to_vec();
            let lines = a.font.wrap(&s, W - 24);
            for (i, l) in lines.iter().enumerate().take(4) {
                let w = a.font.text_width(l) as i32;
                a.font.draw(&mut c.back, W, (W as i32 - w) / 2, 156 + i as i32 * 10, l, 255);
            }
        }
    }
}

/// sub_10A44(cx): star panels and the planet approaching (STARS.HSQ).
fn stars(c: &mut Core, a: &Assets, cx: i32) {
    c.select_bank(a, 0x2C);
    c.fill(VIEW, 0);
    let d = -((cx * cx) >> 1);
    c.put(a, 0, d, 0, Some(VIEW));
    c.put(a, 1, d + 0x130, 0, Some(VIEW));
    c.put(a, 2, d + 0x260, 0, Some(VIEW));
    c.put_centred(a, 0x24, 4 * d + 0x45, 0x4E);
    let x = if cx <= 0x14 { cx * 4 + 0xF2 } else { 2 * d + 0x3E2 };
    c.put_centred(a, ((cx >> 2) + 0x25) as u16, x, 0x67);
    c.put_centred(a, (cx + 3) as u16, ((-d) >> 4) + 0x80, 0x4F);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_maps() {
        let m = group_map(320, 7, &[2, 1, 1, 1, 1, 1, 1]);
        assert_eq!(m.len(), 320);
        assert_eq!(*m.last().unwrap(), 279);
        let m = group_map(320, 2, &[2, 1]);
        assert_eq!(*m.last().unwrap(), 212);
        assert_eq!(group_map(152, 7, &[1, 1, 1, 1, 1, 1, 2]).last(), Some(&132));
        assert_eq!(group_map(150, 2, &[1, 2]).last(), Some(&99));
    }

    #[test]
    fn rng_and_dir() {
        assert_eq!(dir(0x00), (0, -0x20));
        assert_eq!(dir(0xE0), (-0x20, -0x20));
        assert_eq!(dir(0x1F), (0x1F, -0x20));
        assert_eq!(subpixel(0x20, 0), (1, 0));
        assert_eq!(subpixel(0xE0, 0), (-1, 0));
        assert_eq!(subpixel(0x10, 0x10), (1, 0));
    }
}
