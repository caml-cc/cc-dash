//! Making right-to-left text readable in a terminal.
//!
//! Terminals, kitty included, draw the characters they are given from left to
//! right and do not join Arabic letters. Text handed over as-is therefore comes
//! out backwards and with every letter in its isolated form, so the words look
//! like disconnected letters spaced apart.
//!
//! So we do both jobs here before the text reaches the screen:
//!   1. reshape - pick the isolated / initial / medial / final form of each
//!      Arabic letter from its neighbours, and join lam-alef into one glyph
//!   2. reorder - run the Unicode bidirectional algorithm so right-to-left runs
//!      come out in the order they should appear on screen
//!
//! Hebrew and the other right-to-left scripts need no joining, but they do need
//! step 2, and they get it for free.

use std::sync::OnceLock;
use unicode_bidi::BidiInfo;
use unicode_width::UnicodeWidthStr;

/// True if the text contains anything written right to left.
pub fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x0590..=0x05FF |   // Hebrew
            0x0600..=0x06FF |   // Arabic
            0x0700..=0x074F |   // Syriac
            0x0750..=0x077F |   // Arabic Supplement
            0x0780..=0x07BF |   // Thaana
            0x07C0..=0x08FF |   // NKo, Samaritan, Arabic Extended-A
            0xFB1D..=0xFDFF |   // Hebrew and Arabic presentation forms
            0xFE70..=0xFEFF |   // Arabic Presentation Forms-B
            0x10800..=0x10FFF | // old right-to-left scripts
            0x1E800..=0x1EFFF   // Adlam, Arabic Mathematical
        )
    })
}

fn reshaper() -> &'static ar_reshaper::ArabicReshaper {
    static R: OnceLock<ar_reshaper::ArabicReshaper> = OnceLock::new();
    R.get_or_init(ar_reshaper::ArabicReshaper::default)
}

/// Which parts of the right-to-left work termdash should do.
///
/// Terminals disagree about how much they handle themselves. kitty, for
/// instance, shapes Arabic with HarfBuzz but does no reordering; others do
/// neither, and a few do both. Doing a job the terminal also does can be worse
/// than doing nothing, so this is switchable and `--debug-text` prints all
/// four so you can see which one your terminal actually wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RtlMode {
    /// Join the letters and reorder. The default.
    ShapeAndReorder,
    /// Reorder only, for a terminal that shapes Arabic itself.
    ReorderOnly,
    /// Join only, for a terminal that reorders but does not shape.
    ShapeOnly,
    /// Send the text exactly as it came.
    Off,
}

impl RtlMode {
    pub fn parse(s: &str) -> RtlMode {
        match s.trim().to_lowercase().replace('_', "-").as_str() {
            "reorder-only" | "reorder" => RtlMode::ReorderOnly,
            "shape-only" | "shape" => RtlMode::ShapeOnly,
            "off" | "none" | "raw" => RtlMode::Off,
            _ => RtlMode::ShapeAndReorder,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            RtlMode::ShapeAndReorder => "shape-and-reorder",
            RtlMode::ReorderOnly => "reorder-only",
            RtlMode::ShapeOnly => "shape-only",
            RtlMode::Off => "off",
        }
    }
    pub const ALL: [RtlMode; 4] = [
        RtlMode::ShapeAndReorder,
        RtlMode::ReorderOnly,
        RtlMode::ShapeOnly,
        RtlMode::Off,
    ];
}

static MODE: OnceLock<RtlMode> = OnceLock::new();

/// Set once at startup, from the config.
pub fn set_rtl_mode(m: RtlMode) {
    let _ = MODE.set(m);
}

fn rtl_mode() -> RtlMode {
    MODE.get().copied().unwrap_or(RtlMode::ShapeAndReorder)
}

fn reorder(s: &str) -> String {
    // The base direction is pinned left-to-right on purpose. The dashboard is
    // a left-to-right grid of fixed columns, and letting an Arabic value flip
    // the paragraph would throw the rest of the line to the other side of its
    // slot. Right-to-left runs are still reordered correctly inside it, which
    // is the part that actually matters for reading.
    let bidi = BidiInfo::new(s, Some(unicode_bidi::Level::ltr()));
    if bidi.paragraphs.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for para in &bidi.paragraphs {
        out.push_str(&bidi.reorder_line(para, para.range.clone()));
    }
    out
}

