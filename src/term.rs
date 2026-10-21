//! Terminal plumbing: colours, a diffing cell buffer, raw mode, size and signals.
//!
//! Everything is drawn into a grid of cells. On each frame only the cells that
//! actually changed are written to the terminal, which is what keeps the live
//! mode from flickering.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use unicode_width::UnicodeWidthChar;

// ---------------------------------------------------------------- colour

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// How many colours the terminal can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    True,
    C256,
    Mono,
}

impl Depth {
    /// Work it out from the environment. kitty sets COLORTERM=truecolor.
    pub fn detect() -> Depth {
        let ct = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
        if ct.contains("truecolor") || ct.contains("24bit") {
            return Depth::True;
        }
        let term = std::env::var("TERM").unwrap_or_default();
        if term.is_empty() || term == "dumb" {
            return Depth::Mono;
        }
        if term.contains("kitty") || term.contains("direct") || term.contains("24bit") {
            return Depth::True;
        }
        Depth::C256
    }
}

const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn to_256(c: Rgb) -> u8 {
    let Rgb(r, g, b) = c;
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    if mx - mn < 10 {
        let v = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        if v < 6 {
            return 16;
        }
        if v > 246 {
            return 231;
        }
        return 232 + ((v as u16 - 6) * 24 / 241) as u8;
    }
    let idx = |v: u8| -> u8 {
        let mut best = 0usize;
        let mut bd = 255i32;
        for (i, l) in LEVELS.iter().enumerate() {
            let d = (*l as i32 - v as i32).abs();
            if d < bd {
                bd = d;
                best = i;
            }
        }
        best as u8
    };
    16 + 36 * idx(r) + 6 * idx(g) + idx(b)
}

fn push_color(out: &mut String, c: Rgb, depth: Depth, background: bool) {
    let lead = if background { 48 } else { 38 };
    match depth {
        Depth::True => {
            out.push_str(&format!(";{};2;{};{};{}", lead, c.0, c.1, c.2));
        }
        Depth::C256 => {
            out.push_str(&format!(";{};5;{}", lead, to_256(c)));
        }
        Depth::Mono => {}
    }
}

// ---------------------------------------------------------------- cells

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
}

impl Style {
    pub fn new() -> Style {
        Style::default()
    }
    pub fn fg(c: Option<Rgb>) -> Style {
        Style { fg: c, bg: None, bold: false }
    }
    pub fn bold(mut self) -> Style {
        self.bold = true;
        self
    }
    fn sgr(&self, depth: Depth) -> String {
        let mut s = String::from("\x1b[0");
        if self.bold {
            s.push_str(";1");
        }
        if let Some(c) = self.fg {
            push_color(&mut s, c, depth, false);
        }
        if let Some(c) = self.bg {
            push_color(&mut s, c, depth, true);
        }
        s.push('m');
        s
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub st: Style,
    /// True for the right-hand half of a double-width character.
    pub cont: bool,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell { ch: ' ', st: Style::default(), cont: false }
    }
}

/// A grid of cells that knows what it looked like last frame.
pub struct Screen {
    pub w: usize,
    pub h: usize,
    pub depth: Depth,
    cur: Vec<Cell>,
    prev: Vec<Cell>,
    force: bool,
}

impl Screen {
    pub fn new(w: usize, h: usize, depth: Depth) -> Screen {
        Screen {
            w,
            h,
            depth,
            cur: vec![Cell::default(); w * h],
            prev: vec![Cell::default(); w * h],
            force: true,
        }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.cur = vec![Cell::default(); w * h];
        self.prev = vec![Cell::default(); w * h];
        self.force = true;
    }

    /// Next flush writes every cell, not just the changed ones.
    pub fn force_repaint(&mut self) {
        self.force = true;
    }

    pub fn clear(&mut self) {
        for c in self.cur.iter_mut() {
            *c = Cell::default();
        }
    }

    pub fn set(&mut self, x: usize, y: usize, ch: char, st: Style) {
        if x >= self.w || y >= self.h {
            return;
        }
        self.cur[y * self.w + x] = Cell { ch, st, cont: false };
    }

    /// Draw `s` at (x, y). Returns the column just past the text.
    /// Stops at the right edge; double-width characters claim two cells.
    pub fn put(&mut self, x: usize, y: usize, s: &str, st: Style) -> usize {
        if y >= self.h {
            return x;
        }
        let mut cx = x;
        for ch in s.chars() {
            if ch == '\n' {
                break;
            }
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if w == 0 {
                continue;
            }
            if cx + w > self.w {
                break;
            }
            self.cur[y * self.w + cx] = Cell { ch, st, cont: false };
            if w == 2 {
                self.cur[y * self.w + cx + 1] = Cell { ch: ' ', st, cont: true };
            }
            cx += w;
        }
        cx
    }

