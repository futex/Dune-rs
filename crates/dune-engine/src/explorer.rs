//! Resource explorer: browse sprite banks, videos and dialogue text from DUNE.DAT.

use crate::{
    assets::Assets,
    audio::Audio,
    gfx::{Screen, H, W},
    input::{Input, Key},
};
use dune_formats::{palette::Palette, sprite::SpriteSheet, voc::Voc};

pub enum Action {
    PlayVideo(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Sprites,
    Videos,
    Text,
    Sounds,
}

pub struct Explorer {
    tab: Tab,
    sheets: Vec<String>,
    videos: Vec<String>,
    sounds: Vec<String>,
    /// Resources whose palette can be used as a base for sprite banks without one.
    pal_sources: Vec<String>,
    sel: [usize; 4],
    pal_sel: usize,
    sprite_page: usize,
    text_bank: u8,
    text_idx: usize,
}

const WHITE: u8 = 255;
const GREY: u8 = 254;
const BG: u8 = 253;
const HILITE: u8 = 252;

impl Explorer {
    pub fn new(assets: &Assets) -> Explorer {
        let sheets: Vec<String> = assets
            .names_with_ext(".HSQ")
            .into_iter()
            .filter(|n| assets.dat.read(n).ok().and_then(|d| SpriteSheet::parse(&d)).is_some())
            .collect();
        let videos = assets.names_with_ext(".HNM");
        let sounds = assets.names_with_ext(".VOC");
        let mut pal_sources = vec!["(none)".to_string()];
        pal_sources.extend(videos.iter().cloned());
        Explorer {
            tab: Tab::Sprites,
            sheets,
            videos,
            sounds,
            pal_sources,
            sel: [0; 4],
            pal_sel: 0,
            sprite_page: 0,
            text_bank: 1,
            text_idx: 0,
        }
    }

    fn list(&self) -> &[String] {
        match self.tab {
            Tab::Sprites => &self.sheets,
            Tab::Videos => &self.videos,
            Tab::Sounds => &self.sounds,
            Tab::Text => &[],
        }
    }