/// Apply one particular mode, whatever the configured one is.
pub fn for_display_with(s: &str, mode: RtlMode) -> String {
    if !has_rtl(s) {
        return s.to_string();
    }
    match mode {
        RtlMode::ShapeAndReorder => reorder(&reshaper().reshape(s)),
        RtlMode::ReorderOnly => reorder(s),
        RtlMode::ShapeOnly => reshaper().reshape(s),
        RtlMode::Off => s.to_string(),
    }
}

/// Turn logical-order text into what the terminal should actually print.
/// Left-to-right text is returned untouched, so this is cheap for most strings.
pub fn for_display(s: &str) -> String {
    for_display_with(s, rtl_mode())
}

/// Display columns the text takes up.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// One entry per display column: the text to print at that column, or None for
/// the right-hand half of a double-width character. Combining marks ride along
/// with the character they belong to.
fn columns(s: &str) -> Vec<Option<String>> {
    let mut v: Vec<Option<String>> = Vec::new();
    for ch in s.chars() {
        match unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) {
            0 => {
                // a combining mark belongs to the character before it
                if let Some(Some(last)) = v.iter_mut().rev().find(|c| c.is_some()) {
                    last.push(ch);
                }
            }
            1 => v.push(Some(ch.to_string())),
            _ => {
                v.push(Some(ch.to_string()));
                v.push(None);
            }
        }
    }
    v
}

/// Exactly `cols` columns of `ring`, starting `start` columns in and wrapping
/// back to the beginning. This is the scrolling marquee: `ring` is the text
/// plus a separator, so it reads continuously as it goes round.
pub fn scroll_window(ring: &str, cols: usize, start: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    let cells = columns(ring);
    if cells.is_empty() {
        return " ".repeat(cols);
    }
    let total = cells.len();
    let start = start % total;
    let mut out = String::new();
    let mut i = 0;
    while i < cols {
        match &cells[(start + i) % total] {
            Some(s) => {
                let w = UnicodeWidthStr::width(s.as_str()).max(1);
                if i + w > cols {
                    // a double-width character would hang over the right edge
                    out.push(' ');
                    i += 1;
                } else {
                    out.push_str(s);
                    i += w;
                }
            }
            // left edge landed on the second half of a double-width character
            None => {
                out.push(' ');
                i += 1;
            }
        }
    }
    out
}

