//! HSQ: Cryo's LZ77 compression.
//!
//! Header (6 bytes):
//! ```text
//! u16 + u8  decompressed size (24 bit)
//! u16       compressed size (including header)
//! u8        checksum: the 6 header bytes sum to 0xAB (mod 256)
//! ```
//! The stream is driven by a 16-bit little-endian bit queue, refilled on demand
//! from the input and consumed LSB first:
//! * `1`       literal byte
//! * `0 0 b b` short match: count = bb + 2, offset = byte - 256
//! * `0 1`     long match: w = u16; offset = (w >> 3) - 8192; count = w & 7,
//!             if 0 then count = next byte (0 = end of stream); count += 2

#[derive(Debug)]
pub enum HsqError {
    BadHeader,
    Truncated,
    BadOffset,
}

impl std::fmt::Display for HsqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for HsqError {}

pub fn header_ok(data: &[u8]) -> bool {
    data.len() >= 6 && data[..6].iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0xAB
}

pub fn unpacked_size(data: &[u8]) -> usize {
    data[0] as usize | (data[1] as usize) << 8 | (data[2] as usize) << 16
}

/// True when `data` looks like an HSQ stream (checksum and packed size agree).
pub fn is_hsq(data: &[u8]) -> bool {
    header_ok(data) && {
        let packed = u16::from_le_bytes([data[3], data[4]]) as usize;
        packed == data.len() || packed == (data.len() & 0xFFFF)
    }
}

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
    queue: u32,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, HsqError> {
        let b = *self.src.get(self.pos).ok_or(HsqError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }
    fn word(&mut self) -> Result<u16, HsqError> {
        Ok(self.byte()? as u16 | (self.byte()? as u16) << 8)
    }
    fn bit(&mut self) -> Result<u32, HsqError> {
        if self.queue == 1 {
            self.queue = self.word()? as u32 | 0x10000;
        }
        let b = self.queue & 1;
        self.queue >>= 1;
        Ok(b)
    }
}

pub fn decompress(data: &[u8]) -> Result<Vec<u8>, HsqError> {
    if !header_ok(data) {
        return Err(HsqError::BadHeader);
    }
    let size = unpacked_size(data);
    let mut out = Vec::with_capacity(size);
    let mut r = Reader { src: data, pos: 6, queue: 1 };
    loop {
        if r.bit()? == 1 {
            out.push(r.byte()?);
            continue;
        }
        let (count, offset) = if r.bit()? == 1 {
            let w = r.word()?;
            let mut count = (w & 7) as usize;
            let offset = (w >> 3) as isize - 8192;
            if count == 0 {
                count = r.byte()? as usize;
                if count == 0 {
                    break;
                }
            }
            (count + 2, offset)
        } else {
            let hi = r.bit()?;
            let lo = r.bit()?;
            let count = ((hi << 1) | lo) as usize + 2;
            (count, r.byte()? as isize - 256)
        };
        let start = out.len() as isize + offset;
        if start < 0 {
            return Err(HsqError::BadOffset);
        }
        for i in 0..count {
            let b = out[start as usize + i];
            out.push(b);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a stream by hand: 3 literals "abc", a short match (offset -3, count 4), end.
    #[test]
    fn literals_short_match_and_end() {
        // Bits, LSB first: 1 1 1 (literals) | 0 0 1 0 (short, count=0b10+2=4) | 0 1 (long -> end)
        // = 1,1,1,0,0,1,0,0,1 -> 0b1_0010_0111 = 0x0127
        let mut body = vec![0x27, 0x01];
        body.extend_from_slice(b"abc");
        body.push(0xFD); // short offset byte: 0xFD - 256 = -3
        body.extend_from_slice(&[0x00, 0x00, 0x00]); // long: w=0 -> count byte 0 -> end
        let packed = (6 + body.len()) as u16;
        let mut hdr = vec![7, 0, 0, packed as u8, (packed >> 8) as u8, 0];
        let sum = hdr.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        hdr[5] = 0xABu8.wrapping_sub(sum);
        hdr.extend(body);
        assert!(is_hsq(&hdr));
        assert_eq!(decompress(&hdr).unwrap(), b"abcabca");
    }

    #[test]
    fn rejects_bad_checksum() {
        assert!(decompress(&[1, 0, 0, 7, 0, 0, 0]).is_err());
    }
}
