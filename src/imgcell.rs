//! Turning a picture into something a terminal can show.
//!
//! Four ways out:
//!   kitty   - a real image, sent with the kitty graphics protocol
//!   blocks  - half blocks, two true-colour pixels per cell
//!   braille - 2x4 dithered dots per cell, one colour per cell
//!   ascii   - characters chosen by brightness, with / \ | - on edges

use crate::config::ImageCfg;
use crate::term::{Rgb, Style};
use base64::Engine;
use image::{DynamicImage, GenericImageView, RgbImage};
use std::io::Cursor;
use std::path::Path;

/// dark -> bright, 70 steps
pub const DEFAULT_RAMP: &str =
    " .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

const BRAILLE_DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
const SHADES: [char; 5] = [' ', '░', '▒', '▓', '█'];
const UPPER_HALF: char = '▀';

/// One cell of a text-mode picture.
pub type ArtCell = (char, Style);

#[derive(Clone, Debug)]
pub struct TextArt {
    pub rows: Vec<Vec<ArtCell>>,
    pub w: usize,
    pub h: usize,
}

#[derive(Clone)]
pub struct KittyArt {
    pub png: Vec<u8>,
    pub cols: usize,
    pub rows: usize,
}

#[derive(Clone)]
pub enum Art {
    Kitty(KittyArt),
    Text(TextArt),
}

// ---------------------------------------------------------------- loading

/// Turn a file:// URL (possibly percent-encoded) into a path.
pub fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

pub fn load_path(path: &Path) -> Result<DynamicImage, String> {
    image::ImageReader::open(path)
        .map_err(|e| format!("{}: {}", path.display(), e))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {}", path.display(), e))?
        .decode()
        .map_err(|e| format!("{}: {}", path.display(), e))
}

pub fn load_bytes(data: &[u8]) -> Result<DynamicImage, String> {
    image::load_from_memory(data).map_err(|e| e.to_string())
}

/// Find the logo: an explicit path, or the first ~/.config/termdash/logo.*
pub fn find_logo(cfg: &ImageCfg, dir: &Path) -> Option<std::path::PathBuf> {
    if !cfg.logo_path.is_empty() {
        let p = shellexpand(&cfg.logo_path);
        return if p.exists() { Some(p) } else { None };
    }
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_stem().map(|s| s == "logo").unwrap_or(false)
                && p.extension()
                    .map(|e| {
                        let e = e.to_string_lossy().to_lowercase();
                        matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "ico")
                    })
                    .unwrap_or(false)
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

pub fn shellexpand(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(p)
}

// ---------------------------------------------------------------- scaling

fn flatten_on_black(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        out.put_pixel(
            x,
            y,
            image::Rgb([
                (p[0] as u32 * a / 255) as u8,
                (p[1] as u32 * a / 255) as u8,
                (p[2] as u32 * a / 255) as u8,
            ]),
        );
    }
    out
}

/// Scale to exactly pw x ph pixels, cropping or padding as asked.
fn fit_to(img: &DynamicImage, pw: u32, ph: u32, contain: bool) -> RgbImage {
    let filter = image::imageops::FilterType::Lanczos3;
    if contain {
        let (iw, ih) = img.dimensions();
        if iw == 0 || ih == 0 {
            return RgbImage::new(pw.max(1), ph.max(1));
        }
        let s = (pw as f32 / iw as f32).min(ph as f32 / ih as f32);
        let nw = ((iw as f32 * s).round() as u32).max(1);
        let nh = ((ih as f32 * s).round() as u32).max(1);
        let small = flatten_on_black(&img.resize_exact(nw, nh, filter));
        let mut canvas = RgbImage::new(pw, ph);
        image::imageops::replace(&mut canvas, &small, ((pw - nw) / 2) as i64, ((ph - nh) / 2) as i64);
        canvas
    } else {
        flatten_on_black(&img.resize_to_fill(pw, ph, filter))
    }
}

// ---------------------------------------------------------------- tone

