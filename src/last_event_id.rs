//! Pure Rust implementation of 17track's Last-Event-ID header/cookie generation.
//!
//! The Last-Event-ID is only sent on the first API request (when `guid` is empty).
//! It is constructed from a DJB2 canvas fingerprint hash, a murmur-like hash of
//! request metadata, and the `_yq_bid` device identifier.
//!
//! Algorithm reverse-engineered from 17track's layout JS chunk.

use std::time::{SystemTime, UNIX_EPOCH};

/// Static canvas fingerprint DJB2 hash.
///
/// The original JS draws `"https://github.com/fingerprintjs/fingerprintjs2"` on a
/// canvas and hashes the result including screen properties. We use a fixed value
/// that represents a consistent browser environment.
///
/// The hash input string is:
/// `{colorDepth}\r\n{language}\r\n{tzOffset}\r\n{height}x{width}\r\n{canvasDataURL}`
///
/// For a standard Windows Chrome environment (24-bit color, en-US, UTC-8, 1080x1920),
/// we use a precomputed constant. The server doesn't validate the actual canvas content,
/// just that the format is consistent.
const DEFAULT_CANVAS_HASH: u32 = 1022200205;

/// Default timezone offset to use in the metadata string.
/// This is the browser's `new Date().getTimezoneOffset()`, NOT the API's timeZoneOffset.
/// 300 = UTC-5 (Eastern), 480 = UTC-8 (Pacific), etc.
const DEFAULT_TZ_OFFSET: i32 = 300;

/// DJB2 hash (seed 5381), iterating in reverse order.
///
/// Matches the JS implementation:
/// ```js
/// function P(e) {
///     if (!e) return 0;
///     for (var a = 5381, i = e.length; i;)
///         a = 33 * a ^ e.charCodeAt(--i);
///     return a >>> 0;
/// }
/// ```
pub fn djb2(s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    let mut a: i32 = 5381;
    for ch in s.chars().rev() {
        a = a.wrapping_mul(33) ^ (ch as i32);
    }
    a as u32
}

/// Murmur-like hash (seed 0x4e67c6a7), iterating in reverse order.
///
/// Matches the JS implementation:
/// ```js
/// var l = 0x4e67c6a7 ^ (t << 16);
/// for (r = e.length - 1; r >= 0; r--)
///     o = e.charCodeAt(r),
///     l ^= (l << 5) + o + (l >> 2);
/// return Math.abs(0x7fffffff & l);
/// ```
///
/// Note: All operations are signed 32-bit. `>>` is arithmetic shift in JS for i32.
fn murmur_hash(s: &str, t: i32) -> u32 {
    if s.is_empty() {
        return 0;
    }
    let mut l: i32 = (0x4e67c6a7_u32 as i32) ^ (t << 16);
    for ch in s.chars().rev() {
        let o = ch as i32;
        // JS: l ^= (l << 5) + o + (l >> 2)
        // >> on signed i32 in JS is arithmetic shift
        let val = (l << 5).wrapping_add(o).wrapping_add(l >> 2);
        l ^= val;
    }
    (0x7fffffff & l).unsigned_abs()
}

/// Zero-pad a hex string to 8 characters.
fn pad8_hex(value: u32) -> String {
    format!("{:08x}", value)
}

/// Hex-encode each character of a string (no per-char zero-padding).
///
/// For printable ASCII (>= 0x20), each char produces exactly 2 hex digits.
/// The first char omits leading zeros, subsequent chars use `.toString(16)`.
///
/// JS implementation:
/// ```js
/// for (var t = "", n = 0; n < e.length; n++)
///     "" == t ? t = e.charCodeAt(n).toString(16) : t += e.charCodeAt(n).toString(16);
/// ```
fn hex_encode_chars(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for (i, ch) in s.chars().enumerate() {
        let code = ch as u32;
        if i == 0 {
            // First char: no zero-padding (JS .toString(16))
            result.push_str(&format!("{:x}", code));
        } else {
            result.push_str(&format!("{:x}", code));
        }
    }
    result
}

