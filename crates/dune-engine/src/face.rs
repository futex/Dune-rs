//! Character close-ups, ported from the original routines (docs/re/faces.md):
//! sub_19908 (start), sub_1978E (first draw), task sub_199DA every 16 ticks (frames,
//! listening blocks), sub_19F1C (voice start), task sub_1A7C2 every tick (lip-sync),
//! loc_19DE3 (mouth), sub_19BB1 & co (diff-based dirty-rect redraw).
//!
//! The animator owns two 320x152 buffers like the game: `clean` (DBDE, the background:
//! the room, or the x4 zoom of the room behind the speaker) and `back` (DBD6, what is shown).

use dune_formats::face::FaceBank;

pub const VIEW_W: usize = 320;
pub const VIEW_H: usize = 152;
pub const VIEW_PIXELS: usize = VIEW_W * VIEW_H;
const FACE_PERIOD: u64 = 16;
/// Lip-sync step in 16.16 ticks: N * 0x800 with the FREQ.HSQ calibration N = 254.
const LIP_STEP: u32 = 254 * 0x800;

type Entry = (usize, u8, u8);

#[derive(Clone, Copy)]
struct Rect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// Global RNG (sub_1E3B7).
pub struct Rng(pub u16);

impl Rng {
    pub fn next(&mut self, mask: u16) -> u16 {
        let p = self.0 as u32 * 0xE56D;
        self.0 = (p as u16).wrapping_add(1);
        let ax = (((p >> 16) & 0xFF) as u16) << 8 | (self.0 >> 8);
        ax & mask
    }
}

struct Lips {
    bytes: Vec<u8>,
    pos: usize,
    /// DC2C:DC2E — 16.16 tick time of the last consumed byte.
    last: u64,
}

pub struct FaceAnim {
    bank: FaceBank,
    /// Speaking character (47C4), for the c == 7 / 9 / 12 special cases.
    character: usize,
    variant: u8,
    f0: usize,
    origin: (i32, i32),
    face_rect: Rect,
    /// Mouth rectangle (ds:27FA + 8 * bank), relative to the face origin.
    mouth_rect: Option<Rect>,
    story_phase: u8,
    next: usize,
    budget: i16,
    flags: u8,
    prev: Vec<Entry>,
    rng: Rng,
    lips: Option<Lips>,
    last_mouth: u8,
    speaking: bool,
    anchor: u64,
    pub clean: Vec<u8>,
    pub back: Vec<u8>,
}

pub struct FaceParams {
    pub character: usize,
    pub variant: u8,
    /// ds:F0 (0, or 0x0A for the captive Harkonnen captain).
    pub f0: usize,
    /// sub_109C7 dx (intro scenes); 0 for the TALK close-up.
    pub shift: u16,
    pub mouth_rect: Option<(u16, u16, u16, u16)>,
    /// ds:478C: words in the phrase (frame budget = 4 per word).
    pub words: i16,
    pub story_phase: u8,
    pub seed: u16,
}

impl FaceAnim {
    /// sub_19908 + sub_1978E: picks the first sequence and draws frame 0 fully.
    /// `now` is the current tick (the 16-tick task is anchored there).
    pub fn new(bank: FaceBank, p: FaceParams, background: &[u8], now: u64) -> FaceAnim {
        let (x0, y0, x1, y1) = bank.rect;
        let (mut x0, mut x1) = (x0 as i32, x1 as i32);
        if x0 < p.shift as i32 {
            x0 += p.shift as i32;
            x1 = (x1 + p.shift as i32).min(320);
        }
        let mut a = FaceAnim {
            character: p.character,
            variant: p.variant,
            f0: p.f0,
            origin: (x0, y0 as i32),
            face_rect: Rect { x0, y0: y0 as i32, x1, y1: y1 as i32 },
            mouth_rect: p.mouth_rect.map(|(a, b, c, d)| Rect { x0: a as i32, y0: b as i32, x1: c as i32, y1: d as i32 }),
            story_phase: p.story_phase,
            next: 0,
            budget: p.words.wrapping_mul(4),
            flags: 0xC0,
            prev: Vec::new(),
            rng: Rng(p.seed),
            lips: None,
            last_mouth: 0xFF,
            speaking: false,
            anchor: now,
            clean: background[..VIEW_PIXELS].to_vec(),
            back: background[..VIEW_PIXELS].to_vec(),
            bank,
        };
        let bp = a.pick_bp();
        let mut si = a.bank.seq_ptr(bp);
        si = a.skip_variant(si);
        a.next = si;
        // sub_1978E: full redraw of frame 0; the advanced pointer is NOT stored.
        a.prev.clear();
        a.redraw(si);
        a
    }

