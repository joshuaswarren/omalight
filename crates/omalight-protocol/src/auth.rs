/// Shared-secret compare. Length is not returned early, and an empty expected token never matches,
/// including the empty string compared with itself.
pub fn tokens_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = u8::from(a.len() != b.len() || b.is_empty());
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

pub fn token_ok(presented: &str, expected: &str) -> bool {
    if expected.len() < 16 {
        return false;
    }
    tokens_eq(presented, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_token_is_rejected() {
        assert!(!token_ok("", ""));
        assert!(!token_ok("short", "short"));
        assert!(!token_ok("0123456789abcde", "0123456789abcde"));
        let expected = "0123456789abcdef";
        assert!(token_ok(expected, expected));
        assert!(!token_ok("0123456789abcdee", expected));
    }

    #[test]
    fn empty_expected_token_never_matches() {
        assert!(!tokens_eq("", ""));
        assert!(!tokens_eq("presented", ""));
    }

    #[test]
    fn zero_padding_does_not_hide_length_mismatch() {
        assert!(!tokens_eq("secret", "secret\0"));
        assert!(!tokens_eq("secret\0", "secret"));
    }

    #[test]
    fn byte_mismatches_cannot_cancel() {
        assert!(!tokens_eq("ab", "ba"));
    }

    #[test]
    fn pairing_codes_match_without_becoming_valid_tokens() {
        assert!(tokens_eq("123456", "123456"));
        assert!(!token_ok("123456", "123456"));
    }
}
