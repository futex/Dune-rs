//! DNVGA transition effects (fn 0x151, jump table cs:25A9), see docs/re/animations.md §1.
//!
//! A transition goes from what is on screen (`fb` + DAC) to the back buffer, through the
//! driver's palette model (§0.2): `shadow` (what the game believes), `target` (save slot,
//! fade target) and per-colour dirty flags. The caller passes the state as it is when the
//! wrapper `sub_1C108` calls fn 0x151 (k=41 save already done, setup palette written to
//! the shadow and marked dirty) and gets back the state after the wrapper (full back->screen
//! copy + dirty flush).
//!
//! Stepped on the 200.3 Hz tick clock: effects paced by the vertical retrace do one step per
//! 3 ticks (§0.4). The first step happens when the transition is created.

use dune_formats::palette::Palette;

const W: usize = 320;
const H: usize = 200;
const RETRACE: u32 = 3;

/// 4x4 ordered-dissolve offsets (cs:2FD7).
const T4: [(usize, usize); 16] =
    [(1, 1), (0, 3), (3, 2), (2, 0), (0, 1), (2, 3), (0, 0), (1, 2), (3, 0), (1, 3), (2, 1), (3, 3), (2, 2), (1, 0), (3, 1), (0, 2)];
const T2: [(usize, usize); 4] = [(0, 0), (1, 1), (1, 0), (0, 1)];
const T8: [(usize, usize); 16] =
    [(3, 3), (0, 5), (5, 4), (4, 2), (0, 3), (4, 5), (0, 2), (3, 4), (5, 2), (3, 5), (4, 3), (5, 5), (4, 4), (3, 2), (5, 3), (2, 4)];

/// The driver palette buffers (§0.2).
#[derive(Clone)]
pub struct PalState {
    pub shadow: Palette,
    pub target: Palette,
    pub dirty: [bool; 256],
}

impl PalState {
    pub fn new() -> PalState {
        PalState { shadow: Palette::black(), target: Palette::black(), dirty: [false; 256] }
    }

    /// k=32: send the dirty colours of the shadow to the DAC.
    pub fn apply(&mut self, dac: &mut Palette) {
        for c in 0..256 {
            if self.dirty[c] {
                dac.0[c] = self.shadow.0[c];
                self.dirty[c] = false;
            }
        }
    }

    /// k=2: write colours into the shadow (dirty if different).
    pub fn set(&mut self, first: usize, cols: &[[u8; 3]]) {
        for (i, c) in cols.iter().enumerate() {
            let k = first + i;
            if k < 256 && self.shadow.0[k] != *c {
                self.shadow.0[k] = *c;
                self.dirty[k] = true;
            }
        }
    }

    /// k=41: target := shadow.
    pub fn save(&mut self) {
        self.target = self.shadow.clone();
    }

    /// k=39 on colours [first, first+count): shadow += trunc((target - shadow) / n), DAC = shadow.
    pub fn step(&mut self, dac: &mut Palette, first: usize, count: usize, n: i32) {
        let t = self.target.clone();
        pal_step(&mut self.shadow, &t, first, count, n);
        for c in first..(first + count).min(256) {
            dac.0[c] = self.shadow.0[c];
        }
    }
}

impl Default for PalState {
    fn default() -> Self {
        PalState::new()
    }
}

pub struct Transition {
    effect: u8,
    /// Screen as displayed (updated in place).
    pub fb: Vec<u8>,
    /// DAC palette (what is displayed).
    pub dac: Palette,
    old_fb: Vec<u8>,
    new_fb: Vec<u8>,
    /// Driver palette state; final after `done`.
    pub pal: PalState,
    base: usize,
    step: u32,
    wait: u32,
    lfsr: u32,
    /// Spiral cursor (effect 0x02).
    spiral: Vec<(i64, i64, u32)>,
    spiral_di: i64,
    pub done: bool,
}

impl Transition {
    #[allow(dead_code)]
    /// Compatibility constructor: the new palette is written to the shadow (all colours
    /// dirty) and the DAC is the saved "old" palette.
    pub fn new(effect: u8, fb: &[u8], dac: &Palette, new_fb: &[u8], new_pal: &Palette, origin: usize) -> Transition {
        let pal = PalState { shadow: new_pal.clone(), target: dac.clone(), dirty: [true; 256] };
        Transition::start(effect, fb, dac, new_fb, pal, origin)
    }

