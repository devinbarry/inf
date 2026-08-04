use serde::Serialize;
use thiserror::Error;

/// One entry in an ambiguous-match error. Kept structured rather than
/// pre-formatted so the JSON error payload can carry real fields — an agent
/// disambiguating a query needs the id, not a rendered line it has to re-parse.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub name: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

impl std::fmt::Display for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.slug {
            Some(slug) => write!(f, "  {} ({})", self.name, slug),
            None => write!(f, "  {} ({})", self.name, self.id),
        }
    }
}

fn render(candidates: &[Candidate]) -> String {
    candidates
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Error, Debug)]
pub enum IfpError {
    #[error("API error: {0}")]
    Api(String),

    /// A 404 from the API. Kept distinct from `Api` because "this resource does
    /// not exist yet" is a normal, recoverable condition for some callers —
    /// notably `identity client-secret`, which attaches Universal Auth when the
    /// lookup misses. Folding it into `Api` made every other failure, including
    /// an expired session, look like an absent resource.
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Config error: {0}")]
    Config(String),

    /// The invocation itself was malformed — clap rejected it. Rendered through
    /// the same JSON envelope as everything else so an agent that generates a
    /// slightly wrong command line gets a parseable failure rather than English
    /// usage text.
    #[error("Usage error: {0}")]
    Usage(String),

    /// A destructive command was run without `--yes`. Its own kind because it
    /// is the one failure an agent can recover from mechanically, by retrying
    /// with the flag.
    #[error("{0}\nRefusing to continue without confirmation. Pass --yes to proceed.")]
    ConfirmationRequired(String),

    /// Delete protection is enabled on the target. Distinct from a generic
    /// config error because the remedy is a specific other command.
    #[error("{0}")]
    DeleteProtected(String),

    /// An update command was given no fields to change.
    #[error("nothing to update: {0}")]
    NothingToUpdate(String),

    /// An error with extra context attached. Wrapping rather than re-tagging
    /// keeps the original `kind`, exit code and structured payload intact —
    /// collapsing a mid-command `not_logged_in` into a generic API error told
    /// agents to retry when they needed to re-authenticate.
    #[error("{source}\n{note}")]
    Annotated { source: Box<IfpError>, note: String },

    /// The `infisical` CLI is not on PATH. Without it there is no session to reuse.
    #[error("`infisical` CLI not found on PATH. Install it, then run `infisical login`")]
    CliNotFound,

    /// The CLI is present but holds no usable session.
    #[error("Not logged in. Run `infisical login`")]
    NotLoggedIn,

    /// The CLI ran and failed for a reason that is not "no session" — a locked
    /// keyring, an unreadable config file. Carrying its stderr keeps the user
    /// from being sent to `infisical login` for a problem login will not fix.
    #[error("`infisical` CLI failed: {0}")]
    CliFailed(String),

    /// The CLI did not answer within the timeout. Its own kind because it is
    /// transient and worth retrying, unlike a locked keyring, which is not.
    #[error("`infisical` CLI timed out: {0}")]
    CliTimeout(String),

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

    #[error("Ambiguous match '{query}', candidates:\n{}", render(.candidates))]
    AmbiguousMatch {
        query: String,
        candidates: Vec<Candidate>,
    },

    #[error("Aborted")]
    Aborted,
}

impl IfpError {
    /// Exit code: 1 when the request itself failed — the server refused it or
    /// it could not be completed — and 2 when the invocation was wrong and the
    /// user must change something locally. A connection failure belongs in the
    /// first group: nothing about the command line would have fixed it.
    pub fn exit_code(&self) -> i32 {
        match self {
            IfpError::Api(_)
            | IfpError::NotFound(_)
            | IfpError::Forbidden(_)
            | IfpError::Http(_) => 1,
            IfpError::Annotated { source, .. } => source.exit_code(),
            _ => 2,
        }
    }

    /// Attach context while preserving the underlying failure.
    pub fn annotate(self, note: impl Into<String>) -> IfpError {
        IfpError::Annotated {
            source: Box::new(self),
            note: note.into(),
        }
    }

