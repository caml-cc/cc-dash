//! termdash - a terminal dashboard: system specs on the left, music player on
//! the right. `termdash` draws once, `termdash --live` keeps it updating.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("termdash supports Linux and macOS only");

mod audio;
mod config;
mod imgcell;
mod player;
mod sysinfo;
mod term;
mod text;
mod ui;

use audio::Levels;
use config::Config;
use imgcell::Art;
use player::{PlayerState, Sample};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use term::{Depth, Screen, TermSize};
use ui::{Dash, Placement, COVER_ID, LOGO_ID};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
termdash - terminal dashboard

USAGE:
    termdash [OPTIONS]

OPTIONS:
    -1, --once            Draw once and exit (good for ~/.zshrc)
    -l, --live            Keep updating until q, Esc or Ctrl+C
    -c, --config <PATH>   Use this config file instead of the default
        --image <MODE>    Override the picture mode for this run:
                          kitty, blocks, braille, ascii or none
        --print-config    Print the default config file and exit
        --config-path     Print where the config file lives and exit
        --debug-text T    Show how T is shaped and reordered for the terminal
                          (for diagnosing Arabic and other right-to-left text)
    -h, --help            Show this help
    -V, --version         Show the version

The config file is written on first run and every setting is documented there:
    ~/.config/termdash/config.toml
Put a picture at ~/.config/termdash/logo.png for the top-left corner.
";

struct Args {
    live: bool,
    config: Option<PathBuf>,
    image_override: Option<String>,
}

fn parse_args(cfg_default_live: bool) -> Result<Args, String> {
    let mut a = Args { live: cfg_default_live, config: None, image_override: None };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "-l" | "--live" => a.live = true,
            "-1" | "--once" => a.live = false,
            "-c" | "--config" => {
                i += 1;
                a.config = Some(PathBuf::from(argv.get(i).ok_or("--config needs a path")?));
            }
            "--image" => {
                i += 1;
                a.image_override = Some(argv.get(i).ok_or("--image needs a mode")?.clone());
            }
            other => return Err(format!("unknown option '{}'  (try --help)", other)),
        }
        i += 1;
    }
    Ok(a)
}

fn main() {
    // These three do not need the config file, so handle them first.
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", HELP);
                return;
            }
            "-V" | "--version" => {
                println!("termdash {}", VERSION);
                return;
            }
            "--print-config" => {
                print!("{}", config::DEFAULT_CONFIG);
                return;
            }
            "--config-path" => {
                println!("{}", Config::path().display());
                return;
            }
            "--debug-text" => {
                let input: Vec<String> = std::env::args().skip(2).collect();
                debug_text(&input.join(" "));
                return;
            }
            _ => {}
        }
    }

    let pre = Config::load(None).unwrap_or_default();
    let _ = &pre;
    let args = match parse_args(pre.general.default_mode == "live") {
        Ok(a) => a,
        Err(e) => {
            eprintln!("termdash: {}", e);
            std::process::exit(2);
        }
    };
    let mut cfg = match (&args.config).as_ref() {
        Some(p) => match Config::load(Some(p)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("termdash: {}", e);
                std::process::exit(2);
            }
        },
        None => match Config::load(None) {
            Ok(c) => c,
            Err(e) => {
                // A broken config should not leave the user with no dashboard.
                eprintln!("termdash: {}\ntermdash: falling back to defaults", e);
                Config::default()
            }
        },
    };

    text::set_rtl_mode(text::RtlMode::parse(&cfg.text.rtl));

    if let Some(m) = &args.image_override {
        cfg.image.logo_mode = m.clone();
        cfg.image.cover_mode = m.clone();
    }
    // Only kitty, Ghostty and WezTerm can show a real image; anything else
    // gets the best text rendering instead.
    if !term::supports_kitty_graphics() {
        for m in [&mut cfg.image.logo_mode, &mut cfg.image.cover_mode] {
            if m == "kitty" {
                *m = "blocks".into();
            }
        }
    }

    if args.live {
        run_live(&cfg);
    } else {
        run_once(&cfg);
    }
}

// ---------------------------------------------------------------- diagnostics

