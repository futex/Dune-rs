//! Top-level state machine.

use crate::{
    assets::Assets,
    audio::Audio,
    explorer::Explorer,
    gfx::Screen,
    input::{Input, Key},
    intro::{Intro, Outcome, Prologue},
    video::{Video, VideoOpts},
    world::World,
};

pub enum Mode {
    Intro(Intro),
    Prologue(Prologue),
    World(World),
    Explorer(Explorer),
    Video { video: Video, back: Box<Mode> },
    Empty,
}

pub struct Game {
    pub assets: Assets,
    pub audio: Audio,
    pub screen: Screen,
    pub mode: Mode,
    pub quit: bool,
}

impl Game {
    pub fn new(assets: Assets, audio: Audio, skip_intro: bool, explorer: bool) -> Game {
        let mut g = Game { assets, audio, screen: Screen::new(), mode: Mode::Empty, quit: false };
        g.mode = if explorer {
            Mode::Explorer(Explorer::new(&g.assets))
        } else if skip_intro {
            Mode::World(World::new(&mut g.assets, &mut g.audio))
        } else {
            Mode::Intro(Intro::new())
        };
        g
    }

    pub fn update(&mut self, dt: f32, input: &Input) {
        self.audio.tick(dt);
        // F1 toggles the resource explorer from anywhere.
        if input.pressed(Key::F1) {
            crate::intro::stop_all(&mut self.audio);
            self.screen.brightness = 256;
            self.mode = match self.mode {
                Mode::Explorer(_) => Mode::World(World::new(&mut self.assets, &mut self.audio)),
                _ => Mode::Explorer(Explorer::new(&self.assets)),
            };
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Empty);
        self.mode = match mode {
            Mode::Empty => Mode::Intro(Intro::new()),
            Mode::Intro(mut it) => match it.update(dt, input, &mut self.assets, &mut self.audio, &mut self.screen) {
                Outcome::Running => Mode::Intro(it),
                Outcome::Skip => {
                    self.screen.brightness = 256;
                    Mode::World(World::new(&mut self.assets, &mut self.audio))
                }
                Outcome::Credits => {
                    self.screen.brightness = 256;
                    Mode::Prologue(Prologue::new(&self.assets, &mut self.audio))
                }
            },
            Mode::Prologue(mut p) => {
                if p.update(dt, input, &mut self.assets, &mut self.audio, &mut self.screen) {
                    Mode::World(World::new(&mut self.assets, &mut self.audio))
                } else {
                    Mode::Prologue(p)
                }
            }
            Mode::World(mut w) => {
                w.update(dt, input, &mut self.assets, &mut self.audio, &mut self.screen);
                if w.quit {
                    self.quit = true;
                }
                Mode::World(w)
            }
            Mode::Video { mut video, back } => {
                if input.any() || video.done() {
                    self.audio.stop();
                    *back
                } else {
                    video.update(dt, &mut self.audio, &mut self.screen);
                    Mode::Video { video, back }
                }
            }
            Mode::Explorer(mut ex) => {
                let action = ex.update(input, &mut self.assets, &mut self.audio, &mut self.screen);
                if input.pressed(Key::Char('q')) {
                    self.quit = true;
                }
                match action {
                    Some(crate::explorer::Action::PlayVideo(n)) => match self.assets.hnm(&n) {
                        Some(h) => {
                            self.audio.stop();
                            Mode::Video { video: Video::new(h, VideoOpts::for_name(&n)), back: Box::new(Mode::Explorer(ex)) }
                        }
                        None => Mode::Explorer(ex),
                    },
                    None => Mode::Explorer(ex),
                }
            }
        };
    }
}
