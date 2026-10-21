//! Everything the user can change lives here, loaded from
//! ~/.config/termdash/config.toml. The file that gets written on first run is
//! DEFAULT_CONFIG below, and it parses into exactly the same values as the
//! Default impls (there is a test that checks this).

use crate::term::Rgb;
use serde::de::{Deserializer, Error as DeError};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- colours

/// A colour that may be "default", meaning: leave it to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ColorOpt(pub Option<Rgb>);

fn named(name: &str) -> Option<Rgb> {
    let c = match name {
        "black" => (0, 0, 0),
        "red" => (205, 49, 49),
        "green" => (13, 188, 121),
        "yellow" => (229, 229, 16),
        "blue" => (36, 114, 200),
        "magenta" => (188, 63, 188),
        "cyan" => (17, 168, 205),
        "white" => (229, 229, 229),
        "gray" | "grey" => (118, 118, 118),
        "bright_black" => (102, 102, 102),
        "bright_red" => (241, 76, 76),
        "bright_green" => (35, 209, 139),
        "bright_yellow" => (245, 245, 67),
        "bright_blue" => (59, 142, 234),
        "bright_magenta" => (214, 112, 214),
        "bright_cyan" => (41, 184, 219),
        "bright_white" => (255, 255, 255),
        "orange" => (255, 135, 0),
        "pink" => (255, 121, 198),
        "purple" => (165, 120, 230),
        _ => return None,
    };
    Some(Rgb(c.0, c.1, c.2))
}