fn apply_tone(img: &mut RgbImage, cfg: &ImageCfg) {
    if cfg.contrast > 0.0 {
        let mut hist = [0u32; 256];
        for p in img.pixels() {
            let l = luma(p[0], p[1], p[2]);
            hist[l as usize] += 1;
        }
        let total: u32 = hist.iter().sum();
        let cut = (total as f32 * cfg.contrast / 100.0) as u32;
        let mut lo = 0usize;
        let mut acc = 0u32;
        for (i, c) in hist.iter().enumerate() {
            acc += c;
            if acc > cut {
                lo = i;
                break;
            }
        }
        let mut hi = 255usize;
        acc = 0;
        for i in (0..256).rev() {
            acc += hist[i];
            if acc > cut {
                hi = i;
                break;
            }
        }
        if hi > lo + 16 {
            let mut lut = [0u8; 256];
            for (i, v) in lut.iter_mut().enumerate() {
                let t = (i as i32 - lo as i32) * 255 / (hi as i32 - lo as i32);
                *v = t.clamp(0, 255) as u8;
            }
            for p in img.pixels_mut() {
                p[0] = lut[p[0] as usize];
                p[1] = lut[p[1] as usize];
                p[2] = lut[p[2] as usize];
            }
        }
    }

    if (cfg.gamma - 1.0).abs() > 0.001 {
        let mut lut = [0u8; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (255.0 * (i as f32 / 255.0).powf(1.0 / cfg.gamma)).round().clamp(0.0, 255.0) as u8;
        }
        for p in img.pixels_mut() {
            p[0] = lut[p[0] as usize];
            p[1] = lut[p[1] as usize];
            p[2] = lut[p[2] as usize];
        }
    }

    if (cfg.saturation - 1.0).abs() > 0.001 {
        let s = cfg.saturation;
        for p in img.pixels_mut() {
            let l = luma(p[0], p[1], p[2]) as f32;
            for i in 0..3 {
                p[i] = (l + (p[i] as f32 - l) * s).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    if cfg.sharpen > 0 {
        let amount = cfg.sharpen as f32 / 100.0;
        let blurred = image::imageops::blur(img, 1.2);
        for (p, b) in img.pixels_mut().zip(blurred.pixels()) {
            for i in 0..3 {
                let v = p[i] as f32 + (p[i] as f32 - b[i] as f32) * amount;
                p[i] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[inline]
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
}

/// Push a colour towards full brightness while keeping its hue.
fn lift(c: Rgb, amount: f32) -> Rgb {
    let m = c.0.max(c.1).max(c.2);
    if m == 0 {
        return c;
    }
    let f = (1.0 + (255.0 / m as f32 - 1.0) * amount).min(3.5);
    Rgb(
        (c.0 as f32 * f).min(255.0) as u8,
        (c.1 as f32 * f).min(255.0) as u8,
        (c.2 as f32 * f).min(255.0) as u8,
    )
}

// ---------------------------------------------------------------- modes

fn render_blocks(img: &RgbImage, w: usize, h: usize, tint: Option<Rgb>) -> TextArt {
    let mut rows = Vec::with_capacity(h);
    for cy in 0..h {
        let mut row = Vec::with_capacity(w);
        for cx in 0..w {
            let top = img.get_pixel(cx as u32, (cy * 2) as u32);
            let bot = img.get_pixel(cx as u32, (cy * 2 + 1) as u32);
            if let Some(t) = tint {
                // Single colour: use shade characters so there is still detail.
                let v = (luma(top[0], top[1], top[2]) as u16 + luma(bot[0], bot[1], bot[2]) as u16) / 2;
                let idx = (v as usize * (SHADES.len() - 1) + 127) / 255;
                let ch = SHADES[idx];
                row.push((ch, if ch == ' ' { Style::new() } else { Style::fg(Some(t)) }));
            } else {
                row.push((
                    UPPER_HALF,
                    Style {
                        fg: Some(Rgb(top[0], top[1], top[2])),
                        bg: Some(Rgb(bot[0], bot[1], bot[2])),
                        bold: false,
                    },
                ));
            }
        }
        rows.push(row);
    }
    TextArt { rows, w, h }
}

/// Floyd-Steinberg down to one bit, so braille dots keep shading.
fn dither_1bit(img: &RgbImage, w: usize, h: usize, enabled: bool) -> Vec<bool> {
    let mut buf: Vec<f32> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32);
            buf.push(luma(p[0], p[1], p[2]) as f32);
        }
    }
    let mut out = vec![false; w * h];
    if !enabled {
        for i in 0..w * h {
            out[i] = buf[i] >= 128.0;
        }
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let old = buf[i];
            let new = if old >= 128.0 { 255.0 } else { 0.0 };
            out[i] = new > 0.0;
            let err = old - new;
            let mut spread = |xx: isize, yy: isize, f: f32| {
                if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                    return;
                }
                buf[yy as usize * w + xx as usize] += err * f;
            };
            spread(x as isize + 1, y as isize, 7.0 / 16.0);
            spread(x as isize - 1, y as isize + 1, 3.0 / 16.0);
            spread(x as isize, y as isize + 1, 5.0 / 16.0);
            spread(x as isize + 1, y as isize + 1, 1.0 / 16.0);
        }
    }
    out
}

fn render_braille(img: &RgbImage, w: usize, h: usize, cfg: &ImageCfg, tint: Option<Rgb>) -> TextArt {
    let (pw, ph) = (w * 2, h * 4);
    let on = dither_1bit(img, pw, ph, cfg.dither);
    let mut rows = Vec::with_capacity(h);
    for cy in 0..h {
        let mut row = Vec::with_capacity(w);
        for cx in 0..w {
            let mut bits = 0u8;
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for i in 0..2 {
                for j in 0..4 {
                    let (x, y) = (cx * 2 + i, cy * 4 + j);
                    if on[y * pw + x] {
                        bits |= BRAILLE_DOTS[i][j];
                        let p = img.get_pixel(x as u32, y as u32);
                        r += p[0] as u32;
                        g += p[1] as u32;
                        b += p[2] as u32;
                        n += 1;
                    }
                }
            }
            if bits == 0 {
                row.push((' ', Style::new()));
                continue;
            }
            let st = match tint {
                Some(t) => Style::fg(Some(t)),
                None => {
                    let c = Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8);
                    Style::fg(Some(lift(c, cfg.boost)))
                }
            };
            row.push((char::from_u32(0x2800 + bits as u32).unwrap_or(' '), st));
        }
        rows.push(row);
    }
    TextArt { rows, w, h }
}

