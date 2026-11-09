//! HNM playback state: decodes frames, feeds audio, paces itself on the audio clock
//! when the video has sound, or on a fixed frame rate otherwise.

use crate::{audio::Audio, gfx::Screen};
use dune_formats::{
    hnm::{Hnm, HnmPlayer, SCREEN_H, SCREEN_W},
    voc,
};

/// The game's timer rate; HNM frame periods are expressed in these ticks.
pub const TICK_HZ: f32 = 1_193_182.0 / 5957.0;

/// Per-video playback parameters (from the EXE's HNM records, see docs/re/gameflow.md §3).
#[derive(Clone)]
pub struct VideoOpts {
    pub ticks_per_frame: u8,
    /// CD FMVs: each source pixel goes to (2x, 2y), odd rows/columns stay black.
    pub dot_grid: bool,
    /// Vertical offset applied to the whole picture (320x152 scenes are shown at y=24).
    pub y_offset: usize,
    /// Stop after this frame (exclusive).
    pub last_frame: Option<usize>,
    /// Subtitle bank + (on, off) frame pairs: sprite k shown at (0,190) between pairs[k].
    pub subtitles: Option<(std::rc::Rc<dune_formats::sprite::SpriteSheet>, Vec<u16>)>,
}

impl VideoOpts {
    pub fn for_name(name: &str) -> VideoOpts {
        let n = name.trim_end_matches(".HNM");
        let ticks = match n {
            "CRYO" => 0x11,
            "CRYO2" => 0x04,
            "PRESENT" => 0x16,
            "TITLE" | "DEAD2" | "DEAD3" => 0x0A,
            "VER" | "PLANT" => 0x19,
            "CREDITS" => 0x07,
            "DFL2" => 0x12,
            "DEAD" => 0x42,
            _ => 0x10,
        };
        let dot_grid = n == "IRULAN" || n.starts_with("SEQ");
        VideoOpts { ticks_per_frame: ticks, dot_grid, y_offset: 0, last_frame: None, subtitles: None }
    }
}

pub struct Video {
    player: HnmPlayer,
    pub opts: VideoOpts,
    sound: Vec<u8>,
    sound_rate: u32,
    sound_header_done: bool,
    has_sound: bool,
    elapsed: f32,
    shown: usize,
}

impl Video {
    pub fn new(hnm: Hnm, opts: VideoOpts) -> Video {
        let has_sound = (0..hnm.frame_count().min(2)).any(|i| hnm.frame(i).sound.is_some());
        Video {
            player: HnmPlayer::new(hnm),
            opts,
            sound: Vec::new(),
            sound_rate: 11111,
            sound_header_done: false,
            has_sound,
            elapsed: 0.0,
            shown: 0,
        }
    }

    pub fn done(&self) -> bool {
        self.player.done() || self.opts.last_frame.map(|l| self.player.next >= l).unwrap_or(false)
    }

    pub fn frame(&self) -> usize {
        self.player.next
    }

    fn feed_sound(&mut self, chunk: Vec<u8>, audio: &mut Audio) {
        let pcm = if !self.sound_header_done {
            self.sound_header_done = true;
            if voc::is_voc(&chunk) {
                // First chunk holds the VOC header + start of a type-1 block.
                let hdr = u16::from_le_bytes([chunk[20], chunk[21]]) as usize;
                if chunk.len() > hdr + 6 && chunk[hdr] == 1 {
                    self.sound_rate = 1_000_000 / (256 - chunk[hdr + 4] as u32);
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
        self.sound.clear();
        self.sound.extend_from_slice(&pcm);
        audio.play_u8(&self.sound, self.sound_rate);
    }

    /// Advances playback by `dt` seconds. Returns true when the screen changed.
    pub fn update(&mut self, dt: f32, audio: &mut Audio, screen: &mut Screen) -> bool {
        let mut changed = false;
        if self.shown == 0 {
            // Pre-roll a couple of frames of audio to avoid underruns.
            changed |= self.step(audio, screen);
            if self.has_sound {
                for _ in 0..2 {
                    changed |= self.step(audio, screen);
                }
            }
            return changed;
        }
        if self.has_sound {
            // Keep ~0.25 s of audio queued; each frame carries ~1/12 s.
            while !self.done() && audio.queued_secs() < 0.25 {
                changed |= self.step(audio, screen);
            }
        } else {
            self.elapsed += dt;
            let frame_t = self.opts.ticks_per_frame as f32 / TICK_HZ;
            while !self.done() && self.elapsed >= frame_t {
                self.elapsed -= frame_t;
                changed |= self.step(audio, screen);
            }
        }
        changed
    }

    fn step(&mut self, audio: &mut Audio, screen: &mut Screen) -> bool {
        if self.done() {
            return false;
        }
        if let Some(s) = self.player.step() {
            self.feed_sound(s, audio);
        }
        self.shown += 1;
        screen.pal = self.player.palette.clone();
        let src = &self.player.fb;
        if self.opts.dot_grid {
            screen.fb.fill(0);
            for y in 0..SCREEN_H / 2 {
                for x in 0..SCREEN_W / 2 {
                    screen.fb[(y * 2) * SCREEN_W + x * 2] = src[y * SCREEN_W + x];
                }
            }
        } else {
            let off = self.opts.y_offset.min(SCREEN_H);
            screen.fb[..off * SCREEN_W].fill(0);
            screen.fb[off * SCREEN_W..].copy_from_slice(&src[..(SCREEN_H - off) * SCREEN_W]);
        }
        if let Some((bank, pairs)) = &self.opts.subtitles {
            // Frame counter as the game sees it (1-based after the step).
            let f = self.player.next as u16;
            let mut k = 0;
            while k + 1 < pairs.len() {
                if f >= pairs[k] && f < pairs[k + 1] {
                    if let Some(sp) = bank.sprites.get(k / 2) {
                        sp.draw(&mut screen.fb, SCREEN_W, 0, 190, Default::default());
                    }
                    break;
                }
                k += 2;
            }
        }
        true
    }
}