fn from_256(n: u8) -> Rgb {
    const L: [u8; 6] = [0, 95, 135, 175, 215, 255];
    if n < 16 {
        let base: [(u8, u8, u8); 16] = [
            (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
            (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
            (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
            (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
        ];
        let c = base[n as usize];
        return Rgb(c.0, c.1, c.2);
    }
    if n >= 232 {
        let v = 8u16 + (n as u16 - 232) * 10;
        let v = v.min(255) as u8;
        return Rgb(v, v, v);
    }
    let i = n - 16;
    Rgb(L[(i / 36) as usize], L[((i / 6) % 6) as usize], L[(i % 6) as usize])
}

/// Accepts: "" or "default", a name like "cyan", "#38bdf8", "56,189,248",
/// or a 256-colour index such as "74".
pub fn parse_color(s: &str) -> Result<ColorOpt, String> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("default") || s.eq_ignore_ascii_case("none") {
        return Ok(ColorOpt(None));
    }
    if let Some(hex) = s.strip_prefix('#') {
        let h = if hex.len() == 3 {
            hex.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            hex.to_string()
        };
        if h.len() == 6 {
            if let Ok(v) = u32::from_str_radix(&h, 16) {
                return Ok(ColorOpt(Some(Rgb(
                    (v >> 16) as u8,
                    ((v >> 8) & 0xff) as u8,
                    (v & 0xff) as u8,
                ))));
            }
        }
        return Err(format!("bad hex colour '{}'", s));
    }
    if let Some(c) = named(&s.to_lowercase()) {
        return Ok(ColorOpt(Some(c)));
    }
    if s.contains(',') {
        let p: Vec<_> = s.split(',').map(|x| x.trim().parse::<u8>()).collect();
        if p.len() == 3 && p.iter().all(|x| x.is_ok()) {
            let v: Vec<u8> = p.into_iter().map(|x| x.unwrap()).collect();
            return Ok(ColorOpt(Some(Rgb(v[0], v[1], v[2]))));
        }
        return Err(format!("bad r,g,b colour '{}'", s));
    }
    if let Ok(n) = s.parse::<u8>() {
        return Ok(ColorOpt(Some(from_256(n))));
    }
    Err(format!("unknown colour '{}'", s))
}

impl<'de> Deserialize<'de> for ColorOpt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<ColorOpt, D::Error> {
        let s = String::deserialize(d)?;
        parse_color(&s).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------- sections

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Blank means user@hostname.
    pub title: String,
    /// Blank means PRETTY_NAME from /etc/os-release.
    pub os_name: String,
    /// What plain `termdash` does: "once" or "live".
    pub default_mode: String,
    /// Redraw rate cap in live mode.
    pub fps: u32,
    /// How long the package count is reused before it is counted again.
    pub packages_cache_secs: u64,
    /// Press q or Esc to leave live mode.
    pub quit_keys: bool,
}

impl Default for General {
    fn default() -> General {
        General {
            title: String::new(),
            os_name: String::new(),
            default_mode: "once".into(),
            fps: 30,
            packages_cache_secs: 3600,
            quit_keys: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// "auto" puts the player on the right when the window is wide enough.
    pub mode: String,
    pub left_width: usize,
    pub player_width: usize,
    /// Columns between the logo and the specs.
    pub logo_gap: usize,
    /// Columns between the two panels.
    pub side_gap: usize,
    /// Blank rows between the panels when they are stacked.
    pub stack_gap: usize,
    pub logo_width: usize,
    pub logo_height: usize,
    /// Rows into the player panel where the album cover starts.
    pub cover_top: usize,
    pub cover_max_height: usize,
    pub clock_bar_width: usize,
    /// How many columns are as wide as one row is tall. 0 asks the terminal.
    pub cover_aspect: f32,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            mode: "auto".into(),
            left_width: 80,
            player_width: 60,
            logo_gap: 2,
            side_gap: 2,
            stack_gap: 1,
            logo_width: 33,
            logo_height: 15,
            cover_top: 11,
            cover_max_height: 24,
            clock_bar_width: 24,
            cover_aspect: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Info {
    /// Which specs to show, in order. Drop any you do not want.
    pub rows: Vec<String>,
    /// Rename any row, e.g. board = "Mainboard".
    pub labels: HashMap<String, String>,
    pub underline_title: bool,
}

impl Default for Info {
    fn default() -> Info {
        Info {
            rows: [
                "os", "host", "kernel", "board", "uptime", "packages", "shell",
                "resolution", "terminal", "cpu", "gpu", "memory", "disk",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            labels: HashMap::new(),
            underline_title: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockEntry {
    pub label: String,
    /// An IANA zone such as Asia/Bahrain, or "local".
    pub tz: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clocks {
    pub show: bool,
    pub entries: Vec<ClockEntry>,
    /// Show the bar that fills up as the day passes.
    pub day_bar: bool,
    pub show_12h: bool,
}

impl Default for Clocks {
    fn default() -> Clocks {
        Clocks {
            show: true,
            entries: vec![
                ClockEntry { label: "LOCAL".into(), tz: "local".into() },
                ClockEntry { label: "TOKYO".into(), tz: "Asia/Tokyo".into() },
                ClockEntry { label: "NY".into(), tz: "America/New_York".into() },
            ],
            day_bar: true,
            show_12h: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Timers {
    pub show: bool,
    /// MM-DD, or blank to hide the birthday countdown.
    pub birthday: String,
    /// YYYY-MM-DD, or blank to hide the days-alive counter.
    pub born: String,
    pub show_new_year: bool,
    pub birthday_label: String,
    pub new_year_label: String,
    pub alive_label: String,
    /// Any extra countdowns you want: { label = "EXAM", date = "2027-01-12" }
    pub extra: Vec<ExtraTimer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraTimer {
    pub label: String,
    pub date: String,
}

impl Default for Timers {
    fn default() -> Timers {
        Timers {
            show: true,
            birthday: "03-25".into(),
            born: "2000-01-01".into(),
            show_new_year: true,
            birthday_label: "BIRTHDAY".into(),
            new_year_label: "NEW YEAR".into(),
            alive_label: "DAYS ALIVE".into(),
            extra: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerCfg {
    pub show: bool,
    /// Blank means whichever MPRIS player is running; or "spotify", "vlc", ...
    pub name: String,
    /// How often the player is asked for its state.
    pub poll_ms: u64,
    pub show_cover: bool,
    /// Seconds allowed for downloading remote cover art.
    pub art_timeout_secs: u64,
    pub nothing_playing: String,
    /// Column widths of the track line.
    pub col_time: usize,
    pub col_artist: usize,
    pub col_title: usize,
    pub col_album: usize,
    /// Show a progress bar under the track instead of the plain arrow row.
    pub progress_bar: bool,
    /// Scroll artist, title and album past when they are too long (live mode).
    pub scroll: bool,
    /// Columns per second.
    pub scroll_speed: f32,
    /// Seconds a new track is held still before it starts scrolling.
    pub scroll_delay: f32,
    /// Put between the end and the start as the text goes round.
    pub scroll_gap: String,
    /// Blank columns kept between the track columns, when there is no divider.
    pub column_gap: usize,
    /// Lines between the track columns: "unicode", "ascii" or "none".
    pub table_style: String,
}

impl Default for PlayerCfg {
    fn default() -> PlayerCfg {
        PlayerCfg {
            show: true,
            name: String::new(),
            poll_ms: 250,
            show_cover: true,
            art_timeout_secs: 5,
            nothing_playing: "Nothing playing".into(),
            col_time: 7,
            col_artist: 16,
            col_title: 25,
            col_album: 12,
            progress_bar: true,
            scroll: true,
            scroll_speed: 3.0,
            scroll_delay: 2.0,
            scroll_gap: "   •   ".into(),
            column_gap: 1,
            table_style: "unicode".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageCfg {
    /// "kitty" draws a real picture (kitty, Ghostty, WezTerm).
    /// Otherwise "blocks", "braille" or "ascii".
    pub logo_mode: String,
    pub cover_mode: String,
    /// "cover" crops to fill, "contain" fits the whole picture.
    pub fit: String,
    /// Blank means the first ~/.config/termdash/logo.* file.
    pub logo_path: String,
    /// Shown when nothing is playing, or blank for no placeholder.
    pub fallback_cover: String,
    pub saturation: f32,
    /// Above 1 brightens the midtones.
    pub gamma: f32,
    /// Auto-contrast: percent clipped at each end, 0 turns it off.
    pub contrast: f32,
    /// Unsharp mask strength in percent, 0 turns it off.
    pub sharpen: i32,
    /// braille/ascii only: 0..1, how much dim colours get brightened.
    pub boost: f32,
    /// ascii only: edge strength that switches to / \ | -, 0 turns it off.
    pub edges: f32,
    /// ascii only: your own characters, dark to bright. Blank uses a 70-step ramp.
    pub ramp: String,
    /// Draw in one colour instead of full colour, e.g. "#38bdf8".
    pub logo_tint: ColorOpt,
    pub cover_tint: ColorOpt,
    /// braille only: dither the dots instead of a hard threshold.
    pub dither: bool,
    /// Fallback character cell size for kitty mode, if the terminal does not say.
    pub cell_width_px: usize,
    pub cell_height_px: usize,
}

impl Default for ImageCfg {
    fn default() -> ImageCfg {
        ImageCfg {
            logo_mode: "kitty".into(),
            cover_mode: "kitty".into(),
            fit: "cover".into(),
            logo_path: String::new(),
            fallback_cover: String::new(),
            saturation: 1.25,
            gamma: 1.0,
            contrast: 1.0,
            sharpen: 50,
            boost: 0.55,
            edges: 72.0,
            ramp: String::new(),
            logo_tint: ColorOpt(None),
            cover_tint: ColorOpt(None),
            dither: true,
            cell_width_px: 10,
            cell_height_px: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Audio {
    pub show: bool,
    pub cava_path: String,
    /// Blank writes a config next to this file and uses that.
    pub cava_config: String,
    pub method: String,
    pub source: String,
    pub framerate: u32,
    pub lower_cutoff_freq: u32,
    pub higher_cutoff_freq: u32,
    pub noise_reduction: u32,
    /// 0..1 extra smoothing applied by termdash itself. 0 turns it off.
    pub smooth: f32,
    pub bar_width: usize,
    pub labels: Vec<String>,
}

impl Default for Audio {
    fn default() -> Audio {
        Audio {
            show: true,
            cava_path: "cava".into(),
            cava_config: String::new(),
            method: "pulse".into(),
            source: "auto".into(),
            framerate: 60,
            lower_cutoff_freq: 40,
            higher_cutoff_freq: 12000,
            noise_reduction: 60,
            smooth: 0.35,
            bar_width: 0,
            labels: ["Bass", "Mid", "Treble", "Volume"].iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub title: ColorOpt,
    pub label: ColorOpt,
    pub value: ColorOpt,
    pub separator: ColorOpt,
    pub bar_fill: ColorOpt,
    pub bar_warn: ColorOpt,
    pub bar_empty: ColorOpt,
    pub bracket: ColorOpt,
    /// Percent at which a bar turns to bar_warn.
    pub bar_warn_at: u32,
    pub header: ColorOpt,
    pub track: ColorOpt,
    pub bar_fill_char: String,
    pub bar_empty_char: String,
}

impl Default for Colors {
    fn default() -> Colors {
        Colors {
            title: parse_color("bright_cyan").unwrap(),
            label: parse_color("cyan").unwrap(),
            value: ColorOpt(None),
            separator: parse_color("gray").unwrap(),
            bar_fill: parse_color("green").unwrap(),
            bar_warn: parse_color("red").unwrap(),
            bar_empty: ColorOpt(None),
            bracket: ColorOpt(None),
            bar_warn_at: 80,
            header: parse_color("bright_cyan").unwrap(),
            track: ColorOpt(None),
            bar_fill_char: "|".into(),
            bar_empty_char: " ".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Text {
    /// How much of the right-to-left work termdash does itself:
    /// "shape-and-reorder", "reorder-only", "shape-only" or "off".
    pub rtl: String,
}

impl Default for Text {
    fn default() -> Text {
        Text { rtl: "shape-and-reorder".into() }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub text: Text,
    pub layout: Layout,
    pub info: Info,
    pub clocks: Clocks,
    pub timers: Timers,
    pub player: PlayerCfg,
    pub image: ImageCfg,
    pub audio: Audio,
    pub colors: Colors,
}

impl Config {
    pub fn dir() -> PathBuf {
        if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
            if !d.is_empty() {
                return PathBuf::from(d).join("termdash");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".config/termdash")
    }

    pub fn path() -> PathBuf {
        Config::dir().join("config.toml")
    }

    /// Load the config, writing the commented default file if there is none.
    pub fn load(explicit: Option<&Path>) -> Result<Config, String> {
        let path = explicit.map(|p| p.to_path_buf()).unwrap_or_else(Config::path);
        if !path.exists() {
            if explicit.is_some() {
                return Err(format!("{}: no such file", path.display()));
            }
            let _ = std::fs::create_dir_all(Config::dir());
            let _ = std::fs::write(&path, DEFAULT_CONFIG);
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Ok(Config::default()),
        };
        toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e))
    }
}

pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_defaults() {
        let parsed: Config = toml::from_str(DEFAULT_CONFIG).expect("default config must parse");
        let d = Config::default();
        assert_eq!(format!("{:?}", parsed.general), format!("{:?}", d.general));
        assert_eq!(format!("{:?}", parsed.text), format!("{:?}", d.text));
        assert_eq!(format!("{:?}", parsed.layout), format!("{:?}", d.layout));
        assert_eq!(format!("{:?}", parsed.info), format!("{:?}", d.info));
        assert_eq!(format!("{:?}", parsed.clocks), format!("{:?}", d.clocks));
        assert_eq!(format!("{:?}", parsed.timers), format!("{:?}", d.timers));
        assert_eq!(format!("{:?}", parsed.player), format!("{:?}", d.player));
        assert_eq!(format!("{:?}", parsed.image), format!("{:?}", d.image));
        assert_eq!(format!("{:?}", parsed.audio), format!("{:?}", d.audio));
        assert_eq!(format!("{:?}", parsed.colors), format!("{:?}", d.colors));
    }

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color("").unwrap(), ColorOpt(None));
        assert_eq!(parse_color("default").unwrap(), ColorOpt(None));
        assert_eq!(parse_color("#ff8800").unwrap(), ColorOpt(Some(Rgb(255, 136, 0))));
        assert_eq!(parse_color("#f80").unwrap(), ColorOpt(Some(Rgb(255, 136, 0))));
        assert_eq!(parse_color("1,2,3").unwrap(), ColorOpt(Some(Rgb(1, 2, 3))));
        assert!(parse_color("cyan").unwrap().0.is_some());
        assert!(parse_color("74").unwrap().0.is_some());
        assert!(parse_color("nonsense").is_err());
    }
}
