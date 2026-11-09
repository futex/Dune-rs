mod img;
use dune_formats::{hnm::{Hnm, HnmPlayer, SCREEN_H, SCREEN_W}, palette::Palette, sprite::SpriteSheet, DatArchive};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: dune-tool <DUNE.DAT> <command>\n\
         commands:\n  list\n  extract <outdir> [--raw]   (decompresses HSQ unless --raw)\n  sprites <outdir> [NAME...] [--pal NAME]  contact sheet PNG per sprite resource\n  frames <NAME.HNM> <outdir> [every N]  dump video frames as PNG\n  wav <NAME> <out.wav>   export a VOC (or an HNM soundtrack) as WAV\n  text <NAME.HSQ>        dump a string table (PHRASE*/COMMAND*) as UTF-8\n  music <SONG.HSQ> <out.wav> [max_secs]  render AdLib music (play once)\n  room <FILE.SAL> <room> <BANK.HSQ> <out.png> [--pal HNM]  render a room\n"
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage()
    }
    let dat = DatArchive::open(&args[0]).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", args[0]);
        std::process::exit(1)
    });
    match args[1].as_str() {
        "list" => {
            for e in dat.entries() {
                println!("{:<16} {:>10} @{:#010x}", e.name, e.size, e.offset);
            }
        }
        "extract" => {
            let out = PathBuf::from(args.get(2).unwrap_or_else(|| usage()));
            let raw = args.iter().any(|a| a == "--raw");
            for e in dat.entries() {
                let data = if raw { dat.read_entry(e) } else { dat.read(&e.name) }.expect("read");
                let path = out.join(e.name.replace('\\', "/"));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, data).unwrap();
            }
            eprintln!("extracted {} entries to {}", dat.entries().len(), out.display());
        }
        "sprites" => {
            let out = PathBuf::from(args.get(2).unwrap_or_else(|| usage()));
            std::fs::create_dir_all(&out).unwrap();
            let mut base = Palette::default();
            let mut names = Vec::new();
            let mut it = args[3..].iter();
            while let Some(a) = it.next() {
                if a == "--pal" {
                    let src = it.next().unwrap_or_else(|| usage());
                    if src.ends_with(".HNM") {
                        base = Hnm::parse(dat.read(src).expect("read")).expect("hnm").palette;
                    } else if let Some(sh) = dat.read(src).ok().and_then(|d| SpriteSheet::parse(&d)) {
                        sh.apply_palette(&mut base);
                    }
                } else {
                    names.push(a.clone());
                }
            }
            if names.is_empty() {
                names = dat.entries().iter().filter(|e| e.name.ends_with(".HSQ")).map(|e| e.name.clone()).collect();
            }
            for n in names {
                let Ok(data) = dat.read(&n) else { continue };
                let Some(sheet) = SpriteSheet::parse(&data) else { continue };
                let mut pal = base.clone();
                sheet.apply_palette(&mut pal);
                let (w, h, fb, _) = img::contact_sheet(&sheet.sprites, 640);
                let path = out.join(format!("{}.png", n.replace('\\', "_")));
                img::write_png_indexed(&path, w, h, &fb, &pal, Some([255, 0, 255]));
                println!("{n}: {} sprites -> {}", sheet.sprites.len(), path.display());
            }
        }
        "frames" => {
            let name = args.get(2).unwrap_or_else(|| usage());
            let out = PathBuf::from(args.get(3).unwrap_or_else(|| usage()));
            let every: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
            std::fs::create_dir_all(&out).unwrap();
            let hnm = Hnm::parse(dat.read(name).expect("read")).expect("not an HNM");
            let mut pl = HnmPlayer::new(hnm);
            let mut snd = 0;
            while !pl.done() {
                let i = pl.next;
                snd += pl.step().map(|s| s.len()).unwrap_or(0);
                if i % every == 0 {
                    let p = out.join(format!("{}_{i:04}.png", name.trim_end_matches(".HNM")));
                    img::write_png_indexed(&p, SCREEN_W, SCREEN_H, &pl.fb, &pl.palette, None);
                }
            }
            println!("{name}: {} frames, {snd} audio bytes", pl.hnm.frame_count());
        }
        "wav" => {
            let name = args.get(2).unwrap_or_else(|| usage());
            let out = args.get(3).unwrap_or_else(|| usage());
            let data = dat.read(name).expect("read");
            let voc = if name.ends_with(".HNM") {
                let h = Hnm::parse(data).expect("not an HNM");
                let stream: Vec<u8> = (0..h.frame_count()).filter_map(|i| h.frame(i).sound).flatten().collect();
                dune_formats::voc::Voc::parse(&stream)
            } else {
                dune_formats::voc::Voc::parse(&data)
            }
            .expect("no VOC audio");
            write_wav(out, &voc.pcm, voc.sample_rate);
            println!("{name}: {} samples @ {} Hz{}", voc.pcm.len(), voc.sample_rate,
                voc.lipsync.as_ref().map(|l| format!(", lip-sync {} bytes", l.len())).unwrap_or_default());
        }
        "text" => {
            let name = args.get(2).unwrap_or_else(|| usage());
            let t = dune_formats::text::StringTable::parse(&dat.read(name).expect("read")).expect("not a string table");
            for (i, s) in t.strings.iter().enumerate() {
                println!("{i:04}: {}", dune_formats::text::to_unicode(s).replace('\n', " / "));
            }
        }
        "music" => {
            use dune_formats::herad::{Herad, OplOut, IRQ_HZ};
            struct Chip(nuked_opl3::Opl3Chip);
            impl OplOut for Chip {
                fn write(&mut self, r: u8, v: u8) {
                    self.0.write_register(r as u16, v)
                }
            }
            let name = args.get(2).unwrap_or_else(|| usage());
            let out = args.get(3).unwrap_or_else(|| usage());
            let max: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(600.0);
            const RATE: u32 = 22050;
            let mut chip = Chip(nuked_opl3::Opl3Chip::new(RATE));
            let mut h = Herad::new(dat.read(name).expect("read"), 1, &mut chip).expect("not a HERAD song");
            h.set_volume(255);
            let per_irq = RATE as f64 / IRQ_HZ;
            let (mut acc, mut pcm, mut st, mut peak) = (0.0, Vec::new(), [0i16; 2], 0i32);
            let mut n = 0u64;
            while h.playing && (n as f64) < max * RATE as f64 {
                acc += 1.0;
                if acc >= per_irq {
                    acc -= per_irq;
                    h.interrupt(&mut chip);
                }
                let _ = chip.0.generate_resampled(&mut st);
                let v = (st[0] as i32 + st[1] as i32) / 2;
                peak = peak.max(v.abs());
                pcm.push((v / 256 + 128).clamp(0, 255) as u8);
                n += 1;
            }
            write_wav(out, &pcm, RATE);
            println!("{name}: {:.1} s, peak {peak}", n as f64 / RATE as f64);
        }
        "room" => {
            use dune_formats::sal::{draw_room, Sal, VIEW_H, VIEW_W};
            let sal = Sal::parse(&dat.read(&args[2]).expect("read")).expect("bad SAL");
            let idx: usize = args[3].parse().unwrap();
            let bank = SpriteSheet::parse(&dat.read(&args[4]).expect("read bank")).expect("bad bank");
            let mut pal = Palette::default();
            if let Some(i) = args.iter().position(|a| a == "--pal") {
                pal = Hnm::parse(dat.read(&args[i + 1]).expect("read")).expect("hnm").palette;
            }
            bank.apply_palette(&mut pal);
            let mut fb = vec![0u8; VIEW_W * VIEW_H];
            draw_room(&mut fb, &sal.rooms[idx], Some(&bank), |_, m, fb| {
                // mark character slots with a small cross
                for d in -2i32..=2 {
                    for (x, y) in [(m.x as i32 + d, m.y as i32), (m.x as i32, m.y as i32 + d)] {
                        if (0..VIEW_W as i32).contains(&x) && (0..VIEW_H as i32).contains(&y) {
                            fb[y as usize * VIEW_W + x as usize] = 255;
                        }
                    }
                }
            });
            pal.0[255] = [63, 63, 63];
            img::write_png_indexed(std::path::Path::new(&args[5]), VIEW_W, VIEW_H, &fb, &pal, None);
        }
        _ => usage(),
    }
}

fn write_wav(path: &str, pcm: &[u8], rate: u32) {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&8u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    std::fs::write(path, w).expect("write wav");
}
