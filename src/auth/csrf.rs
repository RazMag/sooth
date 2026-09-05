use tower_sessions::Session;

const CSRF_KEY: &str = "csrf_token";

/// A fresh, session-scoped CSRF token. Since the session cookie is already
/// `SameSite=Strict` (blocking cross-site delivery of the cookie in the
/// first place), this token is defense-in-depth rather than the sole
/// protection: a plain random value compared server-side against the
/// session's copy, no HMAC/signing needed.
pub fn generate() -> String {
    hex_encode(&rand::random::<[u8; 32]>())
}

pub async fn store(session: &Session, token: &str) -> Result<(), super::AuthError> {
    session.insert(CSRF_KEY, token).await.map_err(|e| super::AuthError::Session(e.to_string()))
}

pub async fn current(session: &Session) -> Option<String> {
    session.get::<String>(CSRF_KEY).await.ok().flatten()
}

/// Verifies a submitted token against the session's stored value in constant
/// time (so response timing can't be used to guess the token byte by byte).
pub async fn verify(session: &Session, submitted: &str) -> bool {
    match current(session).await {
        Some(expected) => constant_time_eq(expected.as_bytes(), submitted.as_bytes()),
        None => false,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn generate_produces_64_hex_chars() {
        let token = generate();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
