//! Laying the dashboard out. Panels are built as lists of styled runs, which
//! makes their size known before anything is drawn and keeps the drawing code
//! the same for one-shot and live mode.

use crate::audio::Levels;
use crate::config::{ColorOpt, Config};
use crate::imgcell::{Art, TextArt};
use crate::player::PlayerState;
use crate::sysinfo::{Dynamic, Static};
use crate::term::{Screen, Style};
use chrono::{Datelike, Local, NaiveDate, Timelike, Utc};
use unicode_width::UnicodeWidthStr;

/// A line of the dashboard: styled runs, left to right.
pub type Row = Vec<(String, Style)>;

fn seg(s: impl Into<String>, st: Style) -> (String, Style) {
    (s.into(), st)
}

fn spaces(n: usize) -> String {
    " ".repeat(n)
}

/// Cut to `w` columns and make the result printable, which for Arabic and the
/// other right-to-left scripts means joining the letters and reordering them.
fn trunc(s: &str, w: usize) -> String {
    crate::text::prepare(s, w)
}

/// Same, then padded with spaces to exactly `w` columns.
fn fit(s: &str, w: usize) -> String {
    crate::text::fit(s, w)
}

/// How the track table is drawn: what goes between the columns, and what the
/// horizontal rules are made of.
pub struct Table {
    pub divider: String,
    pub rule: char,
    pub join: char,
}

impl Table {
    pub fn from_style(style: &str, gap: usize) -> Table {
        match style {
            "ascii" => Table { divider: " | ".into(), rule: '-', join: '+' },
            // No divider: fall back to plain blank columns, but never zero of
            // them, or two full columns would end up touching.
            "none" => Table { divider: " ".repeat(gap.max(1)), rule: '-', join: '-' },
            _ => Table { divider: " │ ".into(), rule: '─', join: '┼' },
        }
    }
    pub fn divider_width(&self) -> usize {
        crate::text::width(&self.divider)
    }
    /// Which column of the divider holds the line itself, if it has one.
    pub fn join_offset(&self) -> Option<usize> {
        self.divider.chars().position(|c| c != ' ')
    }
}

/// Share out the track columns so they, plus the three dividers, always come to
/// no more than the panel width. The title takes up the slack, and gives it
/// back first when there is not enough room.
pub fn fit_track_columns(panel: usize, want: [usize; 4], divider: usize) -> [usize; 4] {
    const MIN: [usize; 4] = [5, 3, 3, 3];
    let overhead = 3 * divider;
    let avail = panel.saturating_sub(overhead);
    let mut cols = want;
    let total: usize = cols.iter().sum();
    if total <= avail {
        cols[2] += avail - total;
        return cols;
    }
    let mut over = total - avail;
    // Give back in order: title, album, artist, time.
    for idx in [2usize, 3, 1, 0] {
        if over == 0 {
            break;
        }
        let take = cols[idx].saturating_sub(MIN[idx]).min(over);
        cols[idx] -= take;
        over -= take;
    }
    // Panel too narrow even for the minimums: keep taking so the row still fits.
    for idx in [2usize, 3, 1, 0] {
        if over == 0 {
            break;
        }
        let take = cols[idx].min(over);
        cols[idx] -= take;
        over -= take;
    }
    cols
}

/// Pick the biggest cover area that looks square on screen.
/// `cell_aspect` is how many columns are as wide as one row is tall; a
/// terminal cell is usually about 2, but it is never exactly 2, and assuming
/// so leaves the picture stretched and cropped down its sides.
pub fn cover_size(panel_cols: usize, avail_rows: usize, max_rows: usize, cell_aspect: f32) -> (usize, usize) {
    let aspect = if cell_aspect.is_finite() && cell_aspect > 0.1 { cell_aspect } else { 2.0 };
    let mut rows = avail_rows.min(max_rows);
    let mut cols = (rows as f32 * aspect).round() as usize;
    if cols > panel_cols {
        cols = panel_cols;
        rows = ((cols as f32 / aspect).round() as usize).min(avail_rows).min(max_rows);
    }
    if rows == 0 || cols == 0 {
        return (0, 0);
    }
    (cols, rows)
}

fn blit(screen: &mut Screen, x: usize, y: usize, row: &Row) {
    let mut cx = x;
    for (s, st) in row {
        cx = screen.put(cx, y, s, *st);
    }
}

// ---------------------------------------------------------------- geometry

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub stack: bool,
    pub left_w: usize,
    pub right_x: usize,
    pub right_y: usize,
    pub right_w: usize,
    pub cover_cols: usize,
    pub cover_rows: usize,
    pub width: usize,
    pub height: usize,
}

pub struct Dash<'a> {
    pub cfg: &'a Config,
    pub stat: &'a Static,
    pub dynamic: &'a Dynamic,
    pub player: &'a PlayerState,
    /// Position in microseconds, interpolated between polls.
    pub position_us: i64,
    pub levels: Levels,
    pub logo: Option<&'a Art>,
    pub cover: Option<&'a Art>,
    /// Live mode: long track fields scroll. One-shot mode holds everything still.
    pub animate: bool,
    /// Seconds the current track has been on screen, which is what the
    /// scrolling is driven from.
    pub scroll_secs: f32,
}