    /// `origin` = drawing origin row (cs:01A3 / 320: 24 in the intro letterbox).
    pub fn start(effect: u8, fb: &[u8], dac: &Palette, new_fb: &[u8], pal: PalState, origin: usize) -> Transition {
        let effect = (effect & 0xFE) % 0x3E;
        let mut t = Transition {
            effect,
            fb: fb.to_vec(),
            dac: dac.clone(),
            old_fb: fb.to_vec(),
            new_fb: new_fb.to_vec(),
            pal,
            base: origin * W,
            step: 0,
            wait: 0,
            lfsr: 1,
            spiral: Vec::new(),
            spiral_di: 0,
            done: false,
        };
        match effect {
            0x36 => {
                // target := shadow; flags cleared; shadow/DAC 0..254 black; screen := back.
                t.pal.save();
                t.pal.dirty = [false; 256];
                for c in 0..255 {
                    t.pal.shadow.0[c] = [0; 3];
                    t.dac.0[c] = [0; 3];
                }
                t.fb.copy_from_slice(&t.new_fb);
            }
            0x38 => t.pal.save(),
            0x3A => {
                // Swap shadow <-> target for colours 1..254 only: shadow = old, target = new.
                for c in 1..255 {
                    std::mem::swap(&mut t.pal.shadow.0[c], &mut t.pal.target.0[c]);
                }
            }
            0x02 => t.build_spiral(),
            _ => {}
        }
        t.tick_inner();
        t
    }

    fn rows(&self) -> usize {
        if self.effect == 0x34 {
            200
        } else {
            152
        }
    }

    /// Wrapper (sub_1C108 tail): full back buffer -> screen, then flush dirty colours.
    fn finish(&mut self) {
        self.fb.copy_from_slice(&self.new_fb);
        self.pal.apply(&mut self.dac);
        self.done = true;
    }

    /// Advances one tick.
    pub fn tick(&mut self) {
        if self.done {
            return;
        }
        if self.wait > 1 {
            self.wait -= 1;
            return;
        }
        self.wait = 0;
        self.tick_inner();
    }

    fn tick_inner(&mut self) {
        match self.effect {
            0x30 => {
                // §1.4: apply, copy 152 lines at the origin; the wrapper copies the rest.
                self.pal.apply(&mut self.dac);
                self.finish()
            }
            0x10 | 0x34 => self.dissolve4(),
            0x36 => self.fade_in_black(),
            0x38 => self.fade_out_black(),
            0x3A => self.cross_fade(),
            0x08 | 0x3C => self.lfsr_dissolve(),
            0x06 => self.mosaic(),
            0x02 => self.spiral_step(),
            _ => self.finish(),
        }
    }

    /// §1.3: 16 steps of black holes, palette applied, 16 steps revealing the new image.
    fn dissolve4(&mut self) {
        let k = self.step as usize;
        if k >= 32 {
            return self.finish();
        }
        if k == 16 {
            self.pal.apply(&mut self.dac);
        }
        let (ox, oy) = T4[k % 16];
        let r_n = self.rows() / 4;
        for r in 0..r_n {
            for c in 0..80 {
                let p = self.base + (4 * r + oy) * W + 4 * c + ox;
                if p < W * H {
                    self.fb[p] = if k < 16 { 0 } else { self.new_fb[p] };
                }
            }
        }
        self.step += 1;
        self.wait = RETRACE;
    }

    /// §1.5: 64 passes x 8 chunks of 32 colours; a component rises from pass dl = T.
    fn fade_in_black(&mut self) {
        let s = self.step;
        if s >= 64 * 8 {
            return self.finish();
        }
        let dl = 64 - (s / 8) as i32;
        let chunk = (s % 8) as usize;
        for c in chunk * 32..chunk * 32 + 32 {
            for k in 0..3 {
                let t = self.pal.target.0[c][k] as i32;
                let v = self.pal.shadow.0[c][k] as i32;
                let d = (t - v) & 0xFF;
                if d != 0 && d >= dl {
                    self.pal.shadow.0[c][k] = (v + 1) as u8;
                }
            }
            self.dac.0[c] = self.pal.shadow.0[c];
        }
        self.step += 1;
        self.wait = RETRACE;
    }