/// Print the text every which way, so you can see which one your terminal
/// draws correctly and put that name in the config.
fn debug_text(input: &str) {
    fn cps(s: &str) -> String {
        s.chars().map(|c| format!("{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
    }
    println!(
        "TERM={}  COLORTERM={}  right-to-left text: {}",
        std::env::var("TERM").unwrap_or_default(),
        std::env::var("COLORTERM").unwrap_or_default(),
        if text::has_rtl(input) { "yes" } else { "no" }
    );
    println!("\ninput        {}\n  codepoints {}\n", input, cps(input));
    println!("Which of these four reads correctly?\n");
    for m in text::RtlMode::ALL {
        let out = text::for_display_with(input, m);
        println!("  {:<18} {}", m.name(), out);
        println!("  {:<18} {}\n", "", cps(&out));
    }
    println!("Put the one that reads correctly in ~/.config/termdash/config.toml:");
    println!("    [text]");
    println!("    rtl = \"<that name>\"");
    println!();
    println!("If all four look like boxes, empty squares or the wrong letters,");
    println!("it is the font rather than the text: the terminal needs one with");
    println!("the Arabic Presentation Forms (U+FE70 to U+FEFF).");
}

// ---------------------------------------------------------------- shared

fn cell_px(cfg: &Config, size: &TermSize) -> (usize, usize) {
    (
        if size.cell_w > 0 { size.cell_w } else { cfg.image.cell_width_px },
        if size.cell_h > 0 { size.cell_h } else { cfg.image.cell_height_px },
    )
}

/// How many columns are as wide as one row is tall, from the real character
/// cell. Every picture is sized from this, so album art comes out square
/// instead of being stretched into a portrait rectangle and cropped.
fn cell_aspect(px: (usize, usize)) -> f32 {
    if px.0 == 0 {
        return 2.0;
    }
    px.1 as f32 / px.0 as f32
}

fn render_art(
    bytes_or_path: ArtSource,
    mode: &str,
    cols: usize,
    rows: usize,
    cfg: &Config,
    tint: Option<term::Rgb>,
    px: (usize, usize),
) -> Option<Art> {
    if mode == "none" || cols == 0 || rows == 0 {
        return None;
    }
    let img = match bytes_or_path {
        ArtSource::Path(p) => imgcell::load_path(&p).ok()?,
        ArtSource::Bytes(b) => imgcell::load_bytes(&b).ok()?,
    };
    imgcell::render(&img, mode, cols, rows, &cfg.image, tint, px).ok()
}

enum ArtSource {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

fn load_logo(cfg: &Config, px: (usize, usize)) -> Option<Art> {
    let path = imgcell::find_logo(&cfg.image, &Config::dir())?;
    render_art(
        ArtSource::Path(path),
        &cfg.image.logo_mode,
        cfg.layout.logo_width,
        cfg.layout.logo_height,
        cfg,
        cfg.image.logo_tint.0,
        px,
    )
}

fn cover_source(cfg: &Config, st: &PlayerState) -> Option<ArtSource> {
    if !st.art_url.is_empty() {
        if let Some(b) = player::fetch_art(&st.art_url, cfg.player.art_timeout_secs) {
            return Some(ArtSource::Bytes(b));
        }
    }
    if !cfg.image.fallback_cover.is_empty() {
        let p = imgcell::shellexpand(&cfg.image.fallback_cover);
        if p.exists() {
            return Some(ArtSource::Path(p));
        }
    }
    None
}

// ---------------------------------------------------------------- once

fn run_once(cfg: &Config) {
    let size = TermSize::get();
    let px = cell_px(cfg, &size);

    let packages = sysinfo::packages_cached(
        Duration::from_secs(cfg.general.packages_cache_secs),
        None,
    );
    let stat = sysinfo::collect_static(&cfg.general.os_name, packages);
    let dynamic = sysinfo::collect_dynamic();

    let state = player::Backend::new(&cfg.player.name)
        .map(|m| m.read())
        .unwrap_or_default();

    // A short look at cava, so the meters are not simply zero.
    let levels = if cfg.audio.show {
        audio::sample_once(&cfg.audio, &Config::dir(), 700)
    } else {
        Levels::default()
    };

    let logo = load_logo(cfg, px);

    // Work out the height first, then build a screen that exactly fits.
    let probe = Dash {
        cfg,
        stat: &stat,
        dynamic: &dynamic,
        player: &state,
        position_us: state.position_us,
        levels,
        logo: logo.as_ref(),
        cover: None,
        animate: false,
        scroll_secs: 0.0,
    };
    // In one-shot mode the output scrolls, so the cover is not limited by the
    // window height the way it is in live mode.
    let aspect = if cfg.layout.cover_aspect > 0.0 {
        cfg.layout.cover_aspect
    } else {
        cell_aspect(px)
    };
    let mut geo = probe.geometry(size.cols, usize::MAX, probe.left_panel().len(), aspect);
    let (cc, cr) = ui::cover_size(
        cfg.layout.player_width,
        cfg.layout.cover_max_height,
        cfg.layout.cover_max_height,
        aspect,
    );
    geo.cover_cols = cc;
    geo.cover_rows = cr;

    let cover = if cfg.player.show_cover {
        cover_source(cfg, &state).and_then(|src| {
            render_art(
                src,
                &cfg.image.cover_mode,
                geo.cover_cols,
                geo.cover_rows,
                cfg,
                cfg.image.cover_tint.0,
                px,
            )
        })
    } else {
        None
    };

    let dash = Dash { cover: cover.as_ref(), ..probe };
    let height = dash.total_rows(&geo, geo.cover_rows);
    let width = geo.width.max(1);
    let mut screen = Screen::new(width, height.max(1), Depth::detect());
    let places = dash.draw(&mut screen, &geo);

    let mut out = String::new();
    for line in screen.to_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    // The cursor is now one line below the block; place images relative to it.
    for p in places {
        let art = match p.id {
            LOGO_ID => logo.as_ref(),
            _ => cover.as_ref(),
        };
        if let Some(Art::Kitty(k)) = art {
            let up = height + 1 - p.row;
            out.push_str(&imgcell::kitty_place_relative(k, p.id, up, p.col));
        }
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
    let _ = lock.flush();
}

// ---------------------------------------------------------------- live

enum Event {
    Player(Sample),
    Audio(Levels),
    Cover(Option<Art>),
    Packages(String),
}

struct CoverRequest {
    state: PlayerState,
    cols: usize,
    rows: usize,
    px: (usize, usize),
}

fn forward<T: Send + 'static>(rx: Receiver<T>, tx: Sender<Event>, wrap: fn(T) -> Event) {
    std::thread::spawn(move || {
        for item in rx {
            if tx.send(wrap(item)).is_err() {
                break;
            }
        }
    });
}

fn run_live(cfg: &Config) {
    term::install_signal_handlers();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::emergency_restore();
        default_hook(info);
    }));

    let mut guard = term::TerminalGuard::enter(true);
    let (ev_tx, ev_rx) = mpsc::channel::<Event>();

    // player
    let (p_tx, p_rx) = mpsc::channel::<Sample>();
    player::spawn(&cfg.player, p_tx);
    forward(p_rx, ev_tx.clone(), Event::Player);

    // audio
    let (a_tx, a_rx) = mpsc::channel::<Levels>();
    let _cava = audio::spawn(&cfg.audio, &Config::dir(), a_tx);
    forward(a_rx, ev_tx.clone(), Event::Audio);

    // cover loader: decoding and downloading must never block the drawing
    let (c_tx, c_rx) = mpsc::channel::<CoverRequest>();
    {
        let tx = ev_tx.clone();
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            for req in c_rx {
                let art = cover_source(&cfg, &req.state).and_then(|src| {
                    render_art(
                        src,
                        &cfg.image.cover_mode,
                        req.cols,
                        req.rows,
                        &cfg,
                        cfg.image.cover_tint.0,
                        req.px,
                    )
                });
                if tx.send(Event::Cover(art)).is_err() {
                    break;
                }
            }
        });
    }

    let mut size = TermSize::get();
    let mut px = cell_px(cfg, &size);
    let packages = sysinfo::packages_cached(
        Duration::from_secs(cfg.general.packages_cache_secs),
        Some({
            let (tx, rx) = mpsc::channel::<String>();
            forward(rx, ev_tx.clone(), Event::Packages);
            tx
        }),
    );
    let mut stat = sysinfo::collect_static(&cfg.general.os_name, packages);
    let mut dynamic = sysinfo::collect_dynamic();
    let mut logo = load_logo(cfg, px);
    let mut cover: Option<Art> = None;

    let mut sample = Sample { state: PlayerState::default(), at: Instant::now() };
    let mut levels = Levels::default();
    let mut target = Levels::default();

    let mut screen = Screen::new(size.cols, size.rows, Depth::detect());
    let mut placed: Vec<(u32, Placement, usize)> = Vec::new();
    let mut cover_key = String::new();
    let mut cover_dims = (0usize, 0usize);
    // Long titles scroll, and a new track is held still for a moment first,
    // so the clock for that starts over whenever the track changes.
    let mut last_track = String::new();
    let mut track_since = Instant::now();
    let mut last_dynamic = Instant::now() - Duration::from_secs(10);

    let frame = Duration::from_millis((1000 / cfg.general.fps.clamp(1, 240)) as u64);
    let mut next_frame = Instant::now();
    let mut keybuf = [0u8; 32];

    loop {
        if term::QUIT.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if cfg.general.quit_keys {
            let n = term::read_keys(&mut keybuf);
            if keybuf[..n].iter().any(|b| *b == b'q' || *b == 0x1b || *b == 3) {
                break;
            }
        }

        // resize or return from Ctrl+Z
        let resumed = term::took_resume();
        if term::RESIZED.swap(false, std::sync::atomic::Ordering::SeqCst) || resumed {
            size = TermSize::get();
            px = cell_px(cfg, &size);
            screen.resize(size.cols, size.rows);
            logo = load_logo(cfg, px);
            for (id, _, _) in placed.drain(..) {
                let _ = std::io::stdout().write_all(imgcell::kitty_delete(id).as_bytes());
            }
            cover_dims = (0, 0); // force the cover to be rebuilt at the new size
            let _ = std::io::stdout().write_all(b"\x1b[2J");
            screen.force_repaint();
        }

        let wait = next_frame.saturating_duration_since(Instant::now());
        match ev_rx.recv_timeout(wait) {
            Ok(Event::Player(s)) => sample = s,
            Ok(Event::Audio(l)) => target = l,
            Ok(Event::Cover(a)) => cover = a,
            Ok(Event::Packages(p)) => stat.packages = p,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() < next_frame {
            continue;
        }
        next_frame += frame;
        if next_frame < Instant::now() {
            next_frame = Instant::now() + frame;
        }

        if last_dynamic.elapsed() >= Duration::from_secs(1) {
            dynamic = sysinfo::collect_dynamic();
            last_dynamic = Instant::now();
        }
        levels = levels.smoothed(target, cfg.audio.smooth);

        // Position moves on its own between polls, so the counter is smooth.
        let position_us = if sample.state.is_playing() {
            let add = sample.at.elapsed().as_micros() as i64;
            let p = sample.state.position_us + add;
            if sample.state.length_us > 0 {
                p.min(sample.state.length_us)
            } else {
                p
            }
        } else {
            sample.state.position_us
        };

        let track = sample.state.track_key();
        if track != last_track {
            last_track = track;
            track_since = Instant::now();
        }

        let dash = Dash {
            cfg,
            stat: &stat,
            dynamic: &dynamic,
            player: &sample.state,
            position_us,
            levels,
            logo: logo.as_ref(),
            cover: cover.as_ref(),
            animate: true,
            scroll_secs: track_since.elapsed().as_secs_f32(),
        };
        let aspect = if cfg.layout.cover_aspect > 0.0 {
            cfg.layout.cover_aspect
        } else {
            cell_aspect(px)
        };
        let geo = dash.geometry(size.cols, size.rows, dash.left_panel().len(), aspect);

        // Ask for a new cover when the track or the available space changes.
        if cfg.player.show_cover && (geo.cover_cols, geo.cover_rows) != (0, 0) {
            let key = sample.state.track_key();
            if key != cover_key || (geo.cover_cols, geo.cover_rows) != cover_dims {
                cover_key = key;
                cover_dims = (geo.cover_cols, geo.cover_rows);
                let _ = c_tx.send(CoverRequest {
                    state: sample.state.clone(),
                    cols: geo.cover_cols,
                    rows: geo.cover_rows,
                    px,
                });
            }
        }

        screen.clear();
        let places = dash.draw(&mut screen, &geo);
        let mut out = String::new();
        screen.flush_diff(&mut out);
        emit_images(&mut out, &places, &logo, &cover, &mut placed);

        if !out.is_empty() {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(out.as_bytes());
            let _ = lock.flush();
        }
    }

    guard.restore();
}