/// Where a kitty image should be put, 1-based like the escape sequence wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub id: u32,
    pub row: usize,
    pub col: usize,
}

pub const LOGO_ID: u32 = 1;
pub const COVER_ID: u32 = 2;

impl<'a> Dash<'a> {
    fn c(&self, c: ColorOpt) -> Style {
        Style::fg(c.0)
    }

    /// "[|||||||     97%]"
    fn bar(&self, pct: u32, width: usize) -> Row {
        let col = &self.cfg.colors;
        let pct = pct.min(100);
        let filled = (pct as usize * width + 50) / 100;
        let warn_at = (col.bar_warn_at as usize * width).div_ceil(100);
        let fill_ch = col.bar_fill_char.chars().next().unwrap_or('|');
        let empty_ch = col.bar_empty_char.chars().next().unwrap_or(' ');
        let mut row: Row = vec![seg("[", self.c(col.bracket))];
        let lo = filled.min(warn_at);
        if lo > 0 {
            row.push(seg(fill_ch.to_string().repeat(lo), self.c(col.bar_fill)));
        }
        if filled > lo {
            row.push(seg(fill_ch.to_string().repeat(filled - lo), self.c(col.bar_warn)));
        }
        if width > filled {
            row.push(seg(empty_ch.to_string().repeat(width - filled), self.c(col.bar_empty)));
        }
        // {:>3} keeps "100%" the same width as " 97%", so the bar never
        // grows by a column when a meter hits the top and shoves the layout.
        row.push(seg(format!("{:>3}%]", pct), self.c(col.bracket)));
        row
    }

    /// One column of the track line, exactly `w` columns wide.
    ///
    /// `gap` columns at the right are always left blank, so a value that fills
    /// its column still cannot run into the next one. If the text does not fit
    /// and we are live, it scrolls past instead of simply being cut off.
    fn field(&self, s: &str, w: usize, gap: usize) -> String {
        let gap = gap.min(w);
        let avail = w - gap;
        let pad = " ".repeat(gap);
        let shown = crate::text::for_display(s);
        let fits = crate::text::width(&shown) <= avail;
        if fits || !self.animate || !self.cfg.player.scroll || avail == 0 {
            return crate::text::fit(s, avail) + &pad;
        }
        // Hold a new track still for a moment so it can be read at a glance.
        let t = self.scroll_secs - self.cfg.player.scroll_delay;
        if t <= 0.0 {
            return crate::text::fit(s, avail) + &pad;
        }
        let offset = (t * self.cfg.player.scroll_speed.max(0.0)) as usize;
        let ring = format!("{}{}", shown, self.cfg.player.scroll_gap);
        crate::text::scroll_window(&ring, avail, offset) + &pad
    }

    /// A horizontal rule across the player panel, with a join wherever a
    /// column divider crosses it.
    fn rule_row(&self, tbl: &Table, cols: &[usize; 4], pw: usize) -> String {
        let mut line: Vec<char> = std::iter::repeat_n(tbl.rule, pw).collect();
        if let Some(off) = tbl.join_offset() {
            let mut x = 0;
            for c in cols.iter().take(3) {
                x += c;
                if x + off < pw {
                    line[x + off] = tbl.join;
                }
                x += tbl.divider_width();
            }
        }
        line.into_iter().collect()
    }

    fn label_of(&self, key: &str) -> String {
        if let Some(v) = self.cfg.info.labels.get(key) {
            return v.clone();
        }
        match key {
            "os" => "OS",
            "host" => "Host",
            "kernel" => "Kernel",
            "board" => "Motherboard",
            "uptime" => "Uptime",
            "packages" => "Packages",
            "shell" => "Shell",
            "resolution" => "Resolution",
            "terminal" => "Terminal",
            "cpu" => "CPU",
            "gpu" => "GPU",
            "memory" => "Memory",
            "swap" => "Swap",
            "disk" => "Disk",
            "loadavg" => "Load",
            "battery" => "Battery",
            "wm" => "Session",
            "de" => "Desktop",
            "locale" => "Locale",
            "user" => "User",
            other => other,
        }
        .to_string()
    }

    fn value_of(&self, key: &str) -> String {
        match key {
            "os" => self.stat.os.clone(),
            "host" => self.stat.host.clone(),
            "kernel" => self.stat.kernel.clone(),
            "board" => self.stat.board.clone(),
            "uptime" => self.dynamic.uptime.clone(),
            "packages" => self.stat.packages.clone(),
            "shell" => self.stat.shell.clone(),
            "resolution" => self.stat.resolution.clone(),
            "terminal" => self.stat.terminal.clone(),
            "cpu" => self.stat.cpu.clone(),
            "gpu" => self.stat.gpu.clone(),
            "memory" => self.dynamic.memory.clone(),
            "swap" => self.dynamic.swap.clone(),
            "disk" => self.dynamic.disk.clone(),
            "loadavg" => self.dynamic.loadavg.clone(),
            "battery" => self.dynamic.battery.clone(),
            "wm" => self.stat.wm.clone(),
            "de" => self.stat.de.clone(),
            "locale" => self.stat.locale.clone(),
            "user" => self.stat.user.clone(),
            _ => String::new(),
        }
    }

