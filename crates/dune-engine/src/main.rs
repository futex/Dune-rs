//! `dune` — a Rust re-implementation of the Dune (Cryo, 1992/93 CD) engine.
//! Runs on the original DUNE.DAT; no game data is bundled.

mod assets;
mod attract;
mod audio;
mod explorer;
mod face;
mod game;
mod gfx;
mod input;
mod transition;
mod intro;
mod world;
mod panel;
mod sky;
mod map_screen;
mod globe_screen;
mod travel;
mod sim;
mod talk;
mod troop_ui;
mod war_ui;
mod music;
mod video;

use assets::Assets;
use audio::Audio;
use game::Game;
use input::{Input, Key};
use sdl2::{event::Event, keyboard::Keycode, mouse::MouseButton, pixels::PixelFormatEnum};
use std::time::{Duration, Instant};

struct Opts {
    dat: Option<String>,
    lang: u8,
    skip_intro: bool,
    explorer: bool,
    scale: u32,
    /// Headless: run N simulated frames at 1/60 s and save a PNG.
    shot: Option<(usize, String)>,
    keys: Vec<(usize, Key)>,
}

fn parse_args() -> Opts {
    let mut o = Opts { dat: None, lang: 1, skip_intro: false, explorer: false, scale: 3, shot: None, keys: vec![] };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => o.lang = it.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--fr" => o.lang = 2,
            "--de" => o.lang = 3,
            "--it" => o.lang = 5,
            "--es" => o.lang = 6,
            "--skip-intro" => o.skip_intro = true,
            "--explorer" => o.explorer = true,
            // Debug: start in a given place (ds:4 value, hex), e.g. --place 2005
            "--place" => {
                if let Some(v) = it.next().and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok()) {
                    std::env::set_var("DUNE_START_PLACE", v.to_string());
                    o.skip_intro = true;
                }
            }
            "--scale" => o.scale = it.next().and_then(|s| s.parse().ok()).unwrap_or(3),
            "--shot" => {
                let n = it.next().and_then(|s| s.parse().ok()).unwrap_or(60);
                o.shot = Some((n, it.next().unwrap_or_else(|| "shot.png".into())));
            }
            // --key FRAME:KEY  (for scripted headless runs), KEY in up/down/left/right/enter/esc/tab/<char>
            "--key" => {
                if let Some((f, k)) = it.next().as_deref().and_then(|s| s.split_once(':')) {
                    let key = match k {
                        "up" => Key::Up,
                        "down" => Key::Down,
                        "left" => Key::Left,
                        "right" => Key::Right,
                        "enter" => Key::Enter,
                        "esc" => Key::Esc,
                        "tab" => Key::Tab,
                        "pgdn" => Key::PageDown,
                        c => Key::Char(c.chars().next().unwrap_or(' ')),
                    };
                    o.keys.push((f.parse().unwrap_or(0), key));
                }
            }
            "-h" | "--help" => {
                println!("usage: dune [DUNE.DAT] [--lang 1-7|--fr|--de|--it|--es] [--skip-intro] [--explorer] [--scale N]");
                std::process::exit(0)
            }
            s => o.dat = Some(s.to_string()),
        }
    }
    o
}

fn main() {
    let opts = parse_args();
    let Some(dat) = Assets::locate(opts.dat.as_deref()) else {
        eprintln!("DUNE.DAT not found. Pass its path, or set DUNE_DAT=/path/to/DUNE.DAT");
        std::process::exit(1)
    };
    let assets = Assets::open(&dat, opts.lang).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", dat.display());
        std::process::exit(1)
    });

    if let Some((frames, out)) = &opts.shot {
        let mut game = Game::new(assets, Audio::silent(), opts.skip_intro, opts.explorer);
        for f in 0..*frames {
            let keys = opts.keys.iter().filter(|(kf, _)| *kf == f).map(|(_, k)| *k).collect();
            game.update(1.0 / 60.0, &Input { keys, ..Default::default() });
        }
        save_png(&game.screen, out);
        return;
    }

    let sdl = sdl2::init().expect("SDL init");
    let video = sdl.video().expect("SDL video");
    let window = video
        .window("Dune (dune-rs)", gfx::W as u32 * opts.scale, (gfx::H as f32 * 1.2) as u32 * opts.scale)
        .position_centered()
        .resizable()
        .build()
        .expect("window");
    let mut canvas = window.into_canvas().present_vsync().build().expect("canvas");
    // 320x200 shown at 4:3, like a VGA monitor would.
    canvas.set_logical_size(gfx::W as u32 * 3, (gfx::H as f32 * 1.2) as u32 * 3).ok();
    let tc = canvas.texture_creator();
    let mut tex = tc.create_texture_streaming(PixelFormatEnum::ARGB8888, gfx::W as u32, gfx::H as u32).unwrap();
    let audio = Audio::new(&sdl);
    let mut game = Game::new(assets, audio, opts.skip_intro, opts.explorer);
    let mut events = sdl.event_pump().unwrap();
    let mut last = Instant::now();
    let mut argb = vec![0u8; gfx::W * gfx::H * 4];
    sdl.mouse().show_cursor(true);

    let mut mouse = (0, 0);
    let mut held = false;
    'run: loop {
        let mut input = Input::default();
        for ev in events.poll_iter() {
            match ev {
                Event::Quit { .. } => break 'run,
                Event::KeyDown { keycode: Some(k), .. } => {
                    let key = match k {
                        Keycode::Up => Some(Key::Up),
                        Keycode::Down => Some(Key::Down),
                        Keycode::Left => Some(Key::Left),
                        Keycode::Right => Some(Key::Right),
                        Keycode::PageUp => Some(Key::PageUp),
                        Keycode::PageDown => Some(Key::PageDown),
                        Keycode::Return | Keycode::KpEnter => Some(Key::Enter),
                        Keycode::Escape => Some(Key::Esc),
                        Keycode::Tab => Some(Key::Tab),
                        Keycode::Space => Some(Key::Space),
                        Keycode::F1 => Some(Key::F1),
                        k => {
                            let n = k.name();
                            let mut c = n.chars();
                            match (c.next(), c.next()) {
                                (Some(ch), None) => Some(Key::Char(ch.to_ascii_lowercase())),
                                _ => None,
                            }
                        }
                    };
                    if let Some(k) = key {
                        input.keys.push(k);
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    // Logical coordinates (960x720) → 320x200.
                    mouse = (x * gfx::W as i32 / 960, y * gfx::H as i32 / 720);
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, .. } => held = false,
                Event::MouseButtonDown { mouse_btn, .. } => match mouse_btn {
                    MouseButton::Left => {
                        input.clicked = true;
                        held = true;
                    }
                    MouseButton::Right => input.right_clicked = true,
                    _ => {}
                },
                _ => {}
            }
        }
        input.mouse = mouse;
        input.held = held;

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;
        game.update(dt, &input);
        if game.quit {
            break;
        }
        game.screen.to_argb(&mut argb);
        tex.update(None, &argb, gfx::W * 4).unwrap();
        canvas.clear();
        canvas.copy(&tex, None, None).unwrap();
        canvas.present();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn save_png(screen: &gfx::Screen, path: &str) {
    let mut rgb = Vec::with_capacity(gfx::W * gfx::H * 3);
    for &c in &screen.fb {
        rgb.extend_from_slice(&screen.pal.rgb(c));
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path).expect("create screenshot"));
    let mut enc = png::Encoder::new(f, gfx::W as u32, gfx::H as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    eprintln!("saved {path}");
}
