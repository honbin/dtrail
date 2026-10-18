//! Copy to the system clipboard via OSC 52. No platform clipboard dependency,
//! and it works over SSH. The terminal must support OSC 52 (most modern ones do;
//! tmux needs `set -g set-clipboard on`). Fire-and-forget: a terminal that
//! ignores the sequence simply doesn't copy.

use std::io::{self, Write};

/// Set the system clipboard to `text` by writing an OSC 52 sequence to stdout.
pub fn copy(text: &str) {
    let mut out = io::stdout();
    let _ = out.write_all(sequence(text).as_bytes());
    let _ = out.flush();
}

/// The OSC 52 sequence that sets the system clipboard to `text`.
fn sequence(text: &str) -> String {
    // ESC ] 52 ; c ; <base64> BEL
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Minimal standard base64 (RFC 4648, with padding). Avoids a dependency.
fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64, sequence};

    #[test]
    fn sequence_wraps_base64_in_osc52() {
        assert_eq!(sequence("foo"), "\u{1b}]52;c;Zm9v\u{7}");
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
