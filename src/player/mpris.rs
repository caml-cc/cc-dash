//! MPRIS over D-Bus (Linux).

use super::PlayerState;
use std::collections::HashMap;
use zbus::blocking::fdo::{DBusProxy, PropertiesProxy};
use zbus::blocking::Connection;
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedValue;

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

fn get_string(map: &HashMap<String, OwnedValue>, key: &str) -> String {
    match map.get(key) {
        Some(v) => {
            if let Ok(s) = String::try_from(v.clone()) {
                return s;
            }
            // xesam:artist is an array of strings
            if let Ok(list) = Vec::<String>::try_from(v.clone()) {
                return list.join(", ");
            }
            String::new()
        }
        None => String::new(),
    }
}

fn get_i64(map: &HashMap<String, OwnedValue>, key: &str) -> i64 {
    match map.get(key) {
        Some(v) => i64::try_from(v.clone())
            .or_else(|_| u64::try_from(v.clone()).map(|x| x as i64))
            .or_else(|_| i32::try_from(v.clone()).map(|x| x as i64))
            .unwrap_or(0),
        None => 0,
    }
}

pub struct Mpris {
    conn: Connection,
    iface: InterfaceName<'static>,
    preferred: String,
}

impl Mpris {
    pub fn new(preferred: &str) -> Result<Mpris, String> {
        let conn = Connection::session().map_err(|e| format!("session bus: {}", e))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).map_err(|e| e.to_string())?;
        Ok(Mpris { conn, iface, preferred: preferred.to_string() })
    }

    /// Bus names of every running MPRIS player, the preferred one first.
    fn players(&self) -> Vec<String> {
        let proxy = match DBusProxy::new(&self.conn) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let names = match proxy.list_names() {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<String> = names
            .into_iter()
            .map(|n| n.as_str().to_string())
            .filter(|n| n.starts_with(MPRIS_PREFIX))
            .collect();
        out.sort();
        if !self.preferred.is_empty() {
            let want = self.preferred.to_lowercase();
            out.retain(|n| n[MPRIS_PREFIX.len()..].to_lowercase().starts_with(&want));
        }
        out
    }

    fn read_one(&self, bus: &str) -> Option<PlayerState> {
        let props = PropertiesProxy::builder(&self.conn)
            .destination(bus.to_string())
            .ok()?
            .path(MPRIS_PATH)
            .ok()?
            .build()
            .ok()?;
        let all = props.get_all(self.iface.clone().into()).ok()?;
        let status = all
            .get("PlaybackStatus")
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default();
        let meta: HashMap<String, OwnedValue> = all
            .get("Metadata")
            .and_then(|v| HashMap::<String, OwnedValue>::try_from(v.clone()).ok())
            .unwrap_or_default();
        let position = all
            .get("Position")
            .map(|_| get_i64(&all, "Position"))
            .unwrap_or_else(|| {
                props
                    .get(self.iface.clone().into(), "Position")
                    .ok()
                    .and_then(|v| i64::try_from(v).ok())
                    .unwrap_or(0)
            });

        Some(PlayerState {
            running: true,
            status,
            artist: get_string(&meta, "xesam:artist"),
            title: get_string(&meta, "xesam:title"),
            album: get_string(&meta, "xesam:album"),
            art_url: get_string(&meta, "mpris:artUrl"),
            length_us: get_i64(&meta, "mpris:length").max(0),
            position_us: position.max(0),
            bus: bus.to_string(),
        })
    }

    /// Pick a player and read it. Prefers one that is actually playing.
    pub fn read(&self) -> PlayerState {
        let names = self.players();
        let mut fallback: Option<PlayerState> = None;
        for bus in names {
            if let Some(st) = self.read_one(&bus) {
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
