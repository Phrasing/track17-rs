/// Generate a `_yq_bid` device/session identifier.
///
/// Format: `G-{16 uppercase hex chars}` (e.g., `G-EA6CFDB403493F2A`)
///
/// The original algorithm from 17track's JS (module 64179) uses:
/// ```js
/// (new Date().getTime() + 16 * Math.random()) % 16 | 0
/// ```
/// applied to a pattern `"G-xxxxxxxxxxxxxxxx"` where each `x` is replaced
/// with a random hex digit.
pub fn generate_yq_bid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut result = String::with_capacity(18);
    result.push_str("G-");

    // Replicate the JS algorithm: (timestamp + 16 * Math.random()) % 16 | 0
    // Each character uses a fresh random value mixed with the timestamp
    for _ in 0..16 {
        let rand_val: f64 = fastrand::f64();
        let digit = ((timestamp as f64 + 16.0 * rand_val) % 16.0) as u8;
        result.push(
            std::char::from_digit(digit as u32, 16)
                .unwrap_or('0')
                .to_ascii_uppercase(),
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format() {
        let bid = generate_yq_bid();
        assert!(bid.starts_with("G-"), "Should start with G-: {}", bid);
        assert_eq!(bid.len(), 18, "Should be 18 chars: {}", bid);
        // All chars after "G-" should be uppercase hex
        for c in bid[2..].chars() {
            assert!(
                c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_uppercase()),
                "Invalid hex char '{}' in: {}",
                c,
                bid
            );
        }
    }

    #[test]
    fn test_uniqueness() {
        let a = generate_yq_bid();
        let b = generate_yq_bid();
        // Not strictly guaranteed but extremely likely
        assert_ne!(a, b, "Two sequential calls should produce different values");
    }
}
