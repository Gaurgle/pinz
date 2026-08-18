//! Putting text on the system clipboard from inside the alternate screen.
//!
//! Uses OSC 52: the text is handed to the *terminal*, which owns the real
//! clipboard. That is what makes it work over SSH, and it needs no crate - the
//! only machinery is a base64 encoder, which is twenty lines.
//!
//! Not every terminal implements it. iTerm2, Ghostty, kitty, WezTerm and
//! Alacritty do; macOS Terminal.app does not. tmux needs `set-clipboard on`.

use std::io::{self, Write};

/// Largest text we will try to send. Terminals silently drop oversized OSC 52
/// payloads, so past this we report a failure the caller can show instead of
/// truncating a note behind the user's back.
const MAX_BYTES: usize = 100 * 1024;

/// Write `text` to the system clipboard.
pub fn copy(out: &mut impl Write, text: &str) -> io::Result<()> {
    let Some(seq) = osc52(text) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "too much text to copy ({} bytes, limit {MAX_BYTES})",
                text.len()
            ),
        ));
    };
    out.write_all(seq.as_bytes())?;
    out.flush()
}

/// The OSC 52 escape that sets the clipboard to `text`, or `None` if the text is
/// past [`MAX_BYTES`].
///
/// Sent plain, always - including inside tmux. tmux relays a plain OSC 52 to the
/// outer terminal itself when `set-clipboard on`, and populates its own paste
/// buffer while it is at it. The tempting alternative, wrapping this in tmux's
/// DCS passthrough, needs `allow-passthrough on`, which is *off* by default, so
/// it turns a working escape into one tmux silently discards.
fn osc52(text: &str) -> Option<String> {
    if text.len() > MAX_BYTES {
        return None;
    }
    Some(format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
}

/// Base64 per RFC 4648: the standard alphabet, padded with `=`.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        // Pack the chunk into the low 24 bits, then read it back out six bits
        // at a time. Missing input bytes are zero and become '=' below.
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        // Section 10 of the RFC. These cover all three padding cases.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_the_bytes_of_non_ascii_text() {
        assert_eq!(base64("å".as_bytes()), "w6U=");
    }

    #[test]
    fn osc52_wraps_the_payload_in_the_clipboard_escape() {
        assert_eq!(osc52("foo").unwrap(), "\x1b]52;c;Zm9v\x07");
    }

    /// tmux relays a plain OSC 52 itself when `set-clipboard on`. It is the DCS
    /// passthrough form that needs `allow-passthrough on`, which is off by
    /// default - so wrapping the escape is how you get it silently dropped.
    #[test]
    fn osc52_is_never_wrapped_for_tmux() {
        let seq = osc52("foo").unwrap();
        assert!(
            !seq.contains("\x1bPtmux;"),
            "no passthrough wrapper: {seq:?}"
        );
        assert!(!seq.contains("\x1b\x1b"), "no doubled escape: {seq:?}");
    }

    #[test]
    fn osc52_refuses_a_payload_past_the_cap() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(osc52(&big).is_none());
        assert!(
            osc52(&"x".repeat(MAX_BYTES)).is_some(),
            "the cap itself is allowed"
        );
    }

    #[test]
    fn copy_writes_the_escape_to_the_sink() {
        let mut out: Vec<u8> = Vec::new();
        copy(&mut out, "foo").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn copy_reports_an_oversized_payload_rather_than_truncating_it() {
        let mut out: Vec<u8> = Vec::new();
        let err = copy(&mut out, &"x".repeat(MAX_BYTES + 1)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(out.is_empty(), "nothing partial reaches the terminal");
    }
}
