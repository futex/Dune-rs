//! Audio: an SDL callback mixing a streamed PCM channel (voices, video soundtracks)
//! with an optional music generator (the OPL music player).
//! Without an audio device the same mixer runs on an emulated clock (headless mode).

use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub const OUT_RATE: u32 = 44100;

/// Something that renders music samples on demand (implemented by the OPL player).
pub trait MusicSource: Send {
    /// Fills `out` with mono samples at OUT_RATE.
    fn render(&mut self, out: &mut [i16]);
}

#[derive(Default)]
pub struct Mixer {
    pcm: VecDeque<i16>,
    music: Option<Box<dyn MusicSource>>,
    pub pcm_volume: f32,
    pub music_volume: f32,
    scratch: Vec<i16>,
}

impl Mixer {
    fn mix(&mut self, out: &mut [i16]) {
        self.scratch.resize(out.len(), 0);
        self.scratch.fill(0);
        if let Some(m) = &mut self.music {
            m.render(&mut self.scratch);
        }
        for (o, m) in out.iter_mut().zip(&self.scratch) {
            let p = self.pcm.pop_front().unwrap_or(0) as f32 * self.pcm_volume;
            *o = (p + *m as f32 * self.music_volume).clamp(-32768.0, 32767.0) as i16;
        }
    }
}

struct Cb(Arc<Mutex<Mixer>>);

impl AudioCallback for Cb {
    type Channel = i16;
    fn callback(&mut self, out: &mut [i16]) {
        self.0.lock().unwrap().mix(out);
    }
}

pub struct Audio {
    mixer: Arc<Mutex<Mixer>>,
    _device: Option<AudioDevice<Cb>>,
    /// Headless: samples owed to the emulated device.
    debt: f64,
}

impl Audio {
    fn with_mixer() -> Arc<Mutex<Mixer>> {
        Arc::new(Mutex::new(Mixer { pcm_volume: 0.9, music_volume: 0.6, ..Default::default() }))
    }

    pub fn new(sdl: &sdl2::Sdl) -> Audio {
        let mixer = Self::with_mixer();
        let device = sdl.audio().ok().and_then(|a| {
            let spec = AudioSpecDesired { freq: Some(OUT_RATE as i32), channels: Some(1), samples: Some(1024) };
            a.open_playback(None, &spec, |_| Cb(mixer.clone())).ok()
        });
        match &device {
            Some(d) => d.resume(),
            None => eprintln!("audio: no device, continuing silently"),
        }
        Audio { mixer, _device: device, debt: 0.0 }
    }

    pub fn silent() -> Audio {
        Audio { mixer: Self::with_mixer(), _device: None, debt: 0.0 }
    }

    /// Queues unsigned 8-bit PCM at `rate` Hz (linear resampling to OUT_RATE).
    pub fn play_u8(&mut self, pcm: &[u8], rate: u32) {
        if pcm.is_empty() {
            return;
        }
        let n_out = (pcm.len() as u64 * OUT_RATE as u64 / rate as u64) as usize;
        let step = rate as f64 / OUT_RATE as f64;
        let mut m = self.mixer.lock().unwrap();
        m.pcm.reserve(n_out);
        for i in 0..n_out {
            let pos = i as f64 * step;
            let i0 = pos as usize;
            let frac = pos - i0 as f64;
            let a = pcm[i0.min(pcm.len() - 1)] as f64 - 128.0;
            let b = pcm[(i0 + 1).min(pcm.len() - 1)] as f64 - 128.0;
            m.pcm.push_back(((a + (b - a) * frac) * 256.0).clamp(-32768.0, 32767.0) as i16);
        }
    }

    /// Seconds of PCM still waiting to be played.
    pub fn queued_secs(&self) -> f32 {
        self.mixer.lock().unwrap().pcm.len() as f32 / OUT_RATE as f32
    }

    /// Headless only: consume audio as a real device would.
    pub fn tick(&mut self, dt: f32) {
        if self._device.is_some() {
            return;
        }
        self.debt += dt as f64 * OUT_RATE as f64;
        let n = self.debt as usize;
        self.debt -= n as f64;
        let mut buf = vec![0i16; n];
        self.mixer.lock().unwrap().mix(&mut buf);
    }

    pub fn stop(&mut self) {
        self.mixer.lock().unwrap().pcm.clear();
    }

    pub fn set_music(&mut self, m: Option<Box<dyn MusicSource>>) {
        self.mixer.lock().unwrap().music = m;
    }
}
