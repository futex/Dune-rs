//! Dialogue machinery (docs/re/gameflow.md §5, troops.md §1.6 / §2.9, war.md §7):
//! TALK (seg000:9472) with its resume pointer ds:47BA, the special sub-lists said with mask
//! 0x20 (sub_19F31 + sub_19F8B: 2 = troop contact, 4 = confirmations, 5 = rally, 6 = stay),
//! the record actions (table cs:A107), the phrase substitutions (sub_18944) and the work the
//! original does synchronously after a line (here `After`, run when the talk ends).

use crate::{assets::Assets, audio::Audio, intro::play_voc, sim::Sim, world::World};
use dune_formats::dialogue::Record;

/// Continuation run when the line(s) have been said (the original is synchronous).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum After {
    None,
    /// 95E2: WORK FOR ME / RALLY ME answered.
    Rally,
    /// 95E2 for a normal character (COME WITH ME).
    Come(usize),
    /// 9533: STAY HERE answered.
    Stay(usize),
    /// loc_16A89: change of occupation confirmed or refused (troop, saved +3, +12).
    SetJob { si: usize, saved3: u8, saved12: u16 },
    /// 6A45: espionage, after the move confirmation.
    Espionage { si: usize, di: usize },
    /// 776D: search for equipment after SETJOB-CONFIRM (troop, item k, source place).
    Search { si: usize, k: u8, di: usize },
    /// sub_182DA: MOVE TROOP confirmed (troop, destination).
    Move { si: usize, di: usize },
    /// 7D81: equipment changed, then the contact talk again.
    EquipDone,
    /// 7C02: contact talk line said.
    Contact,
    /// A report (event queue) told: pop it.
    Event,
    /// 2CCF: Stilgar's Water of Life.
    WaterOfLife,
    /// The ending script (cut-scene 128F): the next speaker.
    EndingNext,
}

/// sub_18944: phrase expansion. `80 hh ll` = text id, 81..8F = text id from the word table
/// ds:11EB, 90..9F nn = decimal value of ds:nn (92 = word, else byte), A0..EF = renderer
/// codes (dropped here), F0..FF ends a nested text.
pub fn expand(mem: &[u8], assets: &Assets, s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 16);
    expand_into(mem, assets, s, &mut out, 0);
    out
}

fn text_id(mem: &[u8], assets: &Assets, id: u16) -> Vec<u8> {
    let id = id as usize;
    if id == 0 {
        return vec![];
    }
    if id & 0x800 != 0 {
        return assets.phrases(2).map(|t| t.get((id & 0x7FF) - 1).to_vec()).unwrap_or_default();
    }
    let mut t = assets.commands().map(|t| t.get(id - 1).to_vec()).unwrap_or_default();
    if id == 0x73 {
        // sub_132C7 patches the number of days in (sub_1E2E3: 3 characters before the end
        // of the digit run, leading zeros as spaces).
        patch3(&mut t, mem[0x41] as u16);
    }
    if id == 0xC3 {
        patch3(&mut t, (mem[0x29] >> 1) as u16);
    }
    t
}

/// sub_1D03C + sub_1E2E3.
pub fn patch3(t: &mut [u8], v: u16) {
    let Some(p) = t.iter().position(|b| b.is_ascii_digit()) else { return };
    let mut e = p;
    while e < t.len() && t[e].is_ascii_digit() {
        e += 1;
    }
    if e < 3 {
        return;
    }
    let v = v.min(999);
    let h = (v / 100) as u8;
    let (tn, un) = (((v % 100) / 10) as u8, (v % 10) as u8);
    t[e - 3] = if h == 0 { b' ' } else { b'0' + h };
    t[e - 2] = if h == 0 && tn == 0 { b' ' } else { b'0' + tn };
    t[e - 1] = b'0' + un;
}

