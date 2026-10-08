/// Shared-secret compare. Length is not returned early, and an empty expected token never matches,
/// including the empty string compared with itself.
pub fn tokens_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
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
        let expected = "0123456789abcdef";
        assert!(token_ok(expected, expected));
        assert!(!token_ok("0123456789abcdee", expected));
    }
}
