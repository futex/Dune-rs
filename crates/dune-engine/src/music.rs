//! AdLib music: the HERAD player driving an emulated OPL chip.

use crate::audio::{MusicSource, OUT_RATE};
use dune_formats::herad::{Herad, OplOut, IRQ_HZ};
use nuked_opl3::Opl3Chip;

struct Chip(Opl3Chip);

impl OplOut for Chip {
    fn write(&mut self, reg: u8, val: u8) {
        self.0.write_register(reg as u16, val);
    }
}

pub struct Music {
    chip: Chip,
    player: Herad,
    irq_acc: f64,
    /// Shared music position (see Herad::position); u32::MAX when stopped.
    pub pos: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Pending fade request from the game thread: 0 = none, else 0x8000_0000 | volume << 16 | duration.
    pub fade_req: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

/// The game's song names (AdLib versions).
pub const SONGS: &[&str] =
    &["ARRAKIS", "BAGDAD", "MORNING", "SEKENCE", "SIETCHM", "WARSONG", "WATER", "WORMINTR", "WORMSUIT", "CRYOMUS"];

impl Music {
    pub fn new(song: Vec<u8>, repeat: u8) -> Option<Music> {
        let mut chip = Chip(Opl3Chip::new(OUT_RATE));
        let mut player = Herad::new(song, repeat, &mut chip)?;
        player.set_volume(255);
        let pos = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(player.position()));
        let fade_req = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        Some(Music { chip, player, irq_acc: 0.0, pos, fade_req })
    }

    pub fn fade_out(&mut self) {
        self.player.fade(0, 0x300);
    }
}

impl MusicSource for Music {
    fn render(&mut self, out: &mut [i16]) {
        let per_irq = OUT_RATE as f64 / IRQ_HZ;
        let mut st = [0i16; 2];
        for o in out.iter_mut() {
            self.irq_acc += 1.0;
            if self.irq_acc >= per_irq {
                self.irq_acc -= per_irq;
                let f = self.fade_req.swap(0, std::sync::atomic::Ordering::Relaxed);
                if f != 0 {
                    self.player.fade((f >> 16) as u8, f as u16);
                }
                self.player.interrupt(&mut self.chip);
                let p = if self.player.playing { self.player.position() } else { u32::MAX };
                self.pos.store(p, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = self.chip.0.generate_resampled(&mut st);
            *o = ((st[0] as i32 + st[1] as i32) / 2).clamp(-32768, 32767) as i16;
        }
    }
}
