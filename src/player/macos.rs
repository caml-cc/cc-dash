//! macOS has no MPRIS. Music and Spotify are scriptable, so ask them through
//! osascript, and only when their process is actually running.

use super::PlayerState;
use std::process::{Command, Stdio};

struct App {
    /// Process and scripting name.
    name: &'static str,
    script: &'static str,
}

// Each script prints: state, artist, title, album, length in ms, position in s, art url.
const APPS: &[App] = &[
    App {
        name: "Spotify",
        script: r#"tell application "Spotify"
    set s to (player state as text)
    set t to current track
    return s & tab & (artist of t) & tab & (name of t) & tab & (album of t) & tab & (duration of t) & tab & (player position) & tab & (artwork url of t)
end tell"#,
    },
    App {
        name: "Music",
        script: r#"tell application "Music"
    set s to (player state as text)
    set t to current track
    return s & tab & (artist of t) & tab & (name of t) & tab & (album of t) & tab & ((duration of t) * 1000) & tab & (player position) & tab & ""
end tell"#,
    },
];

pub struct NowPlaying {
    preferred: String,
}

fn is_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// osascript prints decimals with the user's locale separator.
fn num(s: &str) -> f64 {
    s.trim().replace(',', ".").parse().unwrap_or(0.0)
}

fn read_app(app: &App) -> Option<PlayerState> {
    let out = Command::new("osascript")
        .args(["-e", app.script])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = text.trim_end_matches('\n').split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    let status = match f[0] {
        "playing" => "Playing",
        "paused" => "Paused",
        _ => "Stopped",
    };
    Some(PlayerState {
        running: true,
        status: status.into(),
        artist: f[1].into(),
        title: f[2].into(),
        album: f[3].into(),
        art_url: f[6].trim().into(),
        length_us: (num(f[4]) * 1000.0) as i64,
        position_us: (num(f[5]) * 1_000_000.0) as i64,
        bus: app.name.into(),
    })
}

impl NowPlaying {
    pub fn new(preferred: &str) -> Result<NowPlaying, String> {
        Ok(NowPlaying { preferred: preferred.to_lowercase() })
    }

    /// Prefers whichever app is playing, otherwise the first one with a track.
    pub fn read(&self) -> PlayerState {
        let mut fallback: Option<PlayerState> = None;
        for app in APPS {
            if !self.preferred.is_empty() && !app.name.to_lowercase().starts_with(&self.preferred) {
                continue;
            }
            if !is_running(app.name) {
                continue;
            }
            if let Some(st) = read_app(app) {
                if st.is_playing() {
                    return st;
                }
                if fallback.is_none() {
                    fallback = Some(st);
                }
            }
        }
        fallback.unwrap_or_default()
    }
}