    pub fn title(&self) -> String {
        if !self.cfg.general.title.is_empty() {
            self.cfg.general.title.clone()
        } else {
            format!("{}@{}", self.stat.user, self.stat.hostname)
        }
    }

    // ------------------------------------------------------------ left panel

    pub fn left_panel(&self) -> Vec<Row> {
        let lay = &self.cfg.layout;
        let col = &self.cfg.colors;
        let indent = lay.logo_width + lay.logo_gap;
        let info_w = lay.left_width.saturating_sub(indent);
        let mut rows: Vec<Row> = Vec::new();

        let title = trunc(&self.title(), info_w);
        rows.push(vec![seg(spaces(indent), Style::new()), seg(title.clone(), self.c(col.title).bold())]);
        if self.cfg.info.underline_title {
            let n = UnicodeWidthStr::width(title.as_str()).min(info_w);
            rows.push(vec![seg(spaces(indent), Style::new()), seg("-".repeat(n), self.c(col.separator))]);
        }
        for key in &self.cfg.info.rows {
            let label = self.label_of(key);
            let value = self.value_of(key);
            let room = info_w.saturating_sub(UnicodeWidthStr::width(label.as_str()) + 2);
            rows.push(vec![
                seg(spaces(indent), Style::new()),
                seg(format!("{}: ", label), self.c(col.label)),
                seg(trunc(&value, room), self.c(col.value)),
            ]);
        }
        // the logo needs this many rows even if there are fewer specs
        while rows.len() < lay.logo_height {
            rows.push(vec![seg(spaces(indent), Style::new())]);
        }

        if self.cfg.clocks.show && !self.cfg.clocks.entries.is_empty() {
            rows.push(Vec::new());
            for (i, e) in self.cfg.clocks.entries.iter().enumerate() {
                rows.push(self.clock_row(i, &e.label, &e.tz));
            }
        }

        let timer_rows = self.timer_rows();
        if !timer_rows.is_empty() {
            rows.push(vec![seg("-".repeat(lay.left_width), self.c(col.separator))]);
            rows.extend(timer_rows);
        }

        rows.push(Vec::new());
        rows.push(vec![
            seg("IP Address: ", self.c(col.label)),
            seg(self.dynamic.ip.clone(), self.c(col.value)),
        ]);
        rows.push(vec![
            seg("Date: ", self.c(col.label)),
            seg(Local::now().format("%Y-%m-%d").to_string(), self.c(col.value)),
        ]);
        rows
    }

    fn clock_row(&self, idx: usize, label: &str, tz: &str) -> Row {
        let col = &self.cfg.colors;
        let now = Local::now();
        let (date, h, m, s) = if tz == "local" {
            (now.date_naive(), now.hour(), now.minute(), now.second())
        } else {
            match tz.parse::<chrono_tz::Tz>() {
                Ok(z) => {
                    let t = Utc::now().with_timezone(&z);
                    (t.date_naive(), t.hour(), t.minute(), t.second())
                }
                Err(_) => (now.date_naive(), now.hour(), now.minute(), now.second()),
            }
        };
        let today = now.date_naive();
        let day = match date.cmp(&today) {
            std::cmp::Ordering::Equal => "Today",
            std::cmp::Ordering::Greater => "Tomorrow",
            std::cmp::Ordering::Less => "Yesterday",
        };
        let t24 = format!("{:02}:{:02}:{:02}", h, m, s);
        let mut row: Row = vec![
            seg(fit(&format!("Clock {}", idx), 10), self.c(col.label)),
            seg(fit(&format!("[{}] {}", label, day), 17), self.c(col.value)),
            seg(t24, self.c(col.value)),
        ];
        if self.cfg.clocks.show_12h {
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            let ap = if h < 12 { "AM" } else { "PM" };
            row.push(seg(
                format!(" | {}", fit(&format!("{}:{:02}:{:02} {}", h12, m, s, ap), 11)),
                self.c(col.value),
            ));
        }
        if self.cfg.clocks.day_bar {
            let secs = h * 3600 + m * 60 + s;
            let pct = (secs * 100 + 43200) / 86400;
            row.push(seg(" ", Style::new()));
            row.extend(self.bar(pct, self.cfg.layout.clock_bar_width));
        }
        row
    }