    /// Blank a rectangle (used to keep image areas free of text).
    pub fn fill_blank(&mut self, x: usize, y: usize, w: usize, h: usize) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.cur[yy * self.w + xx] = Cell::default();
            }
        }
    }

    /// Append the escape sequences needed to turn the previous frame into this one.
    pub fn flush_diff(&mut self, out: &mut String) {
        let mut last_style: Option<Style> = None;
        let mut cursor: Option<(usize, usize)> = None;
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let c = self.cur[i];
                if c.cont {
                    continue;
                }
                let w = UnicodeWidthChar::width(c.ch).unwrap_or(1).max(1);
                let mut changed = self.force || self.prev[i] != c;
                if !changed && w == 2 && i + 1 < self.cur.len() {
                    changed = self.prev[i + 1] != self.cur[i + 1];
                }
                if !changed {
                    continue;
                }
                if cursor != Some((x, y)) {
                    out.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                }
                if last_style != Some(c.st) {
                    out.push_str(&c.st.sgr(self.depth));
                    last_style = Some(c.st);
                }
                out.push(c.ch);
                cursor = Some((x + w, y));
            }
        }
        if last_style.is_some() {
            out.push_str("\x1b[0m");
        }
        self.prev.copy_from_slice(&self.cur);
        self.force = false;
    }

    /// Render the whole buffer as plain lines, for one-shot mode.
    pub fn to_lines(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let mut line = String::new();
            let mut last: Option<Style> = None;
            let mut trailing = String::new();
            for x in 0..self.w {
                let c = self.cur[y * self.w + x];
                if c.cont {
                    continue;
                }
                // Hold back trailing blanks so lines do not carry useless spaces.
                if c.ch == ' ' && c.st.bg.is_none() {
                    trailing.push(' ');
                    continue;
                }
                if !trailing.is_empty() {
                    // Those spaces must not inherit a background colour from
                    // whatever was drawn before them.
                    if last.map(|s: Style| s.bg.is_some()).unwrap_or(false) {
                        line.push_str("\x1b[0m");
                        last = Some(Style::default());
                    }
                    line.push_str(&trailing);
                    trailing.clear();
                }
                if last != Some(c.st) {
                    line.push_str(&c.st.sgr(self.depth));
                    last = Some(c.st);
                }
                line.push(c.ch);
            }
            if last.is_some() {
                line.push_str("\x1b[0m");
            }
            lines.push(line);
        }
        lines
    }
}

// ---------------------------------------------------------------- size

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
    /// Size of one character cell in pixels; 0 when the terminal does not say.
    pub cell_w: usize,
    pub cell_h: usize,
}

impl TermSize {
    pub fn get() -> TermSize {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
        if !ok || ws.ws_col == 0 {
            return TermSize { cols: 80, rows: 24, cell_w: 0, cell_h: 0 };
        }
        let cols = ws.ws_col as usize;
        let rows = ws.ws_row.max(1) as usize;
        TermSize {
            cols,
            rows,
            cell_w: if ws.ws_xpixel > 0 { ws.ws_xpixel as usize / cols } else { 0 },
            cell_h: if ws.ws_ypixel > 0 { ws.ws_ypixel as usize / rows } else { 0 },
        }
    }
}

// ---------------------------------------------------------------- signals

pub static RESIZED: AtomicBool = AtomicBool::new(false);
pub static QUIT: AtomicBool = AtomicBool::new(false);
static SUSPEND: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_winch(_: libc::c_int) {
    RESIZED.store(true, Ordering::SeqCst);
}
extern "C" fn on_quit(_: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
}
extern "C" fn on_cont(_: libc::c_int) {
    SUSPEND.store(1, Ordering::SeqCst);
}

pub fn install_signal_handlers() {
    type Handler = extern "C" fn(libc::c_int);
    unsafe {
        libc::signal(libc::SIGWINCH, on_winch as Handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_quit as Handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_quit as Handler as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_quit as Handler as libc::sighandler_t);
        libc::signal(libc::SIGCONT, on_cont as Handler as libc::sighandler_t);
    }
}

/// True once if the process was resumed after Ctrl+Z.
pub fn took_resume() -> bool {
    SUSPEND.swap(0, Ordering::SeqCst) == 1
}

// ---------------------------------------------------------------- raw mode

/// Kept so the panic hook can put the terminal back even while unwinding.
static SAVED_TERMIOS: std::sync::Mutex<Option<libc::termios>> = std::sync::Mutex::new(None);
static IN_ALT_SCREEN: AtomicBool = AtomicBool::new(false);

/// Undo everything we did to the terminal. Safe to call more than once.
pub fn emergency_restore() {
    if let Ok(mut g) = SAVED_TERMIOS.lock() {
        if let Some(t) = g.take() {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
    if IN_ALT_SCREEN.swap(false, Ordering::SeqCst) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    }
}

/// True when this terminal understands the kitty graphics protocol.
pub fn supports_kitty_graphics() -> bool {
    if unsafe { libc::isatty(libc::STDOUT_FILENO) } != 1 {
        return false;
    }
    for v in ["KITTY_WINDOW_ID", "GHOSTTY_RESOURCES_DIR", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"] {
        if std::env::var(v).map(|s| !s.is_empty()).unwrap_or(false) {
            return true;
        }
    }
    let term = std::env::var("TERM").unwrap_or_default();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    term.contains("kitty")
        || term.contains("ghostty")
        || prog.contains("kitty")
        || prog.contains("ghostty")
        || prog.contains("wezterm")
}

/// Puts the terminal in raw mode on the alternate screen and puts it
/// back on drop, including when the program panics.
pub struct TerminalGuard {
    saved: Option<libc::termios>,
    alt: bool,
}

impl TerminalGuard {
    pub fn enter(alt: bool) -> TerminalGuard {
        let mut saved: Option<libc::termios> = None;
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
                saved = Some(t);
                if let Ok(mut g) = SAVED_TERMIOS.lock() {
                    *g = Some(t);
                }
                let mut raw = t;
                raw.c_lflag &= !(libc::ICANON | libc::ECHO);
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 0;
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw);
            }
        }
        let mut out = std::io::stdout();
        if alt {
            // alternate screen, hide the cursor
            IN_ALT_SCREEN.store(true, Ordering::SeqCst);
            let _ = out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J");
            let _ = out.flush();
        }
        TerminalGuard { saved, alt }
    }

    pub fn restore(&mut self) {
        self.saved = None;
        self.alt = false;
        emergency_restore();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Read whatever the user typed without blocking.
pub fn read_keys(buf: &mut [u8]) -> usize {
    let mut stdin = std::io::stdin();
    let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
    unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    let n = stdin.read(buf).unwrap_or(0);
    unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags) };
    n
}
