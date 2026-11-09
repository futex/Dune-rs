//! Intro / prologue entry points and shared helpers. The attract interpreter, the credits
//! and the prologue are in `attract.rs` (port of sub_10580 / sub_10309 / sub_1021C).

use crate::{
    assets::Assets,
    audio::Audio,
    gfx::W,
    music::Music,
};
use dune_formats::{palette::Palette, sprite::DrawOpts, voc::Voc};

pub use crate::attract::{Intro, Prologue};

pub enum Outcome {
    Running,
    /// ESC: straight into the game.
    Skip,
    /// Any other input: credits + prologue, then the game.
    Credits,
}

pub fn stop_all(audio: &mut Audio) {
    audio.stop();
    audio.set_music(None);
}

/// Starts a song; returns its shared position counter.
pub fn start_music(assets: &Assets, audio: &mut Audio, name: &str, repeat: u8) -> Option<std::sync::Arc<std::sync::atomic::AtomicU32>> {
    let m = assets.dat.read(&format!("{name}.HSQ")).ok().and_then(|d| Music::new(d, repeat));
    let pos = m.as_ref().map(|m| m.pos.clone());
    audio.set_music(m.map(|m| Box::new(m) as Box<dyn crate::audio::MusicSource>));
    pos
}

/// Starts a song; returns its position counter and its fade-request cell (see Music::fade_req).
pub fn start_music_ctl(assets: &Assets, audio: &mut Audio, name: &str, repeat: u8) -> Option<(std::sync::Arc<std::sync::atomic::AtomicU32>, std::sync::Arc<std::sync::atomic::AtomicU32>)> {
    let m = assets.dat.read(&format!("{name}.HSQ")).ok().and_then(|d| Music::new(d, repeat))?;
    let r = (m.pos.clone(), m.fade_req.clone());
    audio.set_music(Some(Box::new(m)));
    Some(r)
}

/// Plays a VOC; returns its lip-sync track, if any.
pub fn play_voc(assets: &Assets, audio: &mut Audio, name: &str) -> Option<Vec<u8>> {
    let v = assets.dat.read(name).ok().and_then(|d| Voc::parse(&d))?;
    audio.play_u8(&v.pcm, v.sample_rate);
    v.lipsync
}

/// Day/night palette (sub_13971 + sub_1398C): blob `idx` of SKY.HSQ (`dn == false`:
/// 0xF0 bytes -> colours 128..207) or SKYDN.HSQ (`dn == true`: 0x1C5 bytes -> colours
/// 73..223), followed by 0x30 bytes -> colours 240..255. Blob data is at sprite entry + 6.
pub fn apply_sky_palette(assets: &Assets, pal: &mut Palette, idx: usize, dn: bool) {
    let Ok(d) = assets.dat.read(if dn { "SKYDN.HSQ" } else { "SKY.HSQ" }) else { return };
    let rd = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let Some(t) = rd(0) else { return };
    let Some(e) = rd(t + 2 * idx).map(|r| t + r + 6) else { return };
    let set = |pal: &mut Palette, first: usize, bytes: usize, at: usize| {
        for i in 0..bytes / 3 {
            if let Some(c) = d.get(at + 3 * i..at + 3 * i + 3) {
                pal.0[first + i] = [c[0] & 63, c[1] & 63, c[2] & 63];
            }
        }
    };
    let (first, bytes) = if dn { (73, 0x1C5) } else { (128, 0xF0) };
    set(pal, first, bytes, e);
    set(pal, 240, 0x30, e + bytes);
}

/// Day/night palette index for game time `t` (sub_1395C): ds:2280[t & 15] + ((t >> 2) & 0x1C).
pub fn sky_index(assets: &Assets, t: u16) -> usize {
    let tab = assets.exe_bytes(0x2280, 16).map(|b| b.to_vec()).unwrap_or_else(|| vec![8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 10, 10, 11, 11, 11]);
    tab[(t & 15) as usize] as usize + ((t >> 2) & 0x1C) as usize
}

/// Sky bands (sub_138B4): SKY.HSQ sprites 0..3, each repeated at x = 0, 40, ... 280,
/// at y = 0, 20, 40, 60.
pub fn draw_sky(assets: &mut Assets, fb: &mut [u8]) {
    let Some(sky) = assets.sheet("SKY.HSQ") else { return };
    for band in 0..4 {
        for k in 0..8 {
            if let Some(s) = sky.sprites.get(band) {
                s.draw(fb, W, k * 40, band as i32 * 20, DrawOpts { clip: Some((0, 0, 320, 152)), ..Default::default() });
            }
        }
    }
}

