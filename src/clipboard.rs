//! Copying to the system clipboard through the terminal (OSC 52).

/// Longest text sent to the terminal. Terminals drop bigger sequences silently.
const MAX_OSC52_BYTES: usize = 1 << 20;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(match chunk.len() {
            1 => '=',
            _ => BASE64[(n >> 6) as usize & 63] as char,
        });
        out.push(match chunk.len() {
            3 => BASE64[n as usize & 63] as char,
            _ => '=',
        });
    }
    out
}

/// The escape sequence that puts `text` on the system clipboard, or `None`
/// when it's too large to send.
pub fn osc52(text: &str) -> Option<String> {
    (text.len() <= MAX_OSC52_BYTES).then(|| format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("世".as_bytes()), "5LiW");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn osc52_wraps_the_text_and_refuses_huge_text() {
        assert_eq!(osc52("hi").unwrap(), "\x1b]52;c;aGk=\x07");
        assert!(osc52(&"x".repeat(MAX_OSC52_BYTES)).is_some());
        assert_eq!(osc52(&"x".repeat(MAX_OSC52_BYTES + 1)), None);
    }
}