    /// Stable machine-readable discriminant. `inf` is driven by agents, so the
    /// failure mode has to be matchable without regexing prose that is free to
    /// be reworded.
    pub fn kind(&self) -> &'static str {
        match self {
            IfpError::Api(_) => "api_error",
            IfpError::NotFound(_) => "not_found",
            IfpError::Http(_) => "http_error",
            IfpError::Config(_) => "config_error",
            IfpError::Usage(_) => "usage_error",
            IfpError::ConfirmationRequired(_) => "confirmation_required",
            IfpError::DeleteProtected(_) => "delete_protected",
            IfpError::NothingToUpdate(_) => "nothing_to_update",
            IfpError::Annotated { source, .. } => source.kind(),
            IfpError::CliNotFound => "cli_not_found",
            IfpError::NotLoggedIn => "not_logged_in",
            IfpError::CliFailed(_) => "cli_failed",
            IfpError::CliTimeout(_) => "cli_timeout",
            IfpError::TokenExpired(_) => "token_expired",
            IfpError::Forbidden(_) => "forbidden",
            IfpError::NoMatch(_) => "no_match",
            IfpError::AmbiguousMatch { .. } => "ambiguous_match",
            IfpError::Aborted => "aborted",
        }
    }

    /// The error as the JSON object written to stderr in the default mode.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "error": self.kind(),
            "message": self.to_string(),
            "exitCode": self.exit_code(),
        });
        // Structured fields come from the innermost error, so wrapping never
        // costs an agent the data it needs to recover.
        let mut inner = self;
        while let IfpError::Annotated { source, .. } = inner {
            inner = source;
        }
        if let IfpError::AmbiguousMatch { query, candidates } = inner {
            value["query"] = serde_json::json!(query);
            value["candidates"] = serde_json::json!(candidates);
        }
        value
    }
}

pub type Result<T> = std::result::Result<T, IfpError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, id: &str, slug: Option<&str>) -> Candidate {
        Candidate {
            name: name.into(),
            id: id.into(),
            slug: slug.map(str::to_string),
        }
    }

    #[test]
    fn request_failures_exit_one() {
        assert_eq!(IfpError::Api("500".into()).exit_code(), 1);
        assert_eq!(IfpError::Forbidden("nope".into()).exit_code(), 1);
        assert_eq!(IfpError::NotFound("gone".into()).exit_code(), 1);
    }

    #[test]
    fn usage_errors_exit_two() {
        assert_eq!(IfpError::NotLoggedIn.exit_code(), 2);
        assert_eq!(IfpError::CliNotFound.exit_code(), 2);
        assert_eq!(IfpError::NoMatch("x".into()).exit_code(), 2);
        assert_eq!(IfpError::Config("bad".into()).exit_code(), 2);
        assert_eq!(IfpError::Aborted.exit_code(), 2);
    }

    #[test]
    fn not_logged_in_names_the_fix() {
        assert_eq!(
            IfpError::NotLoggedIn.to_string(),
            "Not logged in. Run `infisical login`"
        );
    }

    #[test]
    fn cli_failure_is_distinct_from_not_logged_in() {
        // A locked keyring is not a missing session; sending the user to
        // `infisical login` would not fix it.
        let err = IfpError::CliFailed("keyring is locked".into());
        assert_eq!(err.kind(), "cli_failed");
        assert!(err.to_string().contains("keyring is locked"));
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
            candidates: vec![
                candidate("Billing API", "p1", Some("billing-api")),
                candidate("Billing Web", "p2", Some("billing-web")),
            ],
        };
        let msg = err.to_string();
        assert!(msg.contains("bill"));
        assert!(msg.contains("billing-api"));
        assert!(msg.contains("billing-web"));
    }

    #[test]
    fn every_kind_is_a_stable_slug() {
        // Agents match on `kind`, so it must never be prose.
        for err in [
            IfpError::Api("x".into()),
            IfpError::NotFound("x".into()),
            IfpError::Config("x".into()),
            IfpError::CliNotFound,
            IfpError::NotLoggedIn,
            IfpError::CliFailed("x".into()),
            IfpError::TokenExpired("x".into()),
            IfpError::Forbidden("x".into()),
            IfpError::NoMatch("x".into()),
            IfpError::Aborted,
        ] {
            let kind = err.kind();
            assert!(!kind.is_empty());
            assert!(
                kind.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "kind '{kind}' is not a stable slug"
            );
        }
    }

    #[test]
    fn json_error_carries_kind_message_and_exit_code() {
        let json = IfpError::Forbidden("nope".into()).to_json();
        assert_eq!(json["error"], "forbidden");
        assert_eq!(json["exitCode"], 1);
        assert!(json["message"].as_str().unwrap().contains("nope"));
    }

    #[test]
    fn json_error_carries_structured_candidates() {
        // The whole point of structuring these: an agent picks a candidate by
        // id without re-parsing a rendered line.
        let json = IfpError::AmbiguousMatch {
            query: "bill".into(),
            candidates: vec![candidate("Billing API", "p1", Some("billing-api"))],
        }
        .to_json();
        assert_eq!(json["error"], "ambiguous_match");
        assert_eq!(json["query"], "bill");
        assert_eq!(json["candidates"][0]["id"], "p1");
        assert_eq!(json["candidates"][0]["slug"], "billing-api");
    }

    #[test]
    fn candidate_without_slug_renders_its_id() {
        let c = candidate("ci-deployer", "i1", None);
        assert_eq!(c.to_string(), "  ci-deployer (i1)");
        assert!(serde_json::to_value(&c).unwrap().get("slug").is_none());
    }
}