    fn timer_rows(&self) -> Vec<Row> {
        let t = &self.cfg.timers;
        if !t.show {
            return Vec::new();
        }
        let col = &self.cfg.colors;
        let today = Local::now().date_naive();
        let mut out: Vec<Row> = Vec::new();
        let mut add = |label: &str, text: String| {
            out.push(vec![
                seg(fit(&format!("Timer {}", out.len()), 10), self.c(col.label)),
                seg(format!("[{}] {}", label, text), self.c(col.value)),
            ]);
        };

        if !t.birthday.is_empty() {
            if let Some(days) = days_to_next_anniversary(&t.birthday, today) {
                add(&t.birthday_label, format!("{} Days remaining", days));
            }
        }
        if t.show_new_year {
            if let Some(ny) = NaiveDate::from_ymd_opt(today.year() + 1, 1, 1) {
                add(&t.new_year_label, format!("{} Days remaining", (ny - today).num_days()));
            }
        }
        if !t.born.is_empty() {
            if let Ok(d) = NaiveDate::parse_from_str(&t.born, "%Y-%m-%d") {
                add(&t.alive_label, format!("{} Days ago", (today - d).num_days()));
            }
        }
        for e in &t.extra {
            if let Ok(d) = NaiveDate::parse_from_str(&e.date, "%Y-%m-%d") {
                let n = (d - today).num_days();
                let text = if n >= 0 {
                    format!("{} Days remaining", n)
                } else {
                    format!("{} Days ago", -n)
                };
                add(&e.label, text);
            }
        }
        out
    }

    // ----------------------------------------------------------- right panel

    pub fn right_panel(&self, cover_rows: usize) -> Vec<Row> {
        let p = &self.cfg.player;
        let lay = &self.cfg.layout;
        let col = &self.cfg.colors;
        let pw = lay.player_width;
        let mut rows: Vec<Row> = Vec::new();

        if self.cfg.audio.show {
            let bw = if self.cfg.audio.bar_width > 0 {
                self.cfg.audio.bar_width
            } else {
                pw.saturating_sub(15)
            };
            let l = self.levels;
            let vals = [l.bass, l.mid, l.treble, l.volume()];
            for (i, v) in vals.iter().enumerate() {
                let label = self.cfg.audio.labels.get(i).cloned().unwrap_or_default();
                let mut row: Row = vec![seg(fit(&format!("{}:", label), 9), self.c(col.label))];
                row.extend(self.bar(*v, bw));
                rows.push(row);
            }
        }

        if !p.show {
            return rows;
        }

        let g = p.column_gap;
        let tbl = Table::from_style(&p.table_style, g);
        let cw = fit_track_columns(
            pw,
            [p.col_time, p.col_artist, p.col_title, p.col_album],
            tbl.divider_width(),
        );
        let sep: Row = vec![seg(self.rule_row(&tbl, &cw, pw), self.c(col.separator))];
        rows.push(sep.clone());
        let heads = ["Time", "Artist", "Title/Filename", "Album"];
        rows.push(vec![seg(
            heads
                .iter()
                .enumerate()
                .map(|(i, h)| fit(h, cw[i]))
                .collect::<Vec<_>>()
                .join(&tbl.divider),
            self.c(col.header),
        )]);
        rows.push(sep.clone());

        let playing = self.player.running && !self.player.title.is_empty();
        let pos = if playing { fmt_time(self.position_us) } else { "--:--".into() };
        let len = if playing { fmt_time(self.player.length_us) } else { "00:00".into() };
        let (artist, title, album) = if playing {
            (self.player.artist.clone(), self.player.title.clone(), self.player.album.clone())
        } else {
            (String::new(), p.nothing_playing.clone(), String::new())
        };
        // The divider keeps the columns apart, so the fields need no gap of
        // their own; the last one has nothing after it either way.
        let cells = [
            self.field(&pos, cw[0], 0),
            self.field(&artist, cw[1], 0),
            self.field(&title, cw[2], 0),
            self.field(&album, cw[3], 0),
        ];
        rows.push(vec![seg(cells.join(&tbl.divider), self.c(col.track))]);

        if p.progress_bar {
            let pct = if playing && self.player.length_us > 0 {
                ((self.position_us.max(0) as i128 * 100) / self.player.length_us as i128) as u32
            } else {
                0
            };
            rows.push(self.bar(pct.min(100), pw.saturating_sub(6)));
        } else {
            rows.push(vec![seg(format!("=>{}", "-".repeat(pw.saturating_sub(2))), self.c(col.separator))]);
        }

        let left = if playing {
            if artist.is_empty() {
                format!("Playing: {}", title)
            } else {
                format!("Playing: {} - {}", artist, title)
            }
        } else {
            "Playing: nothing".to_string()
        };
        let stamp = format!("[{}/{}]", pos, len);
        let room = pw.saturating_sub(UnicodeWidthStr::width(stamp.as_str()));
        rows.push(vec![
            seg(self.field(&left, room, g), self.c(col.value)),
            seg(stamp, self.c(col.label)),
        ]);

        while rows.len() < lay.cover_top {
            rows.push(Vec::new());
        }
        for _ in 0..cover_rows {
            rows.push(Vec::new());
        }
        rows
    }

    // --------------------------------------------------------------- compose