    /// sub_1994F: idle/talk sequence choice.
    fn pick_bp(&mut self) -> usize {
        if self.variant == 0 {
            self.rng.next(6) as usize + self.f0
        } else {
            (self.variant as usize - 1) * 2
        }
    }

    /// sub_1996C: variants skip the first 32 frames.
    fn skip_variant(&self, si: usize) -> usize {
        if self.variant != 0 {
            self.bank.skip_frames(si, 32)
        } else {
            si
        }
    }

    /// sub_19A7B: listening-block sequence and RNG draw.
    fn listen_block(&mut self) -> (usize, u16) {
        let (mut al, bx) = if self.variant != 0 { (self.variant, 0x0F18) } else { (5u8, 0x0F38) };
        if self.variant == 0 && self.character == 7 && self.story_phase >= 0xC8 {
            al = 6;
        }
        let si = self.bank.seq_ptr((al as usize - 1) * 2 + self.f0);
        (si, self.rng.next(bx))
    }

    /// sub_19A60: skip `n` frames, budget = 8.
    fn start_block(&mut self, si: usize, n: u8) -> usize {
        self.budget = 8;
        self.bank.skip_frames(si, n as usize)
    }

    /// sub_19F1C + loc_1A75C: a voice starts; `lips` is the VOC type-5 block.
    pub fn speak(&mut self, lips: Option<&[u8]>, now: u64) {
        self.flags |= 0x10;
        let (si, ax) = self.listen_block();
        self.next = self.start_block(si, ax as u8);
        self.speaking = true;
        self.last_mouth = 0xFF;
        self.lips = lips.map(|l| Lips { bytes: l.get(2..).unwrap_or(&[]).to_vec(), pos: 0, last: now << 16 | 0x8000 });
        // The first lip byte is read and drawn immediately.
        if let Some(b) = self.lips.as_mut().and_then(|l| {
            let b = l.bytes.get(l.pos).copied().unwrap_or(0xFF);
            l.pos += 1;
            Some(b)
        }) {
            if b == 0xFF {
                self.end_speech();
            } else {
                self.mouth(b);
            }
        }
    }

