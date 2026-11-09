use dune_formats::palette::Palette;
use std::path::Path;

pub fn write_png_indexed(path: &Path, w: usize, h: usize, px: &[u8], pal: &Palette, transparent_bg: Option<[u8; 3]>) {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for &c in px {
        match (c, transparent_bg) {
            (0, Some(bg)) => rgb.extend_from_slice(&bg),
            _ => rgb.extend_from_slice(&pal.rgb(c)),
        }
    }
    write_png_rgb(path, w, h, &rgb);
}

pub fn write_png_rgb(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let f = std::io::BufWriter::new(std::fs::File::create(path).expect("create png"));
    let mut enc = png::Encoder::new(f, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}

/// Lays out sprites left-to-right in rows of at most `max_w` pixels.
pub fn contact_sheet(sprites: &[dune_formats::sprite::Sprite], max_w: usize) -> (usize, usize, Vec<u8>, Vec<(usize, usize)>) {
    let pad = 2;
    let (mut x, mut y, mut row_h, mut w) = (0usize, 0usize, 0usize, 0usize);
    let mut pos = Vec::new();
    for s in sprites {
        let sw = s.width as usize;
        if x > 0 && x + sw > max_w {
            x = 0;
            y += row_h + pad;
            row_h = 0;
        }
        pos.push((x, y));
        x += sw + pad;
        w = w.max(x);
        row_h = row_h.max(s.height as usize);
    }
    let h = (y + row_h).max(1);
    let w = w.max(1);
    let mut fb = vec![0u8; w * h];
    for (s, &(px, py)) in sprites.iter().zip(&pos) {
        s.blit(&mut fb, w, h, px as i32, py as i32, false);
    }
    (w, h, fb, pos)
}
