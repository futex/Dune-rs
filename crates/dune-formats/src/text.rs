//! Text resources (`PHRASE*.HSQ`, `COMMAND*.HSQ`) and the bitmap fonts (`DNCHAR*.BIN`).

/// A table of FF-terminated strings indexed by a leading u16 offset table.
pub struct StringTable {
    pub strings: Vec<Vec<u8>>,
}

pub const END: u8 = 0xFF;
pub const PAGE_BREAK: u8 = 0xFE;

impl StringTable {
    pub fn parse(data: &[u8]) -> Option<StringTable> {
        if data.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([data[0], data[1]]) as usize / 2;
        let mut strings = Vec::with_capacity(count);
        for i in 0..count {
            let o = u16::from_le_bytes([*data.get(2 * i)?, *data.get(2 * i + 1)?]) as usize;
            let s = data.get(o..).unwrap_or(&[]);
            let end = s.iter().position(|&b| b == END).unwrap_or(s.len());
            strings.push(s[..end].to_vec());
        }
        Some(StringTable { strings })
    }

    pub fn get(&self, i: usize) -> &[u8] {
        self.strings.get(i).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Lossy conversion of game text to Unicode (for logs/tools; rendering uses raw bytes).
pub fn to_unicode(s: &[u8]) -> String {
    s.iter()
        .map(|&b| match b {
            b'|' => 'é',
            b'}' => 'è',
            b'~' => 'ù',
            b'{' => 'à',
            b'\\' => 'ç',
            b'^' => 'ê',
            PAGE_BREAK => '\n',
            0x80.. => '¤',
            c => c as char,
        })
        .collect()
}

/// DNCHAR.BIN: big widths[128], small widths[128], big glyphs 128x9, small glyphs 128x7.
pub struct Font {
    pub widths: [u8; 256],
    pub glyphs: Vec<u8>,
    pub height: usize,
}

impl Font {
    /// The 9-pixel font.
    pub fn parse(data: &[u8]) -> Option<Font> {
        if data.len() < 256 + 128 * 9 {
            return None;
        }
        let mut widths = [0u8; 256];
        widths[..128].copy_from_slice(&data[..128]);
        Some(Font { widths, glyphs: data[256..256 + 128 * 9].to_vec(), height: 9 })
    }

    /// The 7-pixel font (panel command lines).
    pub fn parse_small(data: &[u8]) -> Option<Font> {
        let base = 256 + 128 * 9;
        if data.len() < base + 128 * 7 {
            return None;
        }
        let mut widths = [0u8; 256];
        widths[..128].copy_from_slice(&data[128..256]);
        Some(Font { widths, glyphs: data[base..base + 128 * 7].to_vec(), height: 7 })
    }

    pub fn glyph_row(&self, c: u8, row: usize) -> u8 {
        if c >= 128 {
            return 0;
        }
        self.glyphs.get(c as usize * self.height + row).copied().unwrap_or(0)
    }

    pub fn text_width(&self, s: &[u8]) -> usize {
        s.iter().map(|&c| self.widths[c as usize] as usize).sum()
    }

    /// Draws one line; returns the x after the last glyph.
    pub fn draw(&self, fb: &mut [u8], fb_w: usize, x: i32, y: i32, s: &[u8], color: u8) -> i32 {
        let fb_h = fb.len() / fb_w;
        let mut cx = x;
        for &c in s {
            let w = self.widths[c as usize] as i32;
            for row in 0..self.height {
                let bits = self.glyph_row(c, row);
                let py = y + row as i32;
                if py < 0 || py >= fb_h as i32 {
                    continue;
                }
                for col in 0..w.min(8) {
                    if bits & (0x80 >> col) != 0 {
                        let px = cx + col;
                        if px >= 0 && px < fb_w as i32 {
                            fb[py as usize * fb_w + px as usize] = color;
                        }
                    }
                }
            }
            cx += w;
        }
        cx
    }

    /// Greedy word wrap to `max_w` pixels. Page breaks (FE) force a new line.
    pub fn wrap(&self, s: &[u8], max_w: usize) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        for para in s.split(|&b| b == PAGE_BREAK) {
            let mut line: Vec<u8> = Vec::new();
            for word in para.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
                let candidate = if line.is_empty() { word.to_vec() } else { [&line[..], b" ", word].concat() };
                if self.text_width(&candidate) > max_w && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_vec();
                } else {
                    line = candidate;
                }
            }
            lines.push(line);
        }
        lines
    }
}
