//! HERAD: Cryo's AdLib music format, as played by the `DNADL` driver.
//!
//! Port of the driver's logic (see `docs/re/music.md`). The player only produces
//! OPL2 register writes; feed them to any OPL emulator through [`OplOut`].
//! Call [`Herad::interrupt`] at [`IRQ_HZ`] (the game's PIT rate).

pub const IRQ_HZ: f64 = 1_193_182.0 / 5957.0; // 200.299 Hz

const FNUM: [u16; 12] = [0x157, 0x16C, 0x181, 0x198, 0x1B1, 0x1CB, 0x1E6, 0x203, 0x222, 0x243, 0x266, 0x28A];
const BEND: [u8; 13] = [0x13, 0x15, 0x15, 0x17, 0x19, 0x1A, 0x1B, 0x1D, 0x1F, 0x21, 0x23, 0x24, 0x25];
const COARSE: [u16; 10] = [0, 5, 10, 15, 20, 0, 6, 12, 18, 24];
const OPS: [(u8, u8); 9] =
    [(0x00, 0x03), (0x01, 0x04), (0x02, 0x05), (0x08, 0x0B), (0x09, 0x0C), (0x0A, 0x0D), (0x10, 0x13), (0x11, 0x14), (0x12, 0x15)];
const CHANNELS: usize = 9;
const INSTR_SIZE: usize = 40;

pub trait OplOut {
    fn write(&mut self, reg: u8, val: u8);
}

#[derive(Clone, Copy, Default)]
struct Chan {
    delta: u16,
    ptr: usize, // 0 = none (offsets are > 0 in practice)
    start: usize,
    prog: u8,
    note: u8,
    bend_mode: u8,
    transpose: i8,
    slide_cnt: u8,
    slide_dur: u8,
    slide_val: u8,
    slide_step: i8,
    vel_sens: [i8; 2],
    base_lvl: [u8; 2],
    cur_lvl: [u8; 2],
    fb_vel_sens: i8,
    fb_vel_c0: u8,
    at_sens: [i8; 2],
    fb_at_sens: i8,
    fb_at_c0: u8,
    fnum_saved: u16,
}

pub struct Herad {
    data: Vec<u8>,
    ins: usize,
    loop_start: u16,
    loop_end: u16,
    loop_count: u16,
    speed: u16,
    ch: [Chan; CHANNELS],
    acc: u16,
    measure: u16,
    tick_in_measure: u8,
    loop_left: u16,
    save: [(u16, usize); CHANNELS],
    rep: u8,
    pub playing: bool,
    fading: bool,
    master: u8,
    target: u8,
    base: u8,
    fade_mask: u16,
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d.get(o).copied().unwrap_or(0), d.get(o + 1).copied().unwrap_or(0)])
}

fn vol_to_att(v: u8) -> u8 {
    let a = (!v as u16) >> 2;
    (a * a / 63) as u8
}

impl Herad {
    /// `repeat`: 0 = loop forever, 1 = play once (the driver's AL argument).
    pub fn new(data: Vec<u8>, repeat: u8, out: &mut dyn OplOut) -> Option<Herad> {
        if data.len() < 0x34 {
            return None;
        }
        let ins = u16_at(&data, 0) as usize;
        if ins > data.len() {
            return None;
        }
        let mut h = Herad {
            loop_start: u16_at(&data, 0x2C),
            loop_end: u16_at(&data, 0x2E),
            loop_count: u16_at(&data, 0x30),
            speed: u16_at(&data, 0x32),
            ins,
            ch: [Chan::default(); CHANNELS],
            acc: 0,
            measure: 1,
            tick_in_measure: 96,
            loop_left: 0,
            save: [(0, 0); CHANNELS],
            rep: repeat,
            playing: false,
            fading: false,
            master: 0,
            target: 0,
            base: 0,
            fade_mask: 0,
            data,
        };
        // Driver init.
        out.write(0x01, 0x20);
        out.write(0xBD, 0x00);
        out.write(0x08, 0x40);
        for (m, c) in OPS {
            out.write(0x80 + m, 0xFF);
            out.write(0x80 + c, 0xFF);
        }
        for i in 0..CHANNELS {
            let off = u16_at(&h.data, 2 + 2 * i) as usize;
            h.ch[i].start = if off == 0 { 0 } else { off + 2 };
            h.ch[i].prog = 0xFF;
        }
        h.restart();
        h.playing = true;
        h.music_tick(out);
        Some(h)
    }