/// Configuration for Last-Event-ID generation.
pub struct LastEventIdConfig {
    /// The `_yq_bid` device identifier (e.g., `"G-EA6CFDB403493F2A"`).
    pub yq_bid: String,
    /// The `window.YQ.configs.md5` value from the page HTML (e.g., `"1.0.156"`).
    pub configs_md5: String,
    /// Browser timezone offset from `new Date().getTimezoneOffset()` (e.g., 300 for EST).
    pub tz_offset: i32,
    /// DJB2 hash of the canvas fingerprint string. Use `DEFAULT_CANVAS_HASH` for standard env.
    pub canvas_hash: u32,
}

impl Default for LastEventIdConfig {
    fn default() -> Self {
        Self {
            yq_bid: String::new(),
            configs_md5: "1.0.156".to_string(),
            tz_offset: DEFAULT_TZ_OFFSET,
            canvas_hash: DEFAULT_CANVAS_HASH,
        }
    }
}

/// Generate the Last-Event-ID header value.
///
/// # Arguments
/// * `request_body_json` - The full JSON string of the tracking request body
///   (used to compute C[5] hash).
/// * `config` - Configuration with yq_bid, md5, timezone, and canvas hash.
///
/// # Returns
/// The hex-encoded Last-Event-ID string suitable for both the header and cookie.
pub fn generate_last_event_id(request_body_json: &str, config: &LastEventIdConfig) -> String {
    // C array: [hex_encoded_reversed, _, _, domain_check, murmur_metadata, murmur_body]
    // Indices used: C[0], C[3], C[4], C[5]

    // Step 1: Hash request body -> C[5]
    let body_hash = murmur_hash(request_body_json, request_body_json.len() as i32);
    let c5 = pad8_hex(body_hash);

    // Step 2: Canvas fingerprint hash (s) and I multiplier
    let s = config.canvas_hash;
    // I doubles from 5 to 10 after fingerprint (not used in output, just internal state)

    // Step 3: Captcha hash (r) - normally 0 (no captcha on initial request)
    let r: u32 = 0;

    // Step 4: Build the metadata string "a"
    let timestamp_hex = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    // T = _yq_bid cookie value, or fall back to canvas hash string
    let t_value = if config.yq_bid.is_empty() {
        s.to_string()
    } else {
        config.yq_bid.clone()
    };

    // webdriver = "false" (we're not a webdriver)
    // t = 0 (initial counter parameter)
    // S = 0 (global counter)
    // xhr = "true" (XMLHttpRequest available)
    let a = format!(
        "{}:false:{}:0:0/{}/11/true/{}/{}/{}/{}",
        t_value, s, timestamp_hex, config.tz_offset, s, config.configs_md5, r,
    );

    // Step 5: Hash metadata string -> C[4], also sets C[3] = 4
    let metadata_hash = murmur_hash(&a, 0);
    let c4 = pad8_hex(metadata_hash);
    let c3 = "4"; // Domain matches .17track.net

    // Step 6: Reverse string and hex-encode -> C[0]
    let reversed: String = a.chars().rev().collect();
    let c0 = hex_encode_chars(&reversed);

    // Step 7: Assemble C[0] + C[3] + C[4] + C[5]
    format!("{}{}{}{}", c0, c3, c4, c5)
}