fn expand_into(mem: &[u8], assets: &Assets, s: &[u8], out: &mut Vec<u8>, depth: u32) {
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        i += 1;
        match c {
            0x00..=0x7F | 0xFE => out.push(c),
            0xFF => return,
            0x80 => {
                if i + 1 < s.len() + 1 && i + 1 <= s.len() {
                    let id = ((s.get(i).copied().unwrap_or(0) as u16) << 8) | s.get(i + 1).copied().unwrap_or(0) as u16;
                    i += 2;
                    if depth < 4 {
                        let t = text_id(mem, assets, id);
                        expand_into(mem, assets, &t, out, depth + 1);
                    }
                }
            }
            0x81..=0x8F => {
                let a = 0x11EB + 2 * (c & 0x0F) as usize;
                let id = u16::from_le_bytes([mem[a], mem[a + 1]]);
                if depth < 4 {
                    let t = text_id(mem, assets, id);
                    expand_into(mem, assets, &t, out, depth + 1);
                }
            }
            0x90..=0x9F => {
                let a = s.get(i).copied().unwrap_or(0) as usize;
                i += 1;
                let v = if c == 0x92 { u16::from_le_bytes([mem[a], mem[a + 1]]) } else { mem[a] as u16 };
                out.extend(v.to_string().bytes());
            }
            0xA0..=0xCF => {}
            0xD0..=0xEF => {
                // movsb (+ movsb/movsw): operand bytes of renderer codes.
                i += if c >= 0xD2 { 1 } else if c == 0xD0 { 2 } else { 3 };
            }
            _ => return,
        }
    }
}

