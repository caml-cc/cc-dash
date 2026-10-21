//! A stand-in MPRIS player, for trying the player panel without a real one.
//!
//!   dbus-run-session -- bash -c '
//!     cargo run --release --features fake-player --example fake_player -- "Title" "Artist" "Album" &
//!     sleep 1
//!     cargo run --release -- --live
//!   '
//!
//! It is a development tool and is not installed.

use std::collections::HashMap;
use std::time::Instant;
use zbus::zvariant::{OwnedValue, Value};

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property)]
    fn identity(&self) -> String {
        "Fake Player".into()
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
}

struct Player {
    title: String,
    artist: String,
    album: String,
    art: String,
    started: Instant,
    length_us: i64,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        "Playing".into()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m: HashMap<String, OwnedValue> = HashMap::new();
        let mut put = |k: &str, v: Value| {
            if let Ok(o) = OwnedValue::try_from(v) {
                m.insert(k.to_string(), o);
            }
        };
        put("xesam:title", Value::from(self.title.clone()));
        put("xesam:artist", Value::from(vec![self.artist.clone()]));
        put("xesam:album", Value::from(self.album.clone()));
        put("mpris:length", Value::from(self.length_us));
        if !self.art.is_empty() {
            put("mpris:artUrl", Value::from(self.art.clone()));
        }
        m
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        let t = self.started.elapsed().as_micros() as i64;
        if self.length_us > 0 {
            t % self.length_us
        } else {
            t
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let player = Player {
        title: a.first().cloned().unwrap_or_else(|| "A Very Long Song Title That Will Not Fit".into()),
        artist: a.get(1).cloned().unwrap_or_else(|| "An Artist With A Long Name".into()),
        album: a.get(2).cloned().unwrap_or_else(|| "An Album With A Long Name".into()),
        art: a.get(3).cloned().unwrap_or_default(),
        started: Instant::now(),
        length_us: 240_000_000,
    };
    let _conn = zbus::blocking::connection::Builder::session()?
        .name("org.mpris.MediaPlayer2.fakeplayer")?
        .serve_at("/org/mpris/MediaPlayer2", Root)?
        .serve_at("/org/mpris/MediaPlayer2", player)?
        .build()?;
    eprintln!("fake_player: on the bus as org.mpris.MediaPlayer2.fakeplayer");
    std::thread::sleep(std::time::Duration::from_secs(3600));
    Ok(())
}