    pub fn geometry(
        &self,
        term_cols: usize,
        term_rows: usize,
        left_rows: usize,
        cell_aspect: f32,
    ) -> Geometry {
        let lay = &self.cfg.layout;
        let side_min = lay.left_width + lay.side_gap + lay.player_width;
        let stack = match lay.mode.as_str() {
            "side" => false,
            "stack" => true,
            _ => term_cols < side_min,
        };
        let (right_x, right_y) = if stack {
            (0, left_rows + lay.stack_gap)
        } else {
            (lay.left_width + lay.side_gap, 0)
        };
        // The cover gets whatever vertical room is left under the player header.
        let avail = term_rows.saturating_sub(right_y + lay.cover_top);
        let aspect = if lay.cover_aspect > 0.0 { lay.cover_aspect } else { cell_aspect };
        let (cover_cols, cover_rows) =
            cover_size(lay.player_width, avail, lay.cover_max_height, aspect);
        Geometry {
            stack,
            left_w: lay.left_width,
            right_x,
            right_y,
            right_w: lay.player_width,
            cover_cols,
            cover_rows,
            width: if stack {
                lay.left_width.max(lay.player_width)
            } else {
                lay.left_width + lay.side_gap + lay.player_width
            },
            height: 0,
        }
    }

    /// Draw both panels. Returns where any kitty images should go.
    pub fn draw(&self, screen: &mut Screen, geo: &Geometry) -> Vec<Placement> {
        let lay = &self.cfg.layout;
        let mut places = Vec::new();

        let left = self.left_panel();
        for (y, row) in left.iter().enumerate() {
            blit(screen, 0, y, row);
        }

        // logo over the top-left corner
        match self.logo {
            Some(Art::Text(t)) => blit_art(screen, 0, 0, t),
            Some(Art::Kitty(_)) => {
                screen.fill_blank(0, 0, lay.logo_width, lay.logo_height);
                places.push(Placement { id: LOGO_ID, row: 1, col: 1 });
            }
            None => {}
        }

        let right = self.right_panel(geo.cover_rows);
        for (y, row) in right.iter().enumerate() {
            blit(screen, geo.right_x, geo.right_y + y, row);
        }

        if self.cfg.player.show_cover {
            let cy = geo.right_y + lay.cover_top;
            match self.cover {
                Some(Art::Text(t)) => blit_art(screen, geo.right_x, cy, t),
                Some(Art::Kitty(k)) => {
                    screen.fill_blank(geo.right_x, cy, k.cols, k.rows);
                    places.push(Placement { id: COVER_ID, row: cy + 1, col: geo.right_x + 1 });
                }
                None => {}
            }
        }
        places
    }

    /// Rows the whole dashboard needs, for one-shot mode.
    pub fn total_rows(&self, geo: &Geometry, cover_rows: usize) -> usize {
        let l = self.left_panel().len();
        let r = self.right_panel(cover_rows).len();
        if geo.stack {
            geo.right_y + r
        } else {
            l.max(r)
        }
    }
}

fn blit_art(screen: &mut Screen, x: usize, y: usize, art: &TextArt) {
    for ry in 0..art.h.min(art.rows.len()) {
        let row = &art.rows[ry];
        for rx in 0..art.w.min(row.len()) {
            let (ch, st) = row[rx];
            screen.set(x + rx, y + ry, ch, st);
        }
    }
}

