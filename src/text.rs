//! Pure text-display helpers: measuring display width, clipping and padding to a
//! column budget, wrapping, and stripping escape/control sequences. No TUI
//! dependency, so they're unit-tested here.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Display columns a string occupies (CJK counts as 2).
pub fn display_width(s: &str) -> usize {
    s.width()
}

/// Each char together with the zero-width chars after it (combining marks such
/// as NFD Japanese voiced marks, VS16), measured as one string. A zero-width
/// char can change its base's width (VS16 widens some emoji), so a lone char's
/// width isn't the cluster's, and breaking inside a cluster splits a glyph.
fn clusters(s: &str) -> impl Iterator<Item = (&str, usize)> {
    let mut starts = s
        .char_indices()
        .filter(|&(i, c)| i == 0 || c.width().unwrap_or(0) > 0)
        .map(|(i, _)| i)
        .peekable();
    std::iter::from_fn(move || {
        let start = starts.next()?;
        let cluster = &s[start..starts.peek().copied().unwrap_or(s.len())];
        Some((cluster, cluster.width()))
    })
}

/// Truncate to at most `width` display columns.
pub fn clip_to_width(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for (cluster, cw) in clusters(s) {
        if w + cw > width {
            break;
        }
        out.push_str(cluster);
        w += cw;
    }
    out
}

/// Clip to `width`, then pad with spaces to exactly `width` so a styled line
/// fills its row — full-width highlight, and every cell is overwritten.
pub fn fit_to_width(s: &str, width: usize) -> String {
    let mut out = clip_to_width(s, width);
    let pad = width.saturating_sub(display_width(&out));
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

/// Wrap one line by display width (CJK counts as 2), breaking on width, not words.
pub fn wrap_line(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for (cluster, w) in clusters(text) {
        if current_width + w > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(cluster);
        current_width += w;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Strip escape sequences and control characters so they are never written into
/// a terminal cell. Rendered raw, an escape is consumed by the terminal as a
/// zero-width control while we count its bytes as columns, desyncing the grid
/// and corrupting later cells. Session content (e.g. colored tool output)
/// regularly contains these.
///
/// Handled: CSI (`ESC [` … final `0x40–0x7E`, covers SGR and cursor moves), OSC
/// (`ESC ]` … `BEL` or `ESC \`), any other two-byte `ESC x`, lone C0/C1 control
/// characters (dropped), and tabs (turned into a space). This is not a full
/// terminal parser, but it covers what shows up in session logs.
pub fn sanitize_display(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{1b}' {
            i += 1;
            match chars.get(i) {
                Some('[') => {
                    i += 1;
                    while i < chars.len() && !('\u{40}'..='\u{7e}').contains(&chars[i]) {
                        i += 1;
                    }
                    i += 1; // consume the final byte
                }
                Some(']') => {
                    i += 1;
                    while i < chars.len() {
                        if chars[i] == '\u{07}' {
                            i += 1;
                            break;
                        }
                        if chars[i] == '\u{1b}' && chars.get(i + 1) == Some(&'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                Some(_) => i += 1, // other two-byte escape
                None => {}
            }
        } else if c == '\t' {
            out.push(' ');
            i += 1;
        } else if c.is_control() {
            i += 1;
        } else if is_bidi_control(c) {
            // Shown rather than dropped: a viewer should reveal that the log
            // carries them, without letting them reorder what the terminal draws.
            out.push_str(&format!("‹U+{:04X}›", c as u32));
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Explicit embedding/override (U+202A–202E) and isolate (U+2066–2069)
/// controls. LRM/RLM (U+200E/200F) are left alone: they only mark direction.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_clips_to_exact_display_width() {
        let s = fit_to_width("ab", 5);
        assert_eq!(s, "ab   ");
        assert_eq!(display_width(&s), 5);

        assert_eq!(fit_to_width("abcdef", 3), "abc");

        let s = fit_to_width("あ", 5); // width 2 -> pad 3
        assert_eq!(display_width(&s), 5);
    }

    #[test]
    fn wrap_line_respects_cjk_width() {
        // Two full-width chars = width 4; at width 3 they split one per line.
        let parts = wrap_line("あい", 3);
        assert_eq!(parts, vec!["あ".to_string(), "い".to_string()]);
    }

    #[test]
    fn wrap_line_keeps_ascii_together_within_width() {
        let parts = wrap_line("abcdef", 4);
        assert_eq!(parts, vec!["abcd".to_string(), "ef".to_string()]);
    }

    #[test]
    fn sanitize_display_strips_ansi_and_controls() {
        assert_eq!(sanitize_display("\u{1b}[32mok\u{1b}[m done"), "ok done");
        assert_eq!(sanitize_display("a\u{7}b\tc"), "ab c");
        assert_eq!(sanitize_display("plain"), "plain");
    }

    #[test]
    fn sanitize_display_reveals_bidi_controls() {
        assert_eq!(sanitize_display("a\u{202E}b"), "a‹U+202E›b");
        assert_eq!(sanitize_display("\u{2066}x\u{2069}"), "‹U+2066›x‹U+2069›");
        // Direction marks are not reordering controls and pass through.
        assert_eq!(sanitize_display("a\u{200F}b"), "a\u{200F}b");
    }

    #[test]
    fn a_revealed_bidi_control_wraps_by_its_visible_width() {
        // "ab‹U+202E›" is 10 columns, so at width 5 it splits evenly.
        let text = sanitize_display("ab\u{202E}");
        assert_eq!(display_width(&text), 10);
        assert_eq!(wrap_line(&text, 5), vec!["ab‹U+", "202E›"]);
    }

    #[test]
    fn combining_marks_take_no_column_but_vs16_widens() {
        // NFD "が" ×5: each base kana plus a separate voiced mark, 10 columns.
        let nfd = "か\u{3099}".repeat(5);
        assert_eq!(wrap_line(&nfd, 10), vec![nfd.clone()]);
        assert_eq!(clip_to_width(&format!("{nfd}x"), 10), nfd);
        // "❤️" is two columns though its base char alone is one, so nothing
        // after it fits in three, and it moves down whole rather than split.
        let heart = "\u{2764}\u{FE0F}";
        assert_eq!(clip_to_width(&format!("a{heart}b"), 3), format!("a{heart}"));
        assert_eq!(
            wrap_line(&format!("a{heart}b"), 3),
            vec![format!("a{heart}"), "b".into()]
        );
        assert_eq!(
            wrap_line(&format!("ab{heart}"), 3),
            vec!["ab".to_string(), heart.to_string()]
        );
        // VS16 after a wide emoji or a letter adds no column.
        let grin = "\u{1F600}\u{FE0F}";
        assert_eq!(wrap_line(grin, 2), vec![grin]);
        assert_eq!(wrap_line("a\u{FE0F}", 1), vec!["a\u{FE0F}"]);
        assert_eq!(clip_to_width(&format!("{grin}xy"), 3), format!("{grin}x"));
    }
}