fn render_ascii(img: &RgbImage, w: usize, h: usize, cfg: &ImageCfg, tint: Option<Rgb>) -> TextArt {
    let ramp: Vec<char> = if cfg.ramp.chars().count() > 1 {
        cfg.ramp.chars().collect()
    } else {
        DEFAULT_RAMP.chars().collect()
    };
    let top = ramp.len() - 1;
    let mut rows = Vec::with_capacity(h);
    for cy in 0..h {
        let mut row = Vec::with_capacity(w);
        for cx in 0..w {
            let (x0, y0) = (cx * 2, cy * 4);
            let mut v = [[0f32; 4]; 2];
            let mut sum = 0f32;
            for i in 0..2 {
                for j in 0..4 {
                    let p = img.get_pixel((x0 + i) as u32, (y0 + j) as u32);
                    let l = luma(p[0], p[1], p[2]) as f32;
                    v[i][j] = l;
                    sum += l;
                }
            }
            let mean = sum / 8.0;
            let mut ch = ramp[((mean / 255.0 * top as f32).round() as usize).min(top)];
            if cfg.edges > 0.0 {
                let gx = (v[1].iter().sum::<f32>() - v[0].iter().sum::<f32>()) / 4.0;
                let gy = ((v[0][2] + v[0][3] + v[1][2] + v[1][3]) - (v[0][0] + v[0][1] + v[1][0] + v[1][1])) / 8.0;
                if (gx * gx + gy * gy).sqrt() >= cfg.edges {
                    let (ax, ay) = (gx.abs(), gy.abs());
                    ch = if ay >= 2.4 * ax {
                        '-'
                    } else if ax >= 2.4 * ay {
                        '|'
                    } else if gx * gy > 0.0 {
                        '/'
                    } else {
                        '\\'
                    };
                }
            }
            if ch == ' ' {
                row.push((' ', Style::new()));
                continue;
            }
            let st = match tint {
                Some(t) => Style::fg(Some(t)),
                None => {
                    // colour of the brighter half of the cell, so edges keep their hue
                    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                    for i in 0..2 {
                        for j in 0..4 {
                            if v[i][j] >= mean {
                                let p = img.get_pixel((x0 + i) as u32, (y0 + j) as u32);
                                r += p[0] as u32;
                                g += p[1] as u32;
                                b += p[2] as u32;
                                n += 1;
                            }
                        }
                    }
                    if n == 0 {
                        Style::new()
                    } else {
                        Style::fg(Some(lift(Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8), cfg.boost)))
                    }
                }
            };
            row.push((ch, st));
        }
        rows.push(row);
    }
    TextArt { rows, w, h }
}

// ---------------------------------------------------------------- entry

/// Render `img` into `cols` x `rows` terminal cells.
/// `cell_px` is the pixel size of one cell, needed only for kitty mode.
pub fn render(
    img: &DynamicImage,
    mode: &str,
    cols: usize,
    rows: usize,
    cfg: &ImageCfg,
    tint: Option<Rgb>,
    cell_px: (usize, usize),
) -> Result<Art, String> {
    if cols == 0 || rows == 0 {
        return Err("zero size".into());
    }
    let contain = cfg.fit == "contain";
    match mode {
        "kitty" => {
            let pw = (cols * cell_px.0.max(1)) as u32;
            let ph = (rows * cell_px.1.max(1)) as u32;
            let mut scaled = fit_to(img, pw.max(1), ph.max(1), contain);
            apply_tone(&mut scaled, cfg);
            let mut png: Vec<u8> = Vec::new();
            DynamicImage::ImageRgb8(scaled)
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok(Art::Kitty(KittyArt { png, cols, rows }))
        }
        "blocks" => {
            let mut s = fit_to(img, cols as u32, (rows * 2) as u32, contain);
            apply_tone(&mut s, cfg);
            Ok(Art::Text(render_blocks(&s, cols, rows, tint)))
        }
        "ascii" => {
            let mut s = fit_to(img, (cols * 2) as u32, (rows * 4) as u32, contain);
            apply_tone(&mut s, cfg);
            Ok(Art::Text(render_ascii(&s, cols, rows, cfg, tint)))
        }
        _ => {
            let mut s = fit_to(img, (cols * 2) as u32, (rows * 4) as u32, contain);
            apply_tone(&mut s, cfg);
            Ok(Art::Text(render_braille(&s, cols, rows, cfg, tint)))
        }
    }
}

