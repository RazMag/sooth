pub mod csrf;
pub mod middleware;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("session error: {0}")]
    Session(String),
}

/// Hashes a plaintext password with argon2's defaults, returning a PHC
/// string suitable for `auth_password_hash` / `SOOTH_AUTH_PASSWORD_HASH`.
/// Shared by the `--hash-password` CLI and the Settings "change password"
/// form so the two never drift.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::PasswordHasher;

    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to hash password: {e}"))?;
    Ok(hash.to_string())
}

/// Constant-time verify of `candidate` against a stored argon2 PHC hash.
/// `false` (never an error) when the stored hash is unparseable or the
/// password simply doesn't match -- callers treat both as "denied".
pub fn verify_password(stored_hash: &str, candidate: &str) -> bool {
    use argon2::password_hash::PasswordVerifier;

    match argon2::PasswordHash::new(stored_hash) {
        Ok(hash) => argon2::Argon2::default()
            .verify_password(candidate.as_bytes(), &hash)
            .is_ok(),
        Err(_) => false,
    }
}