    fn end_speech(&mut self) {
        self.lips = None;
        self.speaking = false;
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Runs one timer tick. Returns true if `back` changed.
    pub fn tick(&mut self, now: u64) -> bool {
        let mut changed = self.lip_tick(now);
        if now > self.anchor && (now - self.anchor) % FACE_PERIOD == 0 {
            changed |= self.face_tick();
        }
        changed
    }

    /// One call of the 16-tick face task (sub_199BE) when driven by an external scheduler.
    /// Returns true if `back` changed.
    pub fn face_step(&mut self) -> bool {
        self.face_tick()
    }

    /// One call of the per-tick lip-sync task (sub_1A7C2). Returns true if `back` changed.
    pub fn lip_step(&mut self, now: u64) -> bool {
        self.lip_tick(now)
    }

    /// sub_19985: `while (47CE & 7) sub_199BE();` (finish the current 8-frame block).
    pub fn finish_block(&mut self) -> bool {
        let mut changed = false;
        let mut guard = 0;
        while self.budget & 7 != 0 && guard < 4096 {
            changed |= self.face_tick();
            guard += 1;
        }
        changed
    }

    /// Stops lip-sync (sub_1A7A5); the image stays.
    pub fn stop_speech(&mut self) {
        self.end_speech();
    }

    /// Task sub_1A7C2.
    fn lip_tick(&mut self, now: u64) -> bool {
        let Some(l) = &mut self.lips else { return false };
        let mut due = l.last + LIP_STEP as u64;
        let dx = now as i64 - (due >> 16) as i64;
        if dx < 0 {
            return false;
        }
        let mut rem: i64 = dx << 16;
        loop {
            let b = l.bytes.get(l.pos).copied().unwrap_or(0xFF);
            l.pos += 1;
            if b == 0xFF {
                self.end_speech();
                return false;
            }
            rem -= LIP_STEP as i64;
            if rem < 0 {
                l.last = due;
                return self.mouth(b);
            }
            due += LIP_STEP as u64;
        }
    }

    /// sub_1A82E / loc_19DE3: draw the mouth-table entry, clipped to the mouth rect,
    /// without restoring the background.
    fn mouth(&mut self, v: u8) -> bool {
        if v == self.last_mouth {
            return false;
        }
        self.last_mouth = v;
        let Some(mr) = self.mouth_rect else { return false };
        let (ox, oy) = self.origin;
        let clip = Rect { x0: mr.x0 + ox, y0: mr.y0 + oy, x1: mr.x1 + ox, y1: mr.y1 + oy };
        let mut idx = v as usize;
        if self.variant != 0 {
            let w = self.variant as usize - 1;
            if self.character == 13 {
                idx += w;
            }
            idx += 4 * w;
        }
        let entries = self.bank.mouth_entries(idx);
        self.draw_entries(&entries, clip);
        true
    }

    /// Task sub_199DA (every 16 ticks).
    fn face_tick(&mut self) -> bool {
        if self.flags & 0x80 == 0 {
            return false;
        }
        if self.flags & 0x10 == 0 {
            let mut si = self.next;
            let mut exhausted = false;
            loop {
                if self.bank.byte(si) != 0xFF {
                    break;
                }
                if self.budget < 0 {
                    exhausted = true;
                    break;
                }
                let bp = self.pick_bp();
                si = self.bank.seq_ptr(bp);
                si = self.skip_variant(si);
                self.next = si;
            }
            if !exhausted {
                return self.draw_step(si);
            }
            self.flags |= 0x10;
        }
        let si = self.next;
        if self.budget > 0 {
            return self.draw_step(si);
        }
        // (special sequences, sub_19AB4, are driven by dialogue opcodes not used here)
        let (si, ax) = self.listen_block();
        if ax >> 8 != 0 {
            return false;
        }
        let si = self.start_block(si, ax as u8);
        self.draw_step(si)
    }

    fn draw_step(&mut self, si: usize) -> bool {
        self.budget = self.budget.wrapping_sub(1);
        let (changed, next) = self.redraw(si);
        self.next = next;
        changed
    }

    fn entry_box(&self, e: &Entry) -> Option<Rect> {
        let s = self.bank.sheet.sprites.get(e.0)?;
        let x0 = self.origin.0 + e.1 as i32;
        let y0 = self.origin.1 + e.2 as i32;
        Some(Rect { x0, y0, x1: x0 + s.width as i32, y1: y0 + s.height as i32 })
    }

    /// sub_19BB1: returns (something was redrawn, pointer after the frame).
    fn redraw(&mut self, si: usize) -> (bool, usize) {
        let (entries, after) = self.bank.frame_at(si);
        let mut r;
        if self.prev.is_empty() {
            r = self.face_rect;
        } else {
            r = Rect { x0: 0x13F, y0: 0xC7, x1: 0, y1: 0 };
            let (a, b) = (self.prev.clone(), entries.clone());
            for (from, other) in [(&a, &b), (&b, &a)] {
                for e in from.iter().filter(|e| !other.contains(e)) {
                    if let Some(bx) = self.entry_box(e) {
                        r.x0 = r.x0.min(bx.x0);
                        r.y0 = r.y0.min(bx.y0);
                        r.x1 = r.x1.max(bx.x1);
                        r.y1 = r.y1.max(bx.y1);
                    }
                }
                // §4.6: while speaking, body redraws stay above the mouth (not Baron/captain).
                if self.speaking && self.character != 9 && self.character != 12 {
                    if let Some(m) = self.mouth_rect {
                        if m.y0 < r.y1 {
                            r.y1 = m.y0;
                            if m.y0 <= r.y0 {
                                r.x0 = 0x13F;
                            }
                        }
                    }
                }
            }
            if r.x0 == 0x13F {
                self.prev = entries;
                return (false, after);
            }
        }
        r.y1 = r.y1.min(0x98);
        // sub_1C446: restore the dirty rect from the clean buffer.
        for y in r.y0.max(0)..r.y1.min(VIEW_H as i32) {
            let (a, b) = (r.x0.max(0) as usize, r.x1.min(VIEW_W as i32).max(0) as usize);
            if a < b {
                let o = y as usize * VIEW_W;
                self.back[o + a..o + b].copy_from_slice(&self.clean[o + a..o + b]);
            }
        }
        self.prev = entries.clone();
        self.draw_entries(&entries, r);
        (true, after)
    }

    fn draw_entries(&mut self, entries: &[Entry], clip: Rect) {
        let (ox, oy) = self.origin;
        let c = (clip.x0.max(0), clip.y0.max(0), clip.x1.min(VIEW_W as i32), clip.y1.min(VIEW_H as i32));
        for &(s, x, y) in entries {
            if let Some(sp) = self.bank.sheet.sprites.get(s) {
                let o = dune_formats::sprite::DrawOpts { clip: Some(c), ..Default::default() };
                sp.draw(&mut self.back, VIEW_W, ox + x as i32, oy + y as i32, o);
            }
        }
    }

    pub fn palette(&self, pal: &mut dune_formats::palette::Palette) {
        self.bank.sheet.apply_palette(pal);
    }
}

/// sub_19123: face bank index (resource = index + 2) and variant for a character.
pub fn bank_and_variant(c: usize, mem: &[u8]) -> (usize, u8) {
    let rd16 = |a: usize| u16::from_le_bytes([mem[a], mem[a + 1]]);
    if c >= 0x11 {
        // Paul (PAUL.HSQ): age variant from the game time, +1 with ds:F4 >= 0x10.
        let t = rd16(2);
        let ah = ((t << 2) >> 8).min(8) as u8;
        let v = (ah << 1) + (mem[0xF4] >= 0x10) as u8;
        return (0x2D, v + 1);
    }
    if c < 0x0D {
        return (c, 0);
    }
    if c == 0x0D {
        let p = rd16(0x114E) as usize;
        return (c, (mem[p] >> 1) + 1);
    }
    let si = if c == 0x0E {
        rd16(0x4756) as usize
    } else {
        if mem[0x2A] == 0xC8 {
            return (c, 0);
        }
        rd16(0x4758 + 2 * mem[0x476C] as usize) as usize
    };
    let al = mem[si];
    let (mut q, r) = (al / 3, al % 3);
    let dl = if r == 0 { 0x0F } else { 0x11 };
    while q >= dl {
        q -= dl;
    }
    (0x0E + r as usize, q + 1)
}

/// sub_13AF9 / DNVGA 0x3A14 bp=6: the 80x38 block of the character-free room at (x, y)
/// (clamped to x <= 240, y <= 113), scaled x4 by pixel replication to 320x152.
pub fn zoom_backdrop(room: &[u8], x: i32, y: i32) -> Vec<u8> {
    let (x, y) = (x.clamp(0, 0xF0) as usize, y.clamp(0, 0x71) as usize);
    let mut out = vec![0u8; VIEW_PIXELS];
    for row in 0..38 {
        for col in 0..80 {
            let p = room[(y + row) * VIEW_W + x + col];
            for i in 0..4 {
                let o = (4 * row + i) * VIEW_W + 4 * col;
                out[o..o + 4].fill(p);
            }
        }
    }
    out
}