// ---------------------------------------------------------------- kitty protocol

/// Escape sequence that deletes the image with this id.
pub fn kitty_delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", id)
}

/// Escape sequences that put `art` at (row, col), both 1-based.
/// The cursor is saved and restored, so the caller's position is unaffected.
pub fn kitty_place(art: &KittyArt, id: u32, row: usize, col: usize) -> String {
    kitty_place_at(art, id, &format!("\x1b[{};{}H", row, col))
}

/// Same, but positioned relative to where the cursor is now: `up` lines up and
/// then to absolute column `col`. Used by one-shot mode, which has just printed
/// its lines and must not assume where on the screen they landed.
pub fn kitty_place_relative(art: &KittyArt, id: u32, up: usize, col: usize) -> String {
    let mut pos = String::new();
    if up > 0 {
        pos.push_str(&format!("\x1b[{}A", up));
    }
    pos.push_str(&format!("\x1b[{}G", col));
    kitty_place_at(art, id, &pos)
}

fn kitty_place_at(art: &KittyArt, id: u32, pos: &str) -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(&art.png);
    let mut out = String::with_capacity(data.len() + 256);
    out.push_str("\x1b7"); // save cursor
    out.push_str(pos);
    let chunks: Vec<&str> = data
        .as_bytes()
        .chunks(4096)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    if chunks.is_empty() {
        out.push_str("\x1b8");
        return out;
    }
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,t=d,i={},c={},r={},q=2,m={};{}\x1b\\",
                id, art.cols, art.rows, more, chunk
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={},q=2;{}\x1b\\", more, chunk));
        }
    }
    out.push_str("\x1b8"); // restore cursor
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ImageCfg;

    fn sample() -> DynamicImage {
        let mut im = RgbImage::new(64, 48);
        for (x, y, p) in im.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8]);
        }
        DynamicImage::ImageRgb8(im)
    }

    #[test]
    fn every_mode_has_exact_dimensions() {
        let cfg = ImageCfg::default();
        let img = sample();
        for mode in ["blocks", "braille", "ascii"] {
            for (c, r) in [(33, 15), (48, 24), (7, 3), (1, 1)] {
                let art = render(&img, mode, c, r, &cfg, None, (10, 20)).unwrap();
                match art {
                    Art::Text(t) => {
                        assert_eq!(t.rows.len(), r, "{} rows", mode);
                        for row in &t.rows {
                            assert_eq!(row.len(), c, "{} cols", mode);
                        }
                    }
                    _ => panic!("expected text art"),
                }
            }
        }
    }

    #[test]
    fn kitty_mode_makes_a_png() {
        let cfg = ImageCfg::default();
        let art = render(&sample(), "kitty", 10, 5, &cfg, None, (10, 20)).unwrap();
        match art {
            Art::Kitty(k) => {
                assert_eq!(&k.png[1..4], b"PNG");
                assert_eq!((k.cols, k.rows), (10, 5));
                let seq = kitty_place(&k, 1, 2, 3);
                assert!(seq.starts_with("\x1b7\x1b[2;3H"));
                assert!(seq.contains("a=T,f=100,t=d,i=1,c=10,r=5"));
                assert!(seq.ends_with("\x1b8"));
            }
            _ => panic!("expected kitty art"),
        }
    }

    #[test]
    fn tint_forces_one_colour() {
        let cfg = ImageCfg::default();
        let t = Rgb(1, 2, 3);
        for mode in ["blocks", "braille", "ascii"] {
            let art = render(&sample(), mode, 8, 4, &cfg, Some(t), (10, 20)).unwrap();
            if let Art::Text(a) = art {
                for row in &a.rows {
                    for (ch, st) in row {
                        if *ch != ' ' {
                            assert_eq!(st.fg, Some(t), "{}", mode);
                            assert_eq!(st.bg, None);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn file_urls_decode() {
        assert_eq!(file_url_to_path("file:///tmp/a%20b.png").unwrap(), "/tmp/a b.png");
        assert_eq!(file_url_to_path("file:///x.png").unwrap(), "/x.png");
        assert!(file_url_to_path("https://x/y.png").is_none());
    }
}
