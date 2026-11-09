//! The UI panel is the hit-zone table ds:1AE4 (docs/re/ui.md §1.1): 24 zones
//! `{x0,y0,x1,y1, state, flags, sprite, handler}`, drawn from ICONES.HSQ and hit-tested in
//! zone order. Presets (left panel ds:1C36.., compass ds:1C76/1CCA/1D1E/1D72/1DC6) are
//! copied from the EXE image.

use dune_formats::sprite::{DrawOpts, SpriteSheet};

#[derive(Clone, Copy, Default, Debug)]
pub struct Zone {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub state: u8,
    pub flags: u8,
    pub sprite: u16,
    pub handler: u16,
}

pub const ZONES: usize = 0x1AE6;
pub const LEFT_DEFAULT: usize = 0x1C36;
pub const LEFT_GLOBE: usize = 0x1C46;
pub const LEFT_MAP: usize = 0x1C66;
pub const COMPASS_ROOM: usize = 0x1C76;
pub const COMPASS_MAP: usize = 0x1CCA;
pub const COMPASS_HIDDEN: usize = 0x1D1E;
pub const COMPASS_FLIGHT: usize = 0x1D72;
pub const COMPASS_GLOBE: usize = 0x1DC6;

fn zone_at(mem: &[u8], a: usize) -> Zone {
    let w = |o: usize| u16::from_le_bytes([mem[a + o], mem[a + o + 1]]);
    Zone {
        x0: w(0) as i32,
        y0: w(2) as i32,
        x1: w(4) as i32,
        y1: w(6) as i32,
        state: mem[a + 8],
        flags: mem[a + 9],
        sprite: w(10),
        handler: w(12),
    }
}

#[derive(Clone)]
pub struct Panel {
    pub z: [Zone; 24],
}

impl Panel {
    pub fn new(mem: &[u8]) -> Panel {
        let mut z = [Zone::default(); 24];
        for (i, zz) in z.iter_mut().enumerate() {
            *zz = zone_at(mem, ZONES + 14 * i);
        }
        Panel { z }
    }

    /// loc_1D795: copy a left-panel preset ({state, flags, sprite} x 4) into zones 0..3.
    pub fn set_left(&mut self, mem: &[u8], preset: usize) {
        for i in 0..4 {
            let a = preset + 4 * i;
            self.z[i].state = mem[a];
            self.z[i].flags = mem[a + 1];
            self.z[i].sprite = u16::from_le_bytes([mem[a + 2], mem[a + 3]]);
        }
    }

    /// sub_1D72B: copy a 6-zone compass preset into zones 12..17.
    pub fn set_compass(&mut self, mem: &[u8], preset: usize) {
        for i in 0..6 {
            self.z[12 + i] = zone_at(mem, preset + 14 * i);
        }
    }

    /// sub_1D200: draw one zone (fill first if state & 0x40, hidden if state & 0x20).
    pub fn draw_zone(&self, i: usize, fb: &mut [u8], ic: &SpriteSheet) {
        let z = &self.z[i];
        if z.state & 0x40 != 0 {
            for y in z.y0.max(0)..z.y1.min(200) {
                for x in z.x0.max(0)..z.x1.min(320) {
                    fb[y as usize * 320 + x as usize] = 0;
                }
            }
        }
        if z.state & 0x20 != 0 || z.sprite == 0xFFFF {
            return;
        }
        if let Some(s) = ic.sprites.get(z.sprite as usize) {
            s.draw(fb, 320, z.x0, z.y0, DrawOpts::default());
        }
    }

    /// sub_1D6B7: first active zone containing the point (x0 < x < x1, y0 < y <= y1).
    pub fn hit(&self, x: i32, y: i32, from: usize, to: usize) -> Option<usize> {
        (from..to).find(|&i| {
            let z = &self.z[i];
            z.state & 0x80 != 0 && z.state & 0x20 == 0 && z.x0 < x && x < z.x1 && z.y0 < y && y <= z.y1
        })
    }
}