    pub fn update(&mut self, input: &Input, assets: &mut Assets, audio: &mut Audio, screen: &mut Screen) -> Option<Action> {
        let t = self.tab as usize;
        let len = self.list().len().max(1);
        let mut action = None;
        for &k in &input.keys {
            match k {
                Key::Tab => {
                    self.tab = match self.tab {
                        Tab::Sprites => Tab::Videos,
                        Tab::Videos => Tab::Text,
                        Tab::Text => Tab::Sounds,
                        Tab::Sounds => Tab::Sprites,
                    };
                    return None;
                }
                Key::Up if self.tab == Tab::Text => self.text_idx = self.text_idx.saturating_sub(1),
                Key::Down if self.tab == Tab::Text => self.text_idx += 1,
                Key::PageUp if self.tab == Tab::Text => self.text_idx = self.text_idx.saturating_sub(10),
                Key::PageDown if self.tab == Tab::Text => self.text_idx += 10,
                Key::Left | Key::Right if self.tab == Tab::Text => {
                    self.text_bank = if self.text_bank == 1 { 2 } else { 1 };
                    self.text_idx = 0;
                }
                Key::Up => {
                    self.sel[t] = (self.sel[t] + len - 1) % len;
                    self.sprite_page = 0;
                }
                Key::Down => {
                    self.sel[t] = (self.sel[t] + 1) % len;
                    self.sprite_page = 0;
                }
                Key::PageUp => self.sel[t] = self.sel[t].saturating_sub(10),
                Key::PageDown => self.sel[t] = (self.sel[t] + 10).min(len - 1),
                Key::Right if self.tab == Tab::Sprites => self.sprite_page += 1,
                Key::Left if self.tab == Tab::Sprites => self.sprite_page = self.sprite_page.saturating_sub(1),
                Key::Char('p') => self.pal_sel = (self.pal_sel + 1) % self.pal_sources.len(),
                Key::Char('o') => self.pal_sel = (self.pal_sel + self.pal_sources.len() - 1) % self.pal_sources.len(),
                Key::Char(c @ '1'..='7') => assets.lang = c as u8 - b'0',
                Key::Enter | Key::Space => match self.tab {
                    Tab::Videos => action = self.videos.get(self.sel[t]).cloned().map(Action::PlayVideo),
                    Tab::Sounds => {
                        if let Some(v) = self.sounds.get(self.sel[t]).and_then(|n| assets.dat.read(n).ok()).and_then(|d| Voc::parse(&d)) {
                            audio.stop();
                            audio.play_u8(&v.pcm, v.sample_rate);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        self.draw(assets, screen);
        action
    }

    fn base_palette(&self, assets: &Assets) -> Palette {
        let mut pal = Palette::default();
        if self.pal_sel > 0 {
            if let Some(h) = assets.hnm(&self.pal_sources[self.pal_sel]) {
                pal = h.palette.clone();
            }
        }
        pal
    }

    fn ui_colors(pal: &mut Palette) {
        pal.0[WHITE as usize] = [63, 63, 63];
        pal.0[GREY as usize] = [40, 40, 40];
        pal.0[BG as usize] = [6, 6, 10];
        pal.0[HILITE as usize] = [63, 50, 20];
    }

    fn draw_list(&self, assets: &Assets, screen: &mut Screen, x: i32, w: i32) {
        let list = self.list();
        let sel = self.sel[self.tab as usize];
        let rows = 18usize;
        let start = sel.saturating_sub(rows / 2).min(list.len().saturating_sub(rows));
        screen.fill_rect(x, 10, w, H as i32 - 10, BG);
        for (i, n) in list.iter().enumerate().skip(start).take(rows) {
            let y = 12 + (i - start) as i32 * 10;
            assets.font.draw(&mut screen.fb, W, x + 2, y, n.as_bytes(), if i == sel { HILITE } else { GREY });
        }
    }

    fn header(&self, assets: &Assets, screen: &mut Screen, extra: &str) {
        screen.fill_rect(0, 0, W as i32, 10, BG);
        let names = ["SPRITES", "VIDEOS", "TEXT", "SOUNDS"];
        let mut x = 2;
        for (i, n) in names.iter().enumerate() {
            x = assets.font.draw(&mut screen.fb, W, x, 1, n.as_bytes(), if i == self.tab as usize { HILITE } else { GREY }) + 8;
        }
        assets.font.draw(&mut screen.fb, W, x, 1, extra.as_bytes(), WHITE);
    }

    fn draw(&mut self, assets: &mut Assets, screen: &mut Screen) {
        let mut pal = self.base_palette(assets);
        screen.clear(0);
        match self.tab {
            Tab::Sprites => {
                let name = self.sheets.get(self.sel[0]).cloned().unwrap_or_default();
                if let Some(sheet) = assets.sheet(&name) {
                    sheet.apply_palette(&mut pal);
                    // Lay sprites out in the area right of the list, paging with Left/Right.
                    let (ax, ay, aw, ah) = (82i32, 11i32, W as i32 - 82, H as i32 - 11);
                    let mut pages: Vec<Vec<(usize, i32, i32)>> = vec![vec![]];
                    let (mut x, mut y, mut row_h) = (0, 0, 0);
                    for (i, s) in sheet.sprites.iter().enumerate() {
                        let (w, h) = (s.width as i32, s.height as i32);
                        if x > 0 && x + w > aw {
                            x = 0;
                            y += row_h + 2;
                            row_h = 0;
                        }
                        if y > 0 && y + h > ah {
                            pages.push(vec![]);
                            x = 0;
                            y = 0;
                            row_h = 0;
                        }
                        pages.last_mut().unwrap().push((i, x, y));
                        x += w + 2;
                        row_h = row_h.max(h);
                    }
                    self.sprite_page = self.sprite_page.min(pages.len() - 1);
                    for &(i, x, y) in &pages[self.sprite_page] {
                        sheet.sprites[i].blit(&mut screen.fb, W, H, ax + x, ay + y, false);
                    }
                    let info = format!(
                        "{} spr  page {}/{}  pal:{} (P)",
                        sheet.sprites.len(),
                        self.sprite_page + 1,
                        pages.len(),
                        self.pal_sources[self.pal_sel]
                    );
                    self.header(assets, screen, &info);
                }
                self.draw_list(assets, screen, 0, 80);
            }
            Tab::Videos | Tab::Sounds => {
                self.draw_list(assets, screen, 0, 100);
                self.header(assets, screen, "ENTER: play");
            }
            Tab::Text => {
                self.header(assets, screen, &format!("PHRASE{}{}  lang 1-7  bank </>", assets.lang, self.text_bank));
                screen.fill_rect(0, 10, W as i32, H as i32 - 10, BG);
                if let Some(t) = assets.phrases(self.text_bank) {
                    self.text_idx = self.text_idx.min(t.strings.len().saturating_sub(1));
                    let mut y = 14;
                    for i in self.text_idx..t.strings.len() {
                        let lines = assets.font.wrap(t.get(i), W - 40);
                        if y + lines.len() as i32 * 10 > H as i32 {
                            break;
                        }
                        assets.font.draw(&mut screen.fb, W, 2, y, format!("{i:03}").as_bytes(), HILITE);
                        for l in lines {
                            assets.font.draw(&mut screen.fb, W, 30, y, &l, if i == self.text_idx { WHITE } else { GREY });
                            y += 10;
                        }
                        y += 3;
                    }
                }
            }
        }
        Self::ui_colors(&mut pal);
        screen.pal = pal;
    }
}