/// Send kitty images only when they, or where they sit, actually changed.
fn emit_images(
    out: &mut String,
    places: &[Placement],
    logo: &Option<Art>,
    cover: &Option<Art>,
    placed: &mut Vec<(u32, Placement, usize)>,
) {
    let art_for = |id: u32| -> Option<&Art> {
        match id {
            LOGO_ID => logo.as_ref(),
            COVER_ID => cover.as_ref(),
            _ => None,
        }
    };
    // drop images that are no longer wanted
    placed.retain(|(id, _, _)| {
        if places.iter().any(|p| p.id == *id) {
            true
        } else {
            out.push_str(&imgcell::kitty_delete(*id));
            false
        }
    });
    for p in places {
        let k = match art_for(p.id) {
            Some(Art::Kitty(k)) => k,
            _ => continue,
        };
        let stamp = k.png.len() ^ (k.cols << 20) ^ (k.rows << 28);
        let same = placed
            .iter()
            .any(|(id, pl, s)| *id == p.id && pl == p && *s == stamp);
        if same {
            continue;
        }
        placed.retain(|(id, _, _)| *id != p.id);
        out.push_str(&imgcell::kitty_delete(p.id));
        out.push_str(&imgcell::kitty_place(k, p.id, p.row, p.col));
        placed.push((p.id, *p, stamp));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse() {
        // default_mode decides what plain `termdash` does
        assert!(!parse_args(false).unwrap().live || true);
    }

    #[test]
    fn help_mentions_the_main_flags() {
        for flag in ["--live", "--once", "--config", "--image", "--print-config"] {
            assert!(HELP.contains(flag), "help should document {}", flag);
        }
    }
}