    /// Fade-out kernel 0x26E3(ax colours per chunk, dh decrement): one chunk per step.
    /// Chunks start at 0, ax, 2ax ... while < 255.
    fn fade_out_chunk(&mut self, per: usize, dh: u8, chunk: usize) {
        let c0 = chunk * per;
        for c in c0..(c0 + per).min(256) {
            for k in 0..3 {
                self.pal.shadow.0[c][k] = self.pal.shadow.0[c][k].saturating_sub(dh);
            }
            self.dac.0[c] = self.pal.shadow.0[c];
        }
    }

    /// §1.6 0x38: 32 passes x 4 chunks of 64 colours, -2 per pass; screen := back,
    /// shadow := target (the DAC stays black), then the wrapper flush.
    fn fade_out_black(&mut self) {
        if self.step >= 32 * 4 {
            self.pal.shadow = self.pal.target.clone();
            return self.finish();
        }
        self.fade_out_chunk(64, 2, (self.step % 4) as usize);
        self.step += 1;
        self.wait = RETRACE;
    }

    /// §1.6 0x3A: fade out (22 passes x 3 chunks of 85 colours, -3, colour 255 untouched),
    /// screen := back, fade in toward the target (dh = 3, dl = 22..1).
    fn cross_fade(&mut self) {
        let out_steps = 22 * 3;
        if self.step < out_steps {
            self.fade_out_chunk(85, 3, (self.step % 3) as usize);
        } else if self.step < 2 * out_steps {
            if self.step == out_steps {
                self.fb.copy_from_slice(&self.new_fb);
            }
            let s = self.step - out_steps;
            let dl = 22 - (s / 3) as i32;
            let chunk = (s % 3) as usize;
            for c in chunk * 85..chunk * 85 + 85 {
                for k in 0..3 {
                    let t = self.pal.target.0[c][k] as i32;
                    let v = self.pal.shadow.0[c][k] as i32;
                    let d = (t - v) & 0xFF;
                    let (q, r) = (d / 3, d % 3);
                    let (need, step) = if r != 0 { (q + 1, r) } else { (q, 3) };
                    if d != 0 && need >= dl {
                        self.pal.shadow.0[c][k] = (v + step) as u8;
                    }
                }
                self.dac.0[c] = self.pal.shadow.0[c];
            }
        } else {
            return self.finish();
        }
        self.step += 1;
        self.wait = RETRACE;
    }

    /// §1.7: LFSR pixel dissolve, 150 (0x08) or 80 (0x3C) pixel pairs per tick.
    fn lfsr_dissolve(&mut self) {
        let n = 152 * W;
        let per = if self.effect == 0x08 { 150 } else { 80 };
        for _ in 0..per {
            let s = self.lfsr as usize;
            let a = self.base + s;
            if a < W * H {
                self.fb[a] = self.new_fb[a];
            }
            if s + 0x7FFF < n && a + 0x7FFF < W * H {
                self.fb[a + 0x7FFF] = self.new_fb[a + 0x7FFF];
            }
            let carry = self.lfsr & 1;
            self.lfsr >>= 1;
            if carry != 0 {
                self.lfsr ^= 0x4400;
            }
            if self.lfsr == 1 {
                self.fb[0] = self.new_fb[0];
                return self.finish();
            }
        }
        self.wait = 1;
    }

