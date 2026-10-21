//! Audio meters. cava does the FFT; we start it once with a generated config
//! and read its numbers off a pipe, so there is no polling and no restarting.

use crate::config::Audio;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

/// Bass, mid, treble, each 0..100.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Levels {
    pub bass: u32,
    pub mid: u32,
    pub treble: u32,
}

impl Levels {
    pub fn volume(&self) -> u32 {
        self.bass.max(self.mid).max(self.treble)
    }
    /// Move a fraction of the way towards `target`, for a less jumpy meter.
    pub fn smoothed(self, target: Levels, factor: f32) -> Levels {
        if factor <= 0.0 {
            return target;
        }
        let f = factor.clamp(0.0, 0.95);
        let mix = |old: u32, new: u32| -> u32 {
            (old as f32 * f + new as f32 * (1.0 - f)).round() as u32
        };
        Levels {
            bass: mix(self.bass, target.bass),
            mid: mix(self.mid, target.mid),
            treble: mix(self.treble, target.treble),
        }
    }
}

/// Parse a cava raw-ascii frame such as "12;45;80;".
pub fn parse_frame(line: &str) -> Option<Levels> {
    let mut it = line.trim().split(';').filter(|s| !s.is_empty());
    let b = it.next()?.parse::<u32>().ok()?;
    let m = it.next()?.parse::<u32>().ok()?;
    let t = it.next()?.parse::<u32>().ok()?;
    Some(Levels { bass: b.min(100), mid: m.min(100), treble: t.min(100) })
}

pub fn cava_config_text(cfg: &Audio) -> String {
    // macOS has no PulseAudio/PipeWire. cava 1.0 can tap the system output
    // through CoreAudio; the defaults ("pulse", "auto") mean that there. The
    // installer writes explicit values when it has to fall back to BlackHole.
    let mac = cfg!(target_os = "macos");
    let method = if mac && cfg.method == "pulse" { "coreaudio" } else { &cfg.method };
    let source = if mac && method == "coreaudio" && cfg.source == "auto" { "tap" } else { &cfg.source };
    format!(
        "# Written by termdash. Edit [audio] in config.toml instead; this file is\n\
         # regenerated whenever those settings change.\n\
         [general]\n\
         bars = 3\n\
         framerate = {}\n\
         autosens = 1\n\
         lower_cutoff_freq = {}\n\
         higher_cutoff_freq = {}\n\
         \n[input]\n\
         method = {}\n\
         source = {}\n\
         \n[output]\n\
         method = raw\n\
         raw_target = /dev/stdout\n\
         data_format = ascii\n\
         ascii_max_range = 100\n\
         bar_delimiter = 59\n\
         frame_delimiter = 10\n\
         channels = mono\n\
         mono_option = average\n\
         \n[smoothing]\n\
         noise_reduction = {}\n",
        cfg.framerate, cfg.lower_cutoff_freq, cfg.higher_cutoff_freq, method, source,
        cfg.noise_reduction
    )
}

/// Make sure a cava config exists and return its path.
fn ensure_config(cfg: &Audio, dir: &Path) -> Option<PathBuf> {
    if !cfg.cava_config.is_empty() {
        let p = crate::imgcell::shellexpand(&cfg.cava_config);
        return if p.exists() { Some(p) } else { None };
    }
    let path = dir.join("cava.conf");
    let want = cava_config_text(cfg);
    let same = std::fs::read_to_string(&path).map(|cur| cur == want).unwrap_or(false);
    if !same {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&path, want).ok()?;
    }
    Some(path)
}

/// A running cava process, killed when this is dropped.
pub struct Cava {
    child: Option<Child>,
}

impl Drop for Cava {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Start cava and feed levels down `tx`. Returns None when cava is not installed.
pub fn spawn(cfg: &Audio, dir: &Path, tx: Sender<Levels>) -> Option<Cava> {
    if !cfg.show {
        return None;
    }
    let conf = ensure_config(cfg, dir)?;
    let mut child = Command::new(&cfg.cava_path)
        .arg("-p")
        .arg(&conf)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if let Some(lv) = parse_frame(&line) {
                if tx.send(lv).is_err() {
                    break;
                }
            }
        }
    });
    Some(Cava { child: Some(child) })
}

/// Read a single frame, for one-shot mode. Gives up after `ms` milliseconds.
pub fn sample_once(cfg: &Audio, dir: &Path, ms: u64) -> Levels {
    let (tx, rx) = std::sync::mpsc::channel();
    let guard = spawn(cfg, dir, tx);
    if guard.is_none() {
        return Levels::default();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut last = Levels::default();
    // Take the newest frame that arrives inside the window; the first few
    // frames out of cava are usually still ramping up from zero.
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(lv) => last = lv,
            Err(_) => break,
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_parse() {
        assert_eq!(parse_frame("12;45;80;").unwrap(), Levels { bass: 12, mid: 45, treble: 80 });
        assert_eq!(parse_frame("0;0;0;").unwrap(), Levels::default());
        assert!(parse_frame("").is_none());
        assert!(parse_frame("1;2;").is_none());
        // values above the configured range are clamped rather than wrapping
        assert_eq!(parse_frame("500;1;2;").unwrap().bass, 100);
    }

    #[test]
    fn volume_is_the_loudest_band() {
        assert_eq!(Levels { bass: 10, mid: 70, treble: 30 }.volume(), 70);
    }

    #[test]
    fn smoothing_moves_towards_the_target() {
        let a = Levels { bass: 0, mid: 0, treble: 0 };
        let b = Levels { bass: 100, mid: 100, treble: 100 };
        let s = a.smoothed(b, 0.5);
        assert_eq!(s.bass, 50);
        assert_eq!(a.smoothed(b, 0.0), b);
    }

    #[test]
    fn config_text_follows_settings() {
        let mut cfg = Audio::default();
        cfg.framerate = 42;
        cfg.source = "alsa_output.monitor".into();
        let t = cava_config_text(&cfg);
        assert!(t.contains("framerate = 42"));
        assert!(t.contains("source = alsa_output.monitor"));
        assert!(t.contains("bars = 3"));
    }
}
