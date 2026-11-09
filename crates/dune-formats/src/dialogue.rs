//! Dialogue scripts (`DIALOGUE.HSQ`) and their conditions (`CONDIT.HSQ`).
//! See docs/re/gameflow.md §5.

pub const CHARACTERS: usize = 17;
pub const SUBLISTS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct Record {
    /// Offset of the record inside DIALOGUE.HSQ (its b0 byte is mutable game state).
    pub offset: usize,
    pub b0: u8,
    pub condition: u16,
    pub topic_flags: u8,
    pub phrase: u16,
}

impl Record {
    pub fn said(&self) -> bool {
        self.b0 & 0x80 != 0
    }
    pub fn repeatable(&self) -> bool {
        self.b0 & 0x40 != 0
    }
    pub fn action(&self) -> u8 {
        self.b0 & 0x0F
    }
}

pub struct Dialogue {
    /// The raw file; record b0 bytes are updated in place ("already said" bits).
    pub data: Vec<u8>,
    lists: Vec<usize>,
}

impl Dialogue {
    pub fn parse(data: Vec<u8>) -> Option<Dialogue> {
        let n = CHARACTERS * SUBLISTS;
        let lists = (0..n)
            .map(|i| data.get(2 * i..2 * i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize))
            .collect::<Option<Vec<_>>>()?;
        Some(Dialogue { data, lists })
    }

    /// Records of sub-list `idx` (= character * 8 + sub-list).
    pub fn list(&self, idx: usize) -> Vec<Record> {
        let mut out = Vec::new();
        let Some(&mut_p) = self.lists.get(idx) else { return out };
        let mut p = mut_p;
        while p + 4 <= self.data.len() {
            let b = &self.data[p..p + 4];
            if b[0] == 0xFF && b[1] == 0xFF {
                break;
            }
            out.push(Record {
                offset: p,
                b0: b[0],
                condition: b[1] as u16 | ((b[2] as u16 & 0xC0) << 2),
                topic_flags: (b[2] >> 2) & 3,
                phrase: b[3] as u16 | ((b[2] as u16 & 3) << 8),
            });
            p += 4;
        }
        out
    }

    pub fn mark_said(&mut self, r: &Record) {
        self.data[r.offset] |= 0x80;
    }

    /// Lowest phrase number used by a character (voice files are numbered from it).
    pub fn first_phrase(&self, character: usize) -> u16 {
        // Characters 14..16 share the Fremen voice folder, numbered from character 14.
        let c = if character >= 14 { 14 } else { character };
        (0..SUBLISTS).flat_map(|s| self.list(c * SUBLISTS + s)).map(|r| r.phrase).filter(|&p| p > 0).min().unwrap_or(1)
    }

    /// TALK: first eligible record whose condition holds (sub_19F9E), trying
    /// sub-lists idx, idx+1, ... while idx & 3 != 0.
    pub fn pick(&self, character: usize, condit: &Condit, mem: &[u8]) -> Option<Record> {
        let mut idx = character * SUBLISTS;
        let said_mask = 0x80;
        loop {
            for r in self.list(idx) {
                if r.said() && !r.repeatable() && r.b0 & said_mask != 0 {
                    continue;
                }
                if condit.eval(r.condition, mem) {
                    return Some(r);
                }
            }
            idx += 1;
            if idx & 3 == 0 {
                return None;
            }
        }
    }
}

pub struct Condit {
    data: Vec<u8>,
    count: usize,
}

fn rd16(m: &[u8], a: usize) -> u16 {
    u16::from_le_bytes([m.get(a).copied().unwrap_or(0), m.get(a + 1).copied().unwrap_or(0)])
}

impl Condit {
    pub fn parse(data: Vec<u8>) -> Option<Condit> {
        let count = rd16(&data, 0) as usize / 2;
        (count > 0).then_some(Condit { data, count })
    }

    fn operand(&self, p: &mut usize, mem: &[u8]) -> u16 {
        let t = self.data.get(*p).copied().unwrap_or(0xFF);
        let n = self.data.get(*p + 1).copied().unwrap_or(0) as usize;
        match t {
            0x01 => {
                *p += 2;
                mem.get(n).copied().unwrap_or(0) as u16
            }
            0x80 => {
                *p += 2;
                n as u16
            }
            0x81 => {
                let v = rd16(&self.data, *p + 1);
                *p += 3;
                v
            }
            _ => {
                *p += 2;
                rd16(mem, n)
            }
        }
    }

    fn apply(op: u8, a: u16, b: u16) -> u16 {
        match op & 0x1F {
            0x00 => (a == b) as u16,
            0x02 => (a < b) as u16,
            0x04 => (a > b) as u16,
            0x06 => (a != b) as u16,
            0x08 => ((a as i16) <= (b as i16)) as u16,
            0x0A => ((a as i16) >= (b as i16)) as u16,
            0x0C => a.wrapping_add(b),
            0x0E => a.wrapping_sub(b),
            0x10 => a & b,
            0x12 => a | b,
            _ => 0,
        }
    }

    /// Evaluates condition `k` (1-based) against the game's data-segment memory.
    /// Condition 0 is always true.
    pub fn eval(&self, k: u16, mem: &[u8]) -> bool {
        if k == 0 || k as usize > self.count {
            return k == 0;
        }
        let mut p = rd16(&self.data, 2 * (k as usize - 1)) as usize;
        let mut result: Option<u16> = None;
        let mut pending = 0u8;
        let mut cur = self.operand(&mut p, mem);
        loop {
            let op = self.data.get(p).copied().unwrap_or(0xFF);
            p += 1;
            if op == 0xFF {
                break;
            }
            if op & 0x80 != 0 {
                result = Some(match result {
                    None => cur,
                    Some(r) => Self::apply(pending, r, cur),
                });
                pending = op & 0x1F;
                cur = self.operand(&mut p, mem);
            } else {
                let b = self.operand(&mut p, mem);
                cur = Self::apply(op, cur, b);
            }
        }
        let v = match result {
            None => cur,
            Some(r) => Self::apply(pending, r, cur),
        };
        v != 0
    }
}