/// Cut to `cols` display columns, keeping the logical beginning of the text.
/// Right-to-left text must be cut before it is reordered, otherwise the cut
/// takes the end of the sentence instead of the start.
pub fn truncate(s: &str, cols: usize) -> String {
    if UnicodeWidthStr::width(s) <= cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > cols {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Cut to `cols`, then make it printable. Use this for anything that goes on
/// screen in a fixed-width slot.
pub fn prepare(s: &str, cols: usize) -> String {
    let cut = truncate(s, cols);
    let shown = for_display(&cut);
    // Joining can only ever make text narrower (lam-alef becomes one glyph),
    // but clamp anyway so a surprise can never push a column out of line.
    if UnicodeWidthStr::width(shown.as_str()) > cols {
        truncate(&shown, cols)
    } else {
        shown
    }
}

/// Cut to `cols`, make it printable, then pad with spaces to exactly `cols`.
pub fn fit(s: &str, cols: usize) -> String {
    let shown = prepare(s, cols);
    let used = UnicodeWidthStr::width(shown.as_str());
    let mut out = shown;
    out.push_str(&" ".repeat(cols.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // "مرحبا" (hello): meem, reh, hah, beh, alef in logical order
    const HELLO: &str = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}";

    #[test]
    fn every_rtl_mode_does_what_it_says() {
        let joined = reshaper().reshape(HELLO);
        let shaped: Vec<char> = joined.chars().collect();

        let off = for_display_with(HELLO, RtlMode::Off);
        assert_eq!(off, HELLO, "off must change nothing");

        let shape_only = for_display_with(HELLO, RtlMode::ShapeOnly);
        assert_eq!(shape_only, joined, "shape-only must not reorder");
        assert!(shape_only.chars().all(|c| (0xFE70..=0xFEFF).contains(&(c as u32))));

        let reorder_only = for_display_with(HELLO, RtlMode::ReorderOnly);
        let orig: Vec<char> = HELLO.chars().collect();
        assert_eq!(reorder_only.chars().next(), Some(orig[orig.len() - 1]));
        assert!(
            reorder_only.chars().all(|c| !(0xFE70..=0xFEFF).contains(&(c as u32))),
            "reorder-only must not join"
        );

        let both = for_display_with(HELLO, RtlMode::ShapeAndReorder);
        assert_eq!(both.chars().next(), Some(shaped[shaped.len() - 1]));
        assert_ne!(both, shape_only);
        assert_ne!(both, reorder_only);

        // all four leave plain text alone
        for m in RtlMode::ALL {
            assert_eq!(for_display_with("Nothing playing", m), "Nothing playing");
        }
    }

    #[test]
    fn rtl_mode_names_round_trip() {
        for m in RtlMode::ALL {
            assert_eq!(RtlMode::parse(m.name()), m);
        }
        assert_eq!(RtlMode::parse("reorder_only"), RtlMode::ReorderOnly);
        assert_eq!(RtlMode::parse("  OFF  "), RtlMode::Off);
        // anything unrecognised falls back to the full treatment
        assert_eq!(RtlMode::parse("nonsense"), RtlMode::ShapeAndReorder);
    }

    #[test]
    fn plain_text_is_left_alone() {
        assert!(!has_rtl("Nothing playing"));
        assert_eq!(for_display("Nothing playing"), "Nothing playing");
        assert_eq!(for_display(""), "");
    }

    #[test]
    fn arabic_is_detected() {
        assert!(has_rtl(HELLO));
        assert!(has_rtl("track \u{0645}\u{0631}"));
        assert!(has_rtl("\u{05D0}")); // Hebrew alef
        assert!(!has_rtl("Pizzicato Five"));
    }

    #[test]
    fn letters_are_joined_into_presentation_forms() {
        let out = for_display(HELLO);
        // every letter should now be a Presentation Forms-B glyph, not the
        // isolated base letter it started as
        assert!(
            out.chars().all(|c| (0xFE70..=0xFEFF).contains(&(c as u32))),
            "expected joined forms, got {:?}",
            out.chars().map(|c| format!("{:04X}", c as u32)).collect::<Vec<_>>()
        );
        // and none of the original isolated code points should survive
        for c in HELLO.chars() {
            assert!(!out.contains(c), "{:04X} was left unjoined", c as u32);
        }
    }

    #[test]
    fn right_to_left_text_is_reversed_for_the_screen() {
        let out: Vec<char> = for_display(HELLO).chars().collect();
        let logical: Vec<char> = reshaper().reshape(HELLO).chars().collect();
        assert_eq!(out.len(), logical.len());
        // the last letter in logical order must be printed first
        assert_eq!(out[0], logical[logical.len() - 1]);
        assert_eq!(out[out.len() - 1], logical[0]);
    }

    #[test]
    fn lam_alef_becomes_one_glyph() {
        // lam + alef is a single ligature, so the text gets one column narrower
        let lam_alef = "\u{0644}\u{0627}";
        let out = for_display(lam_alef);
        assert_eq!(out.chars().count(), 1, "lam-alef should join into one glyph");
        assert!(matches!(out.chars().next().unwrap() as u32, 0xFEF5..=0xFEFC));
    }

    #[test]
    fn mixed_text_keeps_the_latin_part_in_order() {
        let mixed = format!("Playing: {}", HELLO);
        let out = for_display(&mixed);
        assert!(out.starts_with("Playing: "), "got {:?}", out);
    }

    #[test]
    fn a_label_in_arabic_does_not_shunt_the_rest_of_the_line() {
        // "[<arabic>] 157 Days remaining" must keep the English where it was,
        // whichever way round the label is written.
        let line = format!("[{}] 157 Days remaining", HELLO);
        let out = for_display(&line);
        assert!(out.starts_with('['), "got {:?}", out);
        assert!(out.ends_with("] 157 Days remaining"), "got {:?}", out);
        // and the Arabic between the brackets is still reversed and joined
        let inner: Vec<char> = out
            .chars()
            .skip(1)
            .take_while(|c| *c != ']')
            .collect();
        let joined: Vec<char> = reshaper().reshape(HELLO).chars().collect();
        assert_eq!(inner[0], joined[joined.len() - 1]);
    }

    #[test]
    fn numbers_inside_arabic_stay_readable() {
        // Digits are weak: they must not come out backwards.
        let s = format!("{} 2026", HELLO);
        let out = for_display(&s);
        assert!(out.contains("2026"), "digits were reversed: {:?}", out);
    }

    #[test]
    fn fields_stay_exactly_their_width() {
        for width in [0, 1, 5, 12, 25, 40] {
            for s in [HELLO, "Nothing playing", "\u{05D0}\u{05D1}\u{05D2}", "東京は夜の七時", ""] {
                let out = fit(s, width);
                assert_eq!(
                    UnicodeWidthStr::width(out.as_str()),
                    width,
                    "fit({:?}, {}) gave {:?}",
                    s,
                    width,
                    out
                );
            }
        }
    }

    #[test]
    fn scroll_window_is_always_exactly_the_asked_width() {
        for s in ["abcdef", HELLO, "東京は夜の七時", "a", "", "e\u{0301}x"] {
            for cols in 0..12 {
                for start in 0..14 {
                    let out = scroll_window(s, cols, start);
                    assert_eq!(
                        width(&out),
                        cols,
                        "scroll_window({:?}, {}, {}) gave {:?}",
                        s, cols, start, out
                    );
                }
            }
        }
    }

    #[test]
    fn scroll_window_moves_along_and_wraps_round() {
        let ring = "ABCDEF";
        assert_eq!(scroll_window(ring, 3, 0), "ABC");
        assert_eq!(scroll_window(ring, 3, 1), "BCD");
        assert_eq!(scroll_window(ring, 3, 4), "EFA");
        // back to the start after one full turn
        assert_eq!(scroll_window(ring, 3, 6), "ABC");
        assert_eq!(scroll_window(ring, 8, 4), "EFABCDEF");
    }

    #[test]
    fn scroll_window_does_not_split_a_wide_character() {
        // Each of these is two columns wide, so a window landing mid-character
        // must show a space rather than half a glyph.
        let ring = "\u{6771}\u{4EAC}"; // 東京
        assert_eq!(scroll_window(ring, 2, 0), "\u{6771}");
        assert_eq!(scroll_window(ring, 1, 0), " ");
        // starting mid-character: a space for the tail, and the next one is
        // dropped because two more columns will not fit in the one left
        assert_eq!(scroll_window(ring, 2, 1), "  ");
        assert_eq!(scroll_window(ring, 3, 1), " \u{4EAC}");
        for start in 0..8 {
            assert_eq!(width(&scroll_window(ring, 3, start)), 3);
        }
    }

    #[test]
    fn combining_marks_stay_with_their_letter() {
        let s = "e\u{0301}"; // e + combining acute
        assert_eq!(width(s), 1);
        assert_eq!(scroll_window(s, 1, 0), s);
    }

    #[test]
    fn truncation_keeps_the_start_of_the_sentence() {
        // cut to 3 columns, so the first three letters logically survive
        let cut = truncate(HELLO, 3);
        assert_eq!(cut.chars().count(), 3);
        assert_eq!(cut.chars().next(), HELLO.chars().next());
    }

    #[test]
    fn long_arabic_never_overflows_its_slot() {
        let long = HELLO.repeat(20);
        for w in 1..30 {
            assert!(UnicodeWidthStr::width(prepare(&long, w).as_str()) <= w);
        }
    }
}
