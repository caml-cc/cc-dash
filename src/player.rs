//! The music player panel. On Linux it talks to MPRIS over D-Bus directly, so
//! there is no playerctl process being spawned every second. macOS has no
//! MPRIS; there it asks Music and Spotify through osascript.

use crate::config::PlayerCfg;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod mpris;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
pub use mpris::Mpris as Backend;
#[cfg(target_os = "macos")]
pub use macos::NowPlaying as Backend;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PlayerState {
    pub running: bool,
    pub status: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub art_url: String,
    pub length_us: i64,
    pub position_us: i64,
    pub bus: String,
}

impl PlayerState {
    pub fn is_playing(&self) -> bool {
        self.status == "Playing"
    }
    /// What identifies "a different track", for deciding when to reload the cover.
    pub fn track_key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}\u{1f}{}", self.artist, self.title, self.album, self.art_url)
    }
}

/// A state reading, plus when it was taken so the position can be interpolated.
#[derive(Clone, Debug)]
pub struct Sample {
    pub state: PlayerState,
    pub at: Instant,
}

/// Poll the player on its own thread so the drawing loop never waits on D-Bus.
pub fn spawn(cfg: &PlayerCfg, tx: Sender<Sample>) {
    let preferred = cfg.name.clone();
    // osascript is a process spawn, so on macOS do not poll faster than once a second.
    let floor = if cfg!(target_os = "macos") { 1000 } else { 50 };
    let period = Duration::from_millis(cfg.poll_ms.max(floor));
    std::thread::spawn(move || {
        let mut mpris = Backend::new(&preferred).ok();
        let mut last_retry = Instant::now();
        loop {
            // The session bus may not be up yet, or may go away; retry now and then.
            if mpris.is_none() && last_retry.elapsed() > Duration::from_secs(5) {
                last_retry = Instant::now();
                mpris = Backend::new(&preferred).ok();
            }
            let state = match &mpris {
                Some(m) => m.read(),
                None => PlayerState::default(),
            };
            if tx.send(Sample { state, at: Instant::now() }).is_err() {
                return;
            }
            std::thread::sleep(period);
        }
    });
}

// ---------------------------------------------------------------- cover art

fn hash64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn art_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.cache", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(base).join("termdash/art")
}

/// Get the bytes of a cover, from disk or by downloading once and caching.
pub fn fetch_art(url: &str, timeout_secs: u64) -> Option<Vec<u8>> {
    if url.is_empty() {
        return None;
    }
    if url.starts_with("file://") {
        let p = crate::imgcell::file_url_to_path(url)?;
        return std::fs::read(p).ok();
    }
    if url.starts_with('/') {
        return std::fs::read(url).ok();
    }
    if let Some(rest) = url.strip_prefix("data:") {
        // data:image/png;base64,....
        let b64 = rest.split_once("base64,")?.1;
        use base64::Engine;
        return base64::engine::general_purpose::STANDARD.decode(b64).ok();
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let dir = art_cache_dir();
    let path = dir.join(format!("{:016x}", hash64(url)));
    if let Ok(data) = std::fs::read(&path) {
        if !data.is_empty() {
            return Some(data);
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    let out = std::process::Command::new("curl")
        .args([
            "-sL",
            "--max-time",
            &timeout_secs.to_string(),
            "-o",
            path.to_str()?,
            url,
        ])
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !out.success() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let data = std::fs::read(&path).ok()?;
    if data.is_empty() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let _ = std::io::stdout().flush();
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_key_changes_with_the_track() {
        let a = PlayerState { title: "One".into(), ..Default::default() };
        let b = PlayerState { title: "Two".into(), ..Default::default() };
        assert_ne!(a.track_key(), b.track_key());
        assert_eq!(a.track_key(), a.clone().track_key());
    }

    #[test]
    fn data_urls_decode() {
        // "hi" as base64
        let got = fetch_art("data:image/png;base64,aGk=", 1).unwrap();
        assert_eq!(got, b"hi");
    }

    #[test]
    fn missing_bus_is_not_fatal() {
        // No assertion on the result: the point is that it must not panic
        // when there is no session bus, which is normal in a TTY or a container.
        let _ = Backend::new("");
    }
}