    /// §1.8 0x06: old image mosaic 2/4/8 (36 ticks each), palette, new image 8/4/2.
    fn mosaic(&mut self) {
        let phase = self.step / 36;
        if phase >= 6 {
            return self.finish();
        }
        let tick_in = self.step % 36;
        if tick_in % RETRACE == 0 {
            let entry = (tick_in / RETRACE) as usize;
            let (src, size) = match phase {
                0 => (false, 2),
                1 => (false, 4),
                2 => (false, 8),
                3 => (true, 8),
                4 => (true, 4),
                _ => (true, 2),
            };
            if phase == 3 && tick_in == 0 {
                self.pal.apply(&mut self.dac);
            }
            let (ox, oy) = match size {
                2 => T2[entry % 4],
                4 => T4[entry % 16],
                _ => T8[entry % 16],
            };
            let img = if src { &self.new_fb } else { &self.old_fb };
            let rows = 152;
            for by in (0..rows).step_by(size) {
                for bx in (0..W).step_by(size) {
                    let sp = self.base + (by + oy.min(size - 1)) * W + bx + ox.min(size - 1);
                    let v = img.get(sp).copied().unwrap_or(0);
                    for y in by..(by + size).min(rows) {
                        let o = self.base + y * W + bx;
                        let end = (o + size).min(self.base + y * W + W);
                        if end <= W * H {
                            self.fb[o..end].fill(v);
                        }
                    }
                }
            }
        }
        self.step += 1;
        self.wait = 1;
    }

    /// §1.9: the list of runs (n blocks, advance) of the clockwise square spiral.
    fn build_spiral(&mut self) {
        const RIGHT: i64 = -1272;
        const DOWN: i64 = 0;
        const LEFT: i64 = -1288;
        const UP: i64 = -2560;
        // (pre-adjust of di, advance, n)
        let mut runs = vec![(0, RIGHT, 1), (0, DOWN, 2)];
        let mut n = 2;
        while n <= 36 {
            runs.push((-1288, LEFT, n));
            runs.push((-1272, UP, n));
            runs.push((1288, RIGHT, n + 1));
            runs.push((1272, DOWN, n + 1));
            n += 2;
        }
        self.spiral = runs.into_iter().map(|(a, b, n)| (a, b, n as u32)).collect();
        self.spiral.reverse();
        self.spiral_di = (self.base + 74 * W + 156) as i64;
    }

    /// One run of 8x4 blocks per tick.
    fn spiral_step(&mut self) {
        let Some((pre, adv, n)) = self.spiral.pop() else { return self.finish() };
        self.spiral_di += pre;
        for _ in 0..n {
            let di = self.spiral_di;
            for row in 0..4i64 {
                for col in 0..8i64 {
                    let p = di + row * W as i64 + col;
                    if (0..(W * H) as i64).contains(&p) {
                        self.fb[p as usize] = self.new_fb[p as usize];
                    }
                }
            }
            self.spiral_di = di + 1280 + adv;
        }
        self.wait = 1;
    }
}

/// k=39 interpolation step (§0.2): shadow[i] += trunc((target[i] - shadow[i]) / n) on the
/// colour range [first, first+count).
pub fn pal_step(shadow: &mut Palette, target: &Palette, first: usize, count: usize, n: i32) {
    let n = n.max(1);
    for c in first..(first + count).min(256) {
        for k in 0..3 {
            let d = target.0[c][k] as i32 - shadow.0[c][k] as i32;
            shadow.0[c][k] = (shadow.0[c][k] as i32 + d / n) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Durations from animations.md §1 (ticks) and the final state.
    #[test]
    fn durations_and_final_state() {
        let old = vec![1u8; W * H];
        let new: Vec<u8> = (0..W * H).map(|i| (i % 251) as u8 + 2).collect();
        let mut dac = Palette::black();
        dac.0[7] = [10, 20, 30];
        let mut pal = PalState::new();
        pal.shadow = dac.clone();
        pal.save();
        pal.set(5, &[[40, 41, 42]]);
        for (fx, ticks) in [(0x30u8, 0u32), (0x10, 96), (0x34, 96), (0x36, 1536), (0x38, 384), (0x3A, 396), (0x02, 74), (0x08, 219), (0x3C, 410)] {
            let mut t = Transition::start(fx, &old, &dac, &new, pal.clone(), 24);
            let mut n = 0;
            while !t.done {
                t.tick();
                n += 1;
                assert!(n < 5000);
            }
            assert!((n as i32 - ticks as i32).abs() <= 3, "effect {fx:#x}: {n} ticks");
            assert!(t.fb == new, "effect {fx:#x}");
            if fx != 0x38 {
                assert_eq!(t.dac.0[5], [40, 41, 42], "effect {fx:#x}");
            } else {
                assert_eq!(t.dac.0[7], [0, 0, 0]);
                assert_eq!(t.pal.shadow.0[7], [10, 20, 30]);
            }
        }
    }
}