/// microseconds -> MM:SS, or H:MM:SS past an hour
pub fn fmt_time(us: i64) -> String {
    let s = (us.max(0) / 1_000_000) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// Days until the next MM-DD, counting today as 0.
pub fn days_to_next_anniversary(mmdd: &str, today: NaiveDate) -> Option<i64> {
    let (m, d) = mmdd.split_once('-')?;
    let (m, d) = (m.trim().parse::<u32>().ok()?, d.trim().parse::<u32>().ok()?);
    let this_year = NaiveDate::from_ymd_opt(today.year(), m, d);
    let target = match this_year {
        Some(t) if t >= today => t,
        // 29 February falls back to the next year that has one
        _ => {
            let mut y = today.year() + 1;
            loop {
                if let Some(t) = NaiveDate::from_ymd_opt(y, m, d) {
                    break t;
                }
                y += 1;
                if y > today.year() + 8 {
                    return None;
                }
            }
        }
    };
    Some((target - today).num_days())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Depth;

    fn row_width(r: &Row) -> usize {
        r.iter().map(|(s, _)| UnicodeWidthStr::width(s.as_str())).sum()
    }

    fn dash_parts() -> (Config, Static, Dynamic, PlayerState) {
        let cfg = Config::default();
        let stat = Static {
            user: "circuit".into(),
            hostname: "magneton".into(),
            os: "Fedora Linux 42".into(),
            host: "ThinkPad".into(),
            kernel: "6.18.4".into(),
            board: "LENOVO 20XX".into(),
            shell: "zsh 5.9".into(),
            resolution: "2560x1440".into(),
            terminal: "kitty".into(),
            cpu: "AMD Ryzen 7 5800X".into(),
            gpu: "AMD Radeon".into(),
            wm: "GNOME (wayland)".into(),
            de: "GNOME".into(),
            locale: "en_US.UTF-8".into(),
            packages: "2104 (rpm)".into(),
        };
        let dynamic = Dynamic {
            uptime: "2 hours 1 mins 5 secs".into(),
            memory: "8.00 GiB / 32.00 GiB (25%)".into(),
            swap: "None".into(),
            disk: "200 GiB / 900 GiB (22%)".into(),
            loadavg: "0.4 0.3 0.2".into(),
            battery: "No battery".into(),
            ip: "192.168.1.5".into(),
        };
        (cfg, stat, dynamic, PlayerState::default())
    }

    fn dash<'a>(
        cfg: &'a Config,
        stat: &'a Static,
        dynamic: &'a Dynamic,
        player: &'a PlayerState,
    ) -> Dash<'a> {
        Dash {
            cfg,
            stat,
            dynamic,
            player,
            position_us: 0,
            levels: Levels::default(),
            logo: None,
            cover: None,
            animate: false,
            scroll_secs: 0.0,
        }
    }

    /// A row of the player panel, as plain text with the colours stripped.
    fn panel_row(d: &Dash, n: usize) -> String {
        d.right_panel(4)[n].iter().map(|(s, _)| s.clone()).collect()
    }

    /// The track line (audio rows 0-3, rule, header, rule, then the track).
    fn track_line(d: &Dash) -> String {
        panel_row(d, 7)
    }

    /// Column boundaries of the track table, as the panel is currently set up.
    fn track_columns(cfg: &Config) -> ([usize; 4], Table) {
        let p = &cfg.player;
        let tbl = Table::from_style(&p.table_style, p.column_gap);
        let cw = fit_track_columns(
            cfg.layout.player_width,
            [p.col_time, p.col_artist, p.col_title, p.col_album],
            tbl.divider_width(),
        );
        (cw, tbl)
    }

    fn long_track() -> PlayerState {
        PlayerState {
            running: true,
            artist: "Godspeed You! Black Emperor".into(),
            title: "Sleep / Monheim / Broken Windows, Locks of Love Pt. III".into(),
            album: "Lift Your Skinny Fists Like Antennas to Heaven".into(),
            length_us: 1_200_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn long_fields_never_touch_the_divider() {
        let (cfg, stat, dynamic, _) = dash_parts();
        let player = long_track();
        let mut d = dash(&cfg, &stat, &dynamic, &player);
        let (cw, tbl) = track_columns(&cfg);
        // both still (one-shot) and part way through a scroll
        for (animate, secs) in [(false, 0.0), (true, 0.0), (true, 3.0), (true, 11.5), (true, 40.0)] {
            d.animate = animate;
            d.scroll_secs = secs;
            let line = track_line(&d);
            let chars: Vec<char> = line.chars().collect();
            let mut x = 0;
            for c in cw.iter().take(3) {
                x += c;
                let div: String = chars[x..x + tbl.divider_width()].iter().collect();
                assert_eq!(
                    div, tbl.divider,
                    "divider missing or overwritten at column {} (animate={}, secs={}): {:?}",
                    x, animate, secs, line
                );
                x += tbl.divider_width();
            }
            assert_eq!(
                UnicodeWidthStr::width(line.as_str()),
                cfg.layout.player_width,
                "track line should fill the panel exactly: {:?}",
                line
            );
        }
    }

    #[test]
    fn the_table_has_dividers_and_the_rules_join_them() {
        let (cfg, stat, dynamic, _) = dash_parts();
        let player = long_track();
        let d = dash(&cfg, &stat, &dynamic, &player);
        let (cw, tbl) = track_columns(&cfg);
        let header = panel_row(&d, 5);
        let rule = panel_row(&d, 6);
        assert!(header.contains('│'), "header should carry dividers: {:?}", header);
        assert_eq!(header.matches('│').count(), 3);
        assert_eq!(rule.matches('┼').count(), 3, "rule should join each divider: {:?}", rule);

        // every join must sit in the same column as its divider
        let hchars: Vec<char> = header.chars().collect();
        let rchars: Vec<char> = rule.chars().collect();
        let off = tbl.join_offset().unwrap();
        let mut x = 0;
        for c in cw.iter().take(3) {
            x += c;
            assert_eq!(hchars[x + off], '│');
            assert_eq!(rchars[x + off], '┼');
            x += tbl.divider_width();
        }
    }

    #[test]
    fn the_table_style_can_be_switched_off() {
        let (mut cfg, stat, dynamic, _) = dash_parts();
        cfg.player.table_style = "none".into();
        let player = long_track();
        let d = dash(&cfg, &stat, &dynamic, &player);
        let line = track_line(&d);
        assert!(!line.contains('│'), "no dividers were asked for: {:?}", line);
        // but the columns must still be kept apart by a blank
        let (cw, _) = track_columns(&cfg);
        let chars: Vec<char> = line.chars().collect();
        let mut x = 0;
        for c in cw.iter().take(3) {
            x += c;
            assert_eq!(chars[x], ' ', "columns touching at {}: {:?}", x, line);
            x += 1;
        }

        cfg.player.table_style = "ascii".into();
        let d = dash(&cfg, &stat, &dynamic, &player);
        assert!(track_line(&d).contains(" | "));
    }

    #[test]
    fn the_track_table_always_fits_the_panel() {
        // Whatever widths are asked for, the columns plus the dividers must
        // never be wider than the panel.
        for panel in 10..=160usize {
            for want in [[7, 16, 25, 12], [40, 40, 40, 40], [1, 1, 1, 1], [5, 3, 3, 3]] {
                for div in [0usize, 1, 3] {
                    let cols = fit_track_columns(panel, want, div);
                    let total: usize = cols.iter().sum::<usize>() + 3 * div;
                    assert!(
                        total <= panel,
                        "panel {} want {:?} div {} -> {:?} = {}",
                        panel, want, div, cols, total
                    );
                }
            }
        }
        // with room to spare, the slack goes to the title
        let cols = fit_track_columns(60, [7, 16, 25, 12], 3);
        assert_eq!(cols.iter().sum::<usize>(), 51);
        assert_eq!(cols[0], 7);
        assert_eq!(cols[1], 16);
        assert_eq!(cols[3], 12);
    }

    #[test]
    fn long_fields_scroll_in_live_mode() {
        let (cfg, stat, dynamic, _) = dash_parts();
        let player = long_track();
        let mut d = dash(&cfg, &stat, &dynamic, &player);
        d.animate = true;

        d.scroll_secs = 0.0;
        let held = track_line(&d);
        d.scroll_secs = cfg.player.scroll_delay - 0.1;
        assert_eq!(track_line(&d), held, "a new track should sit still at first");

        d.scroll_secs = cfg.player.scroll_delay + 1.0;
        let moved = track_line(&d);
        assert_ne!(moved, held, "it should be scrolling by now");

        // it keeps moving, and eventually comes back round to the start
        d.scroll_secs = cfg.player.scroll_delay + 2.0;
        assert_ne!(track_line(&d), moved);
        let ring_cols = crate::text::width(&player.title) + crate::text::width(&cfg.player.scroll_gap);
        d.scroll_secs = cfg.player.scroll_delay + ring_cols as f32 / cfg.player.scroll_speed;
        let (cw, tbl) = track_columns(&cfg);
        let start = cw[0] + cw[1] + 2 * tbl.divider_width();
        let title_at = |s: &str| s.chars().skip(start).take(cw[2]).collect::<String>();
        assert_eq!(
            title_at(&track_line(&d)),
            title_at(&held),
            "after one full turn the title should be back where it started"
        );
    }

    #[test]
    fn short_fields_are_left_alone_by_the_scrolling() {
        let (cfg, stat, dynamic, _) = dash_parts();
        let player = PlayerState {
            running: true,
            artist: "Boards".into(),
            title: "Roygbiv".into(),
            album: "Music".into(),
            ..Default::default()
        };
        let mut d = dash(&cfg, &stat, &dynamic, &player);
        d.animate = true;
        let a = track_line(&d);
        d.scroll_secs = 60.0;
        assert_eq!(track_line(&d), a, "text that fits must never move");
        assert!(a.contains("Roygbiv"));
    }

    #[test]
    fn one_shot_mode_never_scrolls() {
        let (cfg, stat, dynamic, _) = dash_parts();
        let player = long_track();
        let mut d = dash(&cfg, &stat, &dynamic, &player);
        d.animate = false;
        let a = track_line(&d);
        d.scroll_secs = 500.0;
        assert_eq!(track_line(&d), a);
        // and it shows the beginning of each field, not some window into it
        assert!(a.contains("Godspeed You!"), "got {:?}", a);
    }

    #[test]
    fn panels_fit_their_configured_width() {
        let (cfg, stat, dynamic, player) = dash_parts();
        let d = dash(&cfg, &stat, &dynamic, &player);
        for row in d.left_panel() {
            assert!(row_width(&row) <= cfg.layout.left_width, "left row too wide: {}", row_width(&row));
        }
        for row in d.right_panel(10) {
            assert!(row_width(&row) <= cfg.layout.player_width, "right row too wide: {}", row_width(&row));
        }
    }

    #[test]
    fn specs_line_up_in_one_column() {
        let (cfg, stat, dynamic, player) = dash_parts();
        let d = dash(&cfg, &stat, &dynamic, &player);
        let indent = cfg.layout.logo_width + cfg.layout.logo_gap;
        for row in d.left_panel().iter().take(2 + cfg.info.rows.len()) {
            assert_eq!(UnicodeWidthStr::width(row[0].0.as_str()), indent);
        }
    }

    #[test]
    fn layout_switches_on_width() {
        let (cfg, stat, dynamic, player) = dash_parts();
        let d = dash(&cfg, &stat, &dynamic, &player);
        assert!(!d.geometry(200, 50, 30, 2.0).stack, "wide window should be side by side");
        assert!(d.geometry(100, 50, 30, 2.0).stack, "narrow window should stack");
    }

    #[test]
    fn cover_area_is_square_on_screen() {
        // For each plausible character cell shape, the chosen area must come
        // out square in pixels, not just in cells.
        for (cw, ch) in [(10, 20), (8, 19), (9, 18), (11, 24), (7, 15), (10, 21)] {
            let aspect = ch as f32 / cw as f32;
            let (cols, rows) = cover_size(60, 34, 24, aspect);
            assert!(cols <= 60 && rows <= 24, "{}x{} overflowed", cols, rows);
            let (pw, ph) = (cols * cw, rows * ch);
            let ratio = pw as f32 / ph as f32;
            assert!(
                (ratio - 1.0).abs() < 0.05,
                "cell {}x{}: area {}x{} cells is {}x{} px, ratio {:.3}",
                cw, ch, cols, rows, pw, ph, ratio
            );
        }
    }

    #[test]
    fn cover_area_fits_a_narrow_panel() {
        // A tall thin panel must give up rows rather than run past its width.
        let (cols, rows) = cover_size(20, 40, 24, 2.2);
        assert!(cols <= 20, "cols {} exceeded the panel", cols);
        assert!(rows <= 24);
        let ratio = (cols as f32) / (rows as f32 * 2.2);
        assert!((ratio - 1.0).abs() < 0.1, "got {}x{}", cols, rows);
        assert_eq!(cover_size(60, 0, 24, 2.0), (0, 0));
    }

    #[test]
    fn drawing_stays_inside_the_screen() {
        let (cfg, stat, dynamic, player) = dash_parts();
        let d = dash(&cfg, &stat, &dynamic, &player);
        let geo = d.geometry(142, 40, d.left_panel().len(), 2.0);
        let mut s = Screen::new(142, 40, Depth::True);
        let places = d.draw(&mut s, &geo);
        assert!(places.is_empty(), "no images configured, so nothing to place");
        let lines = s.to_lines();
        assert_eq!(lines.len(), 40);
        assert!(lines.iter().any(|l| l.contains("circuit@magneton")));
        assert!(lines.iter().any(|l| l.contains("Nothing playing")));
    }

    #[test]
    fn bars_are_a_fixed_width() {
        let (cfg, stat, dynamic, player) = dash_parts();
        let d = dash(&cfg, &stat, &dynamic, &player);
        for pct in [0, 1, 50, 80, 99, 100, 250] {
            assert_eq!(row_width(&d.bar(pct, 24)), 24 + 6, "pct {}", pct);
        }
    }

    #[test]
    fn times_format() {
        assert_eq!(fmt_time(0), "00:00");
        assert_eq!(fmt_time(65_000_000), "01:05");
        assert_eq!(fmt_time(3_725_000_000), "1:02:05");
        assert_eq!(fmt_time(-5), "00:00");
    }

    #[test]
    fn anniversaries_count_forward() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 20).unwrap();
        assert_eq!(days_to_next_anniversary("03-25", today), Some(5));
        assert_eq!(days_to_next_anniversary("03-20", today), Some(0));
        // already gone this year, so it rolls to next year
        assert_eq!(days_to_next_anniversary("03-19", today), Some(364));
        // 29 February skips to the next leap year, 2028
        let t = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(days_to_next_anniversary("02-29", t), Some(730));
        assert_eq!(days_to_next_anniversary("nonsense", today), None);
    }

    #[test]
    fn arabic_track_is_joined_reordered_and_still_fits() {
        let (cfg, stat, dynamic, mut player) = dash_parts();
        player.running = true;
        // "مرحبا بالعالم"
        player.title = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627} \u{0628}\u{0627}\u{0644}\u{0639}\u{0627}\u{0644}\u{0645}".into();
        player.artist = "\u{0641}\u{064A}\u{0631}\u{0648}\u{0632}".into();
        let d = dash(&cfg, &stat, &dynamic, &player);
        let rows = d.right_panel(4);
        for row in &rows {
            assert!(row_width(row) <= cfg.layout.player_width);
        }
        let joined: String = rows.iter().flat_map(|r| r.iter().map(|(s, _)| s.clone())).collect();
        // the raw isolated letters must not reach the screen
        assert!(!joined.contains('\u{0645}'), "unjoined meem reached the screen");
        assert!(
            joined.chars().any(|c| (0xFE70..=0xFEFF).contains(&(c as u32))),
            "expected Arabic presentation forms in the output"
        );
    }

    #[test]
    fn unicode_titles_do_not_overflow() {
        let (cfg, stat, dynamic, mut player) = dash_parts();
        player.running = true;
        player.title = "東京は夜の七時ネオンが光る街角で".into();
        player.artist = "ピチカート・ファイヴ".into();
        let d = dash(&cfg, &stat, &dynamic, &player);
        for row in d.right_panel(4) {
            assert!(row_width(&row) <= cfg.layout.player_width);
        }
    }
}