    fn vlq(&self, p: &mut usize) -> u16 {
        let mut v: u32 = 0;
        loop {
            let b = self.data.get(*p).copied().unwrap_or(0);
            *p += 1;
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                break;
            }
        }
        v.min(0xFFFF) as u16
    }

    fn restart(&mut self) {
        self.measure = 1;
        self.tick_in_measure = 96;
        for i in 0..CHANNELS {
            let c = self.ch[i];
            let mut ptr = c.start;
            let mut delta = 0xFFFF;
            if c.start != 0 {
                delta = self.vlq(&mut ptr).wrapping_add(1);
            }
            self.ch[i].ptr = ptr;
            self.ch[i].delta = delta;
        }
    }

    fn loop_check(&mut self) {
        if self.loop_left == 0 {
            if self.loop_start == self.measure && self.tick_in_measure == 96 {
                for i in 0..CHANNELS {
                    self.save[i] = (self.ch[i].delta, self.ch[i].ptr);
                }
                self.loop_left = self.loop_count.wrapping_sub(1);
            }
        } else if self.loop_end == self.measure {
            self.loop_left -= 1;
            for i in 0..CHANNELS {
                (self.ch[i].delta, self.ch[i].ptr) = self.save[i];
            }
            self.measure = self.loop_start;
        }
    }

    pub fn stop(&mut self, out: &mut dyn OplOut) {
        for i in 0..CHANNELS {
            out.write(0xB0 + i as u8, 0);
        }
        self.playing = false;
    }

    /// One PIT interrupt.
    pub fn interrupt(&mut self, out: &mut dyn OplOut) {
        if !self.playing {
            return;
        }
        let hi = ((self.acc >> 8) as u8).wrapping_sub(1);
        self.acc = (hi as u16) << 8 | (self.acc & 0xFF);
        if hi & 0x80 != 0 {
            self.music_tick(out);
        }
        let carry = self.fade_mask & 0x8000 != 0;
        self.fade_mask = self.fade_mask.rotate_left(1);
        if carry {
            self.fade_step(out);
        }
    }

    fn music_tick(&mut self, out: &mut dyn OplOut) {
        self.acc = self.acc.wrapping_add(self.speed);
        self.loop_check();
        let mut i = 0;
        while i < CHANNELS {
            self.ch[i].delta = self.ch[i].delta.wrapping_sub(1);
            if self.ch[i].delta != 0 {
                let c = &mut self.ch[i];
                if c.slide_cnt != 0 && c.ptr != 0 {
                    c.slide_cnt -= 1;
                    c.slide_val = c.slide_val.wrapping_add(c.slide_step as u8);
                    let v = c.slide_val;
                    self.apply_pitch(i, v, out);
                }
                i += 1;
                continue;
            }
            while self.ch[i].ptr != 0 && self.playing {
                if self.event(i, out) {
                    // Song restarted by track 0: delta[0] already adjusted.
                }
                if self.ch[i].delta != 0 {
                    break;
                }
            }
            if !self.playing {
                return;
            }
            i += 1;
        }
        self.tick_in_measure -= 1;
        if self.tick_in_measure == 0 {
            self.tick_in_measure = 96;
            self.measure = self.measure.wrapping_add(1);
        }
    }

    fn read_delta(&mut self, ch: usize, mut p: usize) {
        self.ch[ch].delta = self.vlq(&mut p);
        self.ch[ch].ptr = p;
    }

    /// Runs one event on `ch`. Returns true if the song restarted.
    fn event(&mut self, ch: usize, out: &mut dyn OplOut) -> bool {
        let p = self.ch[ch].ptr;
        let st = self.data.get(p).copied().unwrap_or(0xFF);
        let p1 = self.data.get(p + 1).copied().unwrap_or(0);
        if st & 0x80 == 0 || st >= 0xF0 {
            return self.end_of_track(ch, out);
        }
        match (st & 0x70) >> 4 {
            0 => {
                // note off: note, vel(ignored)
                self.read_delta(ch, p + 3);
                let c = &mut self.ch[ch];
                if c.note == p1.wrapping_add(c.transpose as u8) {
                    c.note = 0;
                    let f = c.fnum_saved;
                    out.write(0xA0 + ch as u8, f as u8);
                    out.write(0xB0 + ch as u8, (f >> 8) as u8);
                }
            }
            1 => {
                let vel = self.data.get(p + 2).copied().unwrap_or(0);
                self.read_delta(ch, p + 3);
                self.note_on(ch, p1, vel, out);
            }
            2 | 3 => self.read_delta(ch, p + 2),
            4 => {
                self.read_delta(ch, p + 2);
                self.program(ch, p1, out);
            }
            5 => {
                self.read_delta(ch, p + 2);
                self.aftertouch(ch, p1, out);
            }
            6 => {
                self.read_delta(ch, p + 2);
                self.apply_pitch(ch, p1, out);
            }
            _ => return self.end_of_track(ch, out),
        }
        false
    }

    fn end_of_track(&mut self, ch: usize, out: &mut dyn OplOut) -> bool {
        self.ch[ch].delta = 0xFFFF;
        if ch != 0 {
            self.ch[ch].ptr = 0;
            return false;
        }
        self.rep = self.rep.wrapping_sub(1);
        if self.rep == 0 {
            for c in &mut self.ch {
                c.delta = 0xFFFF;
            }
            self.stop(out);
            return false;
        }
        if self.rep == 0xFF {
            self.rep = 0;
        }
        self.restart();
        self.loop_check();
        self.ch[0].delta = self.ch[0].delta.wrapping_sub(1);
        true
    }

    fn instr(&self, prog: u8) -> &[u8] {
        let o = self.ins + INSTR_SIZE * prog as usize;
        self.data.get(o..o + INSTR_SIZE).unwrap_or(&[0; INSTR_SIZE])
    }

    fn program(&mut self, ch: usize, prog: u8, out: &mut dyn OplOut) {
        if self.ch[ch].prog == prog {
            return;
        }
        let i: [u8; INSTR_SIZE] = self.instr(prog).try_into().unwrap();
        let master = self.master;
        let c = &mut self.ch[ch];
        c.prog = prog;
        c.bend_mode = i[0x21];
        c.transpose = i[0x22] as i8;
        let mtl = (master as u16 + i[0x0A] as u16).min(63) as u8;
        let ctl = (master as u16 + i[0x17] as u16).min(63) as u8;
        c.base_lvl = [(i[0x02] & 3) << 6 | mtl, (i[0x0F] & 3) << 6 | ctl];
        c.vel_sens = [i[0x1E] as i8, i[0x1F] as i8];
        c.at_sens = [i[0x26] as i8, i[0x27] as i8];
        let c0 = (i[0x04] << 1) | (!i[0x0E] & 1);
        c.fb_vel_sens = i[0x20] as i8;
        c.fb_vel_c0 = c0;
        c.fb_at_sens = i[0x1B] as i8;
        c.fb_at_c0 = c0;
        c.slide_step = i[0x24] as i8;
        c.slide_dur = i[0x23];
        c.slide_cnt = 0;
        let (m, k) = OPS[ch];
        out.write(0xC0 + ch as u8, c0 & 0x0F);
        for (op, b, tl) in [(m, 0x02usize, mtl), (k, 0x0F, ctl)] {
            let wave = if b == 0x02 { i[0x1C] } else { i[0x1D] };
            out.write(0xE0 + op, wave & 3);
            out.write(0x40 + op, (i[b] & 3) << 6 | tl);
            // Field layout relative to KSL byte: +1 MULT, +3 AR, +4 SL, +5 EG, +6 DR, +7 RR, +9 AM, +10 VIB, +11 KSR
            out.write(0x60 + op, i[b + 3] << 4 | (i[b + 6] & 0xF));
            out.write(0x80 + op, i[b + 4] << 4 | (i[b + 7] & 0xF));
            out.write(
                0x20 + op,
                (i[b + 9] & 1) << 7 | (i[b + 10] & 1) << 6 | (i[b + 5] & 1) << 5 | (i[b + 11] & 1) << 4 | (i[b + 1] & 0xF),
            );
        }
    }

    fn note_on(&mut self, ch: usize, note: u8, vel: u8, out: &mut dyn OplOut) {
        let (m, k) = OPS[ch];
        // Velocity scaling of operator levels.
        for (idx, op) in [(0usize, m), (1, k)] {
            let c = &mut self.ch[ch];
            let s = c.vel_sens[idx];
            if s == 0 {
                c.cur_lvl[idx] = c.base_lvl[idx];
                continue;
            }
            let x = if s > 0 { 0x80u16.wrapping_sub(vel as u16) } else { vel as u16 };
            let sh = 4i32 - s.unsigned_abs() as i32;
            let x = if sh >= 0 { x >> sh } else { x << (-sh) };
            let t = ((c.base_lvl[idx] & 0x3F) as u16 + x).min(63) as u8;
            c.cur_lvl[idx] = (c.base_lvl[idx] & 0xC0) | t;
            out.write(0x40 + op, c.cur_lvl[idx]);
        }
        {
            let c = &mut self.ch[ch];
            let s = c.fb_vel_sens;
            if s == 0 {
                c.fb_at_c0 = c.fb_vel_c0;
            } else {
                let x = if s > 0 { 0x80u16.wrapping_sub(vel as u16) } else { vel as u16 };
                let sh = (6 - s.unsigned_abs() as i32).max(0);
                let mut v = ((x >> sh) & 0xFE) + c.fb_vel_c0 as u16;
                if v > 0x0F {
                    v = (v & 0x0F) | 0x0E;
                }
                c.fb_at_c0 = v as u8;
                out.write(0xC0 + ch as u8, v as u8);
            }
        }
        let c = &mut self.ch[ch];
        if c.note != 0 {
            out.write(0xA0 + ch as u8, 0);
            out.write(0xB0 + ch as u8, 0);
        }
        let n = note.wrapping_add(c.transpose as u8);
        c.note = n;
        c.slide_cnt = c.slide_dur;
        c.slide_val = 0x40;
        let mut i = n as i32 - 24;
        if !(0..96).contains(&i) {
            i = 0;
        }
        let f = FNUM[(i % 12) as usize] | ((i / 12) as u16) << 10;
        c.fnum_saved = f;
        out.write(0xA0 + ch as u8, f as u8);
        out.write(0xB0 + ch as u8, (f >> 8) as u8 | 0x20);
    }

    fn aftertouch(&mut self, ch: usize, p: u8, out: &mut dyn OplOut) {
        let (m, k) = OPS[ch];
        let c = self.ch[ch];
        for (idx, op) in [(0usize, m), (1, k)] {
            let s = c.at_sens[idx];
            if s == 0 {
                continue;
            }
            let x = if s > 0 { p as i32 } else { 0x80 - p as i32 };
            let sh = 4 - s.unsigned_abs() as i32;
            let x = if sh >= 0 { x >> sh } else { x << (-sh) };
            let t = ((c.cur_lvl[idx] & 0x3F) as i32 - x).max(0) as u8;
            out.write(0x40 + op, (c.cur_lvl[idx] & 0xC0) | t);
        }
        let s = c.fb_at_sens;
        if s != 0 {
            let x = if s > 0 { p as i32 } else { 0x80 - p as i32 };
            let sh = (6 - s.unsigned_abs() as i32).max(0);
            let mut v = ((x >> sh) & 0xFE) + c.fb_at_c0 as i32;
            if v > 0x0F {
                v = (v & 0x0F) | 0x0E;
            }
            out.write(0xC0 + ch as u8, v as u8);
        }
    }

    fn apply_pitch(&mut self, ch: usize, b: u8, out: &mut dyn OplOut) {
        let c = &mut self.ch[ch];
        let n = c.note;
        if n == 0 {
            return;
        }
        let q = n.wrapping_sub(24) as i32;
        let mut oct = q / 12;
        let mut semi = q % 12;
        let f: i32;
        if c.bend_mode == 0 {
            if b < 0x40 {
                let d = 0x40 - b as i32;
                semi -= d >> 5;
                let frac = (d & 31) << 3;
                if semi < 0 {
                    semi += 12;
                    oct -= 1;
                    if oct < 0 {
                        oct = 0;
                        semi = 0;
                    }
                }
                f = FNUM[semi as usize] as i32 - ((BEND[semi as usize] as i32 * frac) >> 8);
            } else {
                let u = b as i32 - 0x3F;
                semi += u >> 5;
                let frac = (u & 31) << 3;
                if semi >= 12 {
                    semi -= 12;
                    oct += 1;
                }
                let s = (semi as usize).min(11);
                f = FNUM[s] as i32 + ((BEND[s + 1] as i32 * frac) >> 8);
            }
        } else if b < 0x40 {
            let d = 0x40 - b as i32;
            semi -= d / 5;
            let r = (d % 5) as usize;
            if semi < 0 {
                semi += 12;
                oct -= 1;
                if oct < 0 {
                    oct = 0;
                    semi = 0;
                }
            }
            let s = semi.clamp(0, 11) as usize;
            f = FNUM[s] as i32 - COARSE[if s >= 6 { 5 } else { 0 } + r] as i32;
        } else {
            let u = b as i32 - 0x40;
            semi += u / 5;
            let r = (u % 5) as usize;
            if semi >= 12 {
                semi -= 12;
                oct += 1;
            }
            let s = semi.clamp(0, 11) as usize;
            f = FNUM[s] as i32 + COARSE[if s >= 6 { 5 } else { 0 } + r] as i32;
        }
        let f = f as u16;
        let hi = ((f >> 8) as u8) | ((oct as u8) << 2);
        c.fnum_saved = (hi as u16) << 8 | (f & 0xFF);
        out.write(0xA0 + ch as u8, f as u8);
        out.write(0xB0 + ch as u8, hi | 0x20);
    }

    /// Music position as the game polls it (sub_1DE0C): measure * 16 + sixteenth,
    /// sixteenth = ((96 - tick countdown) / 6) & 15.
    pub fn position(&self) -> u32 {
        self.measure as u32 * 16 + (((96 - self.tick_in_measure as u32) / 6) & 15)
    }

    /// Master volume 0..=255 (driver entry +12).
    pub fn set_volume(&mut self, v: u8) {
        self.base = vol_to_att(v);
        self.target = self.base;
        self.fade_mask = 0xFFFF;
    }

    /// Fades to volume `v` (0 = silence, which stops the song). `dur` is the driver's AX.
    pub fn fade(&mut self, v: u8, dur: u16) {
        self.target = vol_to_att(v);
        self.fade_mask = match dur {
            0..=0x5F => 0xFFFF,
            0x60..=0xBF => 0xAAAA,
            0xC0..=0x17F => 0x8888,
            0x180..=0x2FF => 0x8080,
            _ => 0x8000,
        };
        self.fading = self.playing;
    }

    fn fade_step(&mut self, out: &mut dyn OplOut) {
        if self.master == self.target {
            self.fade_mask = 1;
            self.fading = false;
            return;
        }
        if self.master < self.target {
            for ch in 0..CHANNELS {
                let (m, k) = OPS[ch];
                for (idx, op) in [(0usize, m), (1, k)] {
                    let c = &mut self.ch[ch];
                    if c.base_lvl[idx] & 0x3F != 63 {
                        c.base_lvl[idx] += 1;
                    }
                    if c.cur_lvl[idx] & 0x3F != 63 {
                        c.cur_lvl[idx] += 1;
                        out.write(0x40 + op, c.cur_lvl[idx]);
                    }
                }
            }
            self.master += 1;
            if self.master >= 63 {
                self.stop(out);
            }
        } else {
            for ch in 0..CHANNELS {
                if self.ch[ch].prog == 0xFF {
                    continue;
                }
                let i = self.instr(self.ch[ch].prog);
                let tls = [i[0x0A], i[0x17]];
                let (m, k) = OPS[ch];
                for (idx, op) in [(0usize, m), (1, k)] {
                    let c = &mut self.ch[ch];
                    if c.base_lvl[idx] & 0x3F > tls[idx] {
                        c.base_lvl[idx] -= 1;
                    }
                    if c.cur_lvl[idx] & 0x3F != 0 {
                        c.cur_lvl[idx] -= 1;
                        out.write(0x40 + op, c.cur_lvl[idx]);
                    }
                }
            }
            self.master = self.master.saturating_sub(1);
        }
    }
}