impl World {
    pub(crate) fn sim(&mut self) -> Sim<'_> {
        Sim::new(&mut self.mem, &mut self.map, &mut self.lfsr)
    }

    /// seg000:93DF: a character is selected for talking.
    pub(crate) fn talk_select(&mut self, c: usize) {
        self.set16(0x47BE, (c as u16) << 3);
        let bit = 1u16 << c;
        let v = self.get16(0x0E) | bit;
        self.set16(0x0E, v);
        let v = self.get16(0x14) | bit;
        self.set16(0x14, v);
        self.set16(0x47A2, 0x0FD8 + 16 * c as u16);
        self.set16(0x47BA, 0);
        self.set16(0x47B6, 0);
        self.mem[0x47C2] = 0x80;
        self.mem[0x19] = 0;
        self.set16(0x47C4, c as u16);
    }

    fn dialogue_record_at(&self, p: usize) -> Option<Record> {
        let d = self.dialogue.as_ref()?;
        let b = d.data.get(p..p + 4)?;
        if b[0] == 0xFF && b[1] == 0xFF {
            return None;
        }
        Some(Record {
            offset: p,
            b0: b[0],
            condition: b[1] as u16 | ((b[2] as u16 & 0xC0) << 2),
            topic_flags: (b[2] >> 2) & 3,
            phrase: b[3] as u16 | ((b[2] as u16 & 3) << 8),
        })
    }

    /// Start offset of sub-list `idx` (= character * 8 + sub-list).
    pub(crate) fn sublist_start(&self, idx: usize) -> Option<usize> {
        let d = self.dialogue.as_ref()?;
        let p = u16::from_le_bytes([*d.data.get(2 * idx)?, *d.data.get(2 * idx + 1)?]) as usize;
        Some(p)
    }

    /// sub_19F9E: first eligible record from offset p (mask ds:47C2). Returns the record
    /// and the offset after it.
    pub(crate) fn pick_from(&self, mut p: usize) -> Option<Record> {
        let mask = self.mem[0x47C2];
        let cd = self.condit.as_ref()?;
        loop {
            let r = self.dialogue_record_at(p)?;
            let skip = r.b0 & 0x80 != 0 && r.b0 & 0x40 == 0 && r.b0 & mask != 0;
            if !skip && cd.eval(r.condition, &self.mem) {
                return Some(r);
            }
            p += 4;
        }
    }

    /// TALK TO ME (seg000:9472): the next line from the resume point.
    pub(crate) fn talk_next(&mut self, assets: &mut Assets, audio: &mut Audio) {
        let c = self.get16(0x47C4) as usize;
        if c == 2 && self.mem[0xC2] == 4 {
            self.sim().hawat_check();
        }
        self.mem[0x1B] = 0;
        let mut idx = self.get16(0x47BE) as usize;
        let mut si = self.get16(0x47BA) as usize;
        loop {
            if si == 0 {
                si = self.sublist_start(idx).unwrap_or(0xFFFF);
            }
            if si == 0xFFFF {
                return self.talk_end_menu(c);
            }
            if let Some(r) = self.pick_from(si) {
                let next = self.say_record(assets, audio, r, After::None);
                self.set16(0x47BA, next as u16);
                return;
            }
            idx += 1;
            self.set16(0x47BE, idx as u16);
            si = 0;
            if idx & 3 == 0 {
                if c == 0x0D {
                    idx &= !7;
                    self.set16(0x47BE, idx as u16);
                    self.mem[0x47C2] = 0x20;
                    continue;
                }
                return self.talk_end_menu(c);
            }
        }
    }

    /// D2E2: nothing more to say: the talk menu is closed.
    fn talk_end_menu(&mut self, _c: usize) {
        self.talk_menu_for = None;
        self.argue = None;
        self.build_menu();
        self.dirty = true;
    }

    /// sub_19F31(sub-list) + sub_19F8B: one line of a special sub-list (mask 0x20).
    /// Returns false when no line matched (the continuation still runs).
    pub(crate) fn talk_sub(&mut self, assets: &mut Assets, audio: &mut Audio, c: usize, sub: usize, after: After) -> bool {
        self.set16(0x47C4, c as u16);
        if c == 2 && self.mem[0xC2] == 4 {
            self.sim().hawat_check();
        }
        self.set16(0x47A2, 0x0FD8 + 16 * c as u16);
        let idx = (c << 3) | sub;
        let save = self.mem[0x47C2];
        self.mem[0x47C2] = 0x20;
        let r = self.sublist_start(idx).and_then(|p| self.pick_from(p));
        self.mem[0x47C2] = save;
        match r {
            Some(r) => {
                self.say_record(assets, audio, r, after);
                true
            }
            None => {
                self.run_after(assets, audio, after);
                false
            }
        }
    }

    /// sub_196F1(al): confirmation lines (sub-list 4) with ds:23 = code.
    pub(crate) fn confirm(&mut self, assets: &mut Assets, audio: &mut Audio, code: u8, after: After) {
        self.mem[0x23] = code;
        self.mem[0x47A5] = 0xFF;
        self.talk_sub(assets, audio, 0x0F, 4, after);
    }

    /// Shows a record: pages, voice, face, action. Returns the resume offset.
    pub(crate) fn say_record(&mut self, assets: &mut Assets, audio: &mut Audio, r: Record, after: After) -> usize {
        let c = self.get16(0x47C4) as usize;
        let first_time = r.b0 & 0x80 == 0;
        // Actions run with the record not yet marked as said.
        self.dialogue_action(assets, audio, r.action(), c, first_time);
        if let Some(d) = &mut self.dialogue {
            d.mark_said(&r);
        }
        self.mem[0x19] = 0xFF;
        let mut next = r.offset + 4;
        if std::mem::take(&mut self.mem[0x47A8]) != 0 {
            next = 0xFFFF;
        }
        let bank = if r.offset < self.sublist_start(48).unwrap_or(0) { 1 } else { 2 };
        let raw = assets.phrases(bank).map(|t| t.get(r.phrase as usize - 1).to_vec()).unwrap_or_default();
        let text = expand(&self.mem, assets, &raw);
        self.set16(0x4780, 0x800 | r.phrase);
        self.last_line = Some((c, r.phrase, bank));
        self.show_line(assets, audio, c, r.phrase, &text, after);
        next
    }

    /// The line on screen and its voice.
    pub(crate) fn show_line(&mut self, assets: &mut Assets, audio: &mut Audio, c: usize, phrase: u16, text: &[u8], after: After) {
        let pages = text.split(|&b| b == 0xFE).map(|p| p.to_vec()).collect();
        self.talk = Some(crate::world::Talk { pages, page: 0, timer: 0.0, after });
        let lips = self.play_line_voice(assets, audio, c, phrase);
        self.medallion_to = 0;
        if self.scr == crate::world::Scr::Room {
            self.open_closeup(assets, c, text, lips.as_deref());
        }
        self.dirty = true;
    }

    /// sub_1A6CC: the speaker's VOC.
    pub(crate) fn play_line_voice(&mut self, assets: &mut Assets, audio: &mut Audio, c: usize, phrase: u16) -> Option<Vec<u8>> {
        let first = self.dialogue.as_ref().map(|d| d.first_phrase(c)).unwrap_or(1);
        let mut sp = c.min(0x0E);
        if sp == 0x0E && phrase & 0xFE == 0x2C {
            sp = 0x0C;
        }
        let letter = (b'A' + sp as u8) as char;
        let indoors = if self.mem[0xEA] as i8 <= 0 && self.indoors() && self.place() & 0xFF != 1 { 'I' } else { 'O' };
        audio.stop();
        play_voc(assets, audio, &format!("P{letter}\\P{letter}{:03X}{indoors}.VOC", phrase.wrapping_sub(first).wrapping_add(1) & 0xFFF))
    }

    /// WHAT ? (9ED5): the last line's voice again.
    pub(crate) fn talk_what(&mut self, assets: &mut Assets, audio: &mut Audio) {
        if let Some((c, phrase, _)) = self.last_line {
            let lips = self.play_line_voice(assets, audio, c, phrase);
            if let Some(f) = &mut self.closeup {
                f.speak(lips.as_deref(), self.ticks as u64);
            }
        }
    }

    /// Record actions (cs:A107).
    fn dialogue_action(&mut self, assets: &mut Assets, audio: &mut Audio, a: u8, c: usize, first_time: bool) {
        let _ = (assets, audio);
        match a {
            1 => self.mem[0x47A5] = 0xFF,
            2 => self.mem[0x47A5] = 0,
            3 => {
                // sub_11771: a cut-scene script chosen by the story phase (not ported).
            }
            4 | 5 => {
                self.mem[0x476D] = (a == 5) as u8;
                self.mem[0x9F] = 0;
                self.argue = Some(c == 0x0D);
            }
            6 => self.mem[0x47A8] = self.mem[0x47A8].wrapping_add(1),
            7 => self.mem[0x47A5] = 0x80,
            8 => match c {
                1 => {
                    // A186: Jessica raises the command range.
                    let mut s = self.sim();
                    let ax: u16 = if s.b(0x0A) & 2 != 0 {
                        s.charisma_up(0x28);
                        0xFFCE
                    } else if s.w(0x1176) == 1 {
                        s.charisma_up(0x0A);
                        0x0A
                    } else {
                        s.w(0x1176)
                    };
                    let ax = ax.wrapping_add(0x14);
                    s.sw(0x1176, ax);
                    let bl = if ax < 0x64 { 0x80u8.wrapping_sub((ax / 6) as u8) } else { 0 };
                    s.sb(0xD5, bl);
                }
                3 => self.sim().idaho_setup(),
                5 => self.deferred = Some(After::WaterOfLife),
                0x0C => {
                    let di = self.get16(0x11CE) as usize;
                    self.mem[di + 0x0A] &= 0x7F;
                }
                0x0D => self.sim().smuggler_offer(),
                _ => {}
            },
            9 => match c {
                3 => {
                    self.sim().idaho_accept();
                }
                5 => self.sim().final_assault(),
                _ => {}
            },
            11 if first_time => {
                self.mem[0x2A] = self.mem[0x2A].wrapping_add(1);
                self.mem[0xFF] = 0;
            }
            12 if first_time => {
                let al = (self.mem[0x2A] & 0xFC).wrapping_add(4);
                self.sim().raise_phase(al);
            }
            14 if first_time => self.mem[0xC2] = self.mem[0xC2].wrapping_add(1),
            15 => match c {
                1 => self.mem[0xF5] = self.mem[0xF5].wrapping_add(1),
                3 => self.sim().idaho_reaction(),
                _ => {}
            },
            _ => {}
        }
    }

    /// The talk has ended: its continuation.
    pub(crate) fn run_after(&mut self, assets: &mut Assets, audio: &mut Audio, after: After) {
        if let Some(d) = self.deferred.take() {
            if d != after {
                self.run_after(assets, audio, d);
            }
        }
        match after {
            After::None => {}
            After::Rally => self.rally_done(assets, audio),
            After::Come(c) => {
                if self.mem[0x47A5] == 0xFF && c != 0x0E {
                    self.come_with_me(assets, c);
                }
            }
            After::Stay(c) => {
                if self.mem[0x47A5] == 0xFF {
                    self.stay_here(assets, c);
                }
            }
            After::SetJob { si, saved3, saved12 } => self.setjob_answer(si, saved3, saved12),
            After::Espionage { si, di } => self.espionage_answer(assets, si, di),
            After::Search { si, k, di } => self.search_answer(assets, si, k, di),
            After::Move { si, di } => self.move_answer(assets, si, di),
            After::EquipDone => {
                self.mem[0x46F4] = 0;
                if self.get16(0x1BEA) == 0 {
                    self.contact_talk(assets, audio);
                }
            }
            After::Contact => self.contact_said(assets),
            After::Event => {
                self.sim().pop_event();
                self.mem[0xEA] = 0xFF;
                self.vision = None;
                self.redraw_room(assets);
            }
            After::WaterOfLife => self.water_of_life(assets),
            After::EndingNext => self.ending_next(assets, audio),
        }
    }

    /// 2CCF: Stilgar's Water of Life, after ACCEPT.
    fn water_of_life(&mut self, assets: &mut Assets) {
        self.mem[0x0A] |= 8;
        if self.mem[0x9F] != 1 {
            return;
        }
        if self.mem[0x29] < 0x64 {
            self.mem[0x46D9] = 3;
            return;
        }
        self.mem[0x0A] |= 2;
        self.mem[0xD5] = 0xFF;
        crate::travel::advance_slots(self, assets, 3);
        self.mem[0x23] = 0x11;
        self.effect = Some(0x36);
        self.redraw_room(assets);
    }

    /// ARGUE / ACCEPT / REFUSE (menu ds:1FFE).
    pub(crate) fn argue_answer(&mut self, assets: &mut Assets, audio: &mut Audio, answer: u8) {
        let smug = self.argue.take().unwrap_or(false);
        self.sim().haggle(answer, smug);
        self.build_menu();
        self.talk_next(assets, audio);
    }

    /// COME WITH ME for a normal character (95E2 tail).
    fn come_with_me(&mut self, assets: &mut Assets, c: usize) {
        let rec = 0x0FD8 + 16 * c;
        if c == 0x0E {
            return;
        }
        self.mem[rec + 0x0F] |= 0x40;
        let m = self.get16(0x10) | 1 << c;
        self.set16(0x10, m);
        if let Some(k) = (0..2).find(|&k| self.mem[0x1152 + k] == 0xFF) {
            self.mem[0x1152 + k] = c as u8;
        }
        self.build_menu();
        self.render_room(assets);
    }

    /// STAY HERE (9533 tail).
    fn stay_here(&mut self, assets: &mut Assets, c: usize) {
        let rec = 0x0FD8 + 16 * c;
        self.mem[rec + 0x0F] &= 0xBF;
        let m = self.get16(0x10) & !(1 << c);
        self.set16(0x10, m);
        for k in 0..2 {
            if self.mem[0x1152 + k] == c as u8 {
                self.mem[0x1152 + k] = 0xFF;
            }
        }
        self.build_menu();
        self.render_room(assets);
    }
}
