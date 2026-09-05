pub mod csrf;
pub mod middleware;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("session error: {0}")]
    Session(String),
}