/// Generate the cookie string for the Last-Event-ID.
///
/// Returns a cookie string like `"yq-=<value>;path=/;domain=17track.net"`
pub fn generate_last_event_id_cookie(
    request_body_json: &str,
    config: &LastEventIdConfig,
) -> String {
    let value = generate_last_event_id(request_body_json, config);
    format!("yq-={};path=/;domain=17track.net", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_djb2_empty() {
        assert_eq!(djb2(""), 0);
    }

    #[test]
    fn test_djb2_known_value() {
        // Verify DJB2 algorithm with known inputs.
        let hash = djb2("test");
        assert_ne!(hash, 0);
        // DJB2 of "test" iterating in reverse: t(116), s(115), e(101), t(116)
        // All arithmetic is wrapping i32, then cast to u32 at the end.
        // a = 5381
        // a = (5381 * 33) ^ 116 = 177573 ^ 116 = 177521
        // a = (177521 * 33) ^ 115 = 5858193 ^ 115 = 5858242
        // a = (5858242 * 33) ^ 101 = 193321986 ^ 101 = 193321951
        // a = (193321951 * 33) ^ 116 = wrapping → 2087933123 ^ 116 = 2087933171
        assert_eq!(hash, 2087933171);
    }

    #[test]
    fn test_murmur_empty() {
        assert_eq!(murmur_hash("", 0), 0);
    }

    #[test]
    fn test_murmur_basic() {
        let hash = murmur_hash("hello", 5);
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_pad8_hex() {
        assert_eq!(pad8_hex(0), "00000000");
        assert_eq!(pad8_hex(255), "000000ff");
        assert_eq!(pad8_hex(0x039c8884), "039c8884");
        assert_eq!(pad8_hex(0x20b04e11), "20b04e11");
    }

    #[test]
    fn test_hex_encode_chars() {
        // "AB" -> 0x41 0x42 -> "4142"
        assert_eq!(hex_encode_chars("AB"), "4142");
        // "0" -> 0x30 -> "30"
        assert_eq!(hex_encode_chars("0"), "30");
    }

    #[test]
    fn test_generate_format() {
        let config = LastEventIdConfig {
            yq_bid: "G-EA6CFDB403493F2A".to_string(),
            configs_md5: "1.0.156".to_string(),
            tz_offset: 300,
            canvas_hash: DEFAULT_CANVAS_HASH,
        };

        let body = r#"{"data":[{"num":"TEST123","fc":0,"sc":0}],"guid":"","timeZoneOffset":-480,"sign":"test"}"#;
        let result = generate_last_event_id(body, &config);

        // Should be a hex string followed by "4" + 8 hex chars + 8 hex chars
        assert!(!result.is_empty());
        // All chars should be hex digits
        for ch in result.chars() {
            assert!(
                ch.is_ascii_hexdigit(),
                "Non-hex char '{}' in: {}",
                ch,
                result
            );
        }
    }

    /// Test against the known-good value from the HAR file.
    ///
    /// The HAR shows that for a specific request with:
    /// - `_yq_bid` = "G-EA6CFDB403493F2A"
    /// - `configs.md5` = "1.0.156"
    /// - `tz_offset` = 300
    /// - `canvas_hash` = 1022200205
    /// - timestamp_hex = "19bf6ded9f6"
    ///
    /// The metadata string "a" is:
    /// `G-EA6CFDB403493F2A:false:1022200205:0:0/19bf6ded9f6/11/true/300/1022200205/1.0.156/0`
    ///
    /// And the expected output is:
    /// `302f3635312e302e312f353032303032323230312f3030332f657572742f31312f36663964656436666239312f303a303a353032303032323230313a65736c61663a413246333934333034424446433641452d47420b04e11039c8884`
    #[test]
    fn test_known_hashes() {
        // Test the murmur hash of the known metadata string
        let a =
            "G-EA6CFDB403493F2A:false:1022200205:0:0/19bf6ded9f6/11/true/300/1022200205/1.0.156/0";

        // C[4] = murmur_hash(a, 0) -> should be 0x20b04e11
        let c4 = murmur_hash(a, 0);
        assert_eq!(
            pad8_hex(c4),
            "20b04e11",
            "C[4] murmur hash mismatch for metadata string"
        );

        // Test the hex encoding of the reversed string
        let reversed: String = a.chars().rev().collect();
        let c0 = hex_encode_chars(&reversed);
        assert_eq!(
            c0,
            "302f3635312e302e312f353032303032323230312f3030332f657572742f31312f36663964656436666239312f303a303a353032303032323230313a65736c61663a413246333934333034424446433641452d47",
            "C[0] hex encoding mismatch"
        );

        // Full output: C[0] + "4" + C[4] + C[5]
        // C[5] depends on the request body which we'd need to reproduce exactly
    }
}
