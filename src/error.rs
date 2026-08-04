use thiserror::Error;

#[derive(Error, Debug)]
pub enum IfpError {
    #[error("API error: {0}")]
    Api(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Config error: {0}")]
    Config(String),

    /// The `infisical` CLI is not on PATH. Without it there is no session to reuse.
    #[error("`infisical` CLI not found on PATH. Install it, then run `infisical login`")]
    CliNotFound,

    /// The CLI is present but holds no usable session.
    #[error("Not logged in. Run `infisical login`")]
    NotLoggedIn,

    /// The CLI stores a refresh token but never uses it, so an expired session is
    /// terminal until the user logs in again. Say so plainly rather than letting
    /// the API return an opaque 401.
    #[error("Session expired {0}. Run `infisical login` to renew")]
    TokenExpired(String),

    /// Infisical returns 403 (not 401) when a route rejects the caller's auth mode.
    /// A machine identity hitting a user-JWT-only route lands here.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("No match: {0}")]
    NoMatch(String),

    #[error("Ambiguous match '{query}', candidates:\n{candidates}")]
    AmbiguousMatch { query: String, candidates: String },

    #[error("Aborted")]
    Aborted,
}

impl IfpError {
    /// Exit code: 1 for an authenticated request the server refused, 2 for
    /// CLI/usage/config problems. Mirrors the convention used by pfp.
    pub fn exit_code(&self) -> i32 {
        match self {
            IfpError::Api(_) | IfpError::Forbidden(_) => 1,
            _ => 2,
        }
    }
}

pub type Result<T> = std::result::Result<T, IfpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_errors_exit_one() {
        assert_eq!(IfpError::Api("500".into()).exit_code(), 1);
        assert_eq!(IfpError::Forbidden("nope".into()).exit_code(), 1);
    }

    #[test]
    fn usage_errors_exit_two() {
        assert_eq!(IfpError::NotLoggedIn.exit_code(), 2);
        assert_eq!(IfpError::CliNotFound.exit_code(), 2);
        assert_eq!(IfpError::NoMatch("x".into()).exit_code(), 2);
        assert_eq!(IfpError::Config("bad".into()).exit_code(), 2);
    }

    #[test]
    fn not_logged_in_names_the_fix() {
        assert_eq!(
            IfpError::NotLoggedIn.to_string(),
            "Not logged in. Run `infisical login`"
        );
    }

    #[test]
    fn expired_message_carries_when_and_the_fix() {
        let err = IfpError::TokenExpired("3 days ago".into());
        let msg = err.to_string();
        assert!(msg.contains("3 days ago"));
        assert!(msg.contains("infisical login"));
    }

    #[test]
    fn ambiguous_match_lists_candidates() {
        let err = IfpError::AmbiguousMatch {
            query: "bill".into(),
            candidates: "  billing-api\n  billing-web".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("bill"));
        assert!(msg.contains("billing-api"));
        assert!(msg.contains("billing-web"));
    }
}
