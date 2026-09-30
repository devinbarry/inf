use crate::error::{IfpError, Result};
use base64::Engine;
use serde::Deserialize;
use std::path::PathBuf;

/// Resolved session: where to talk to, what to send, and who we are.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL including the `/api` suffix, e.g. `https://app.infisical.com/api`.
    pub api_url: String,
    pub token: String,
    /// Organization the token is scoped to, read from the JWT payload.
    /// Every org-scoped route needs this and the API will not infer it for us.
    pub org_id: String,
    /// Unix expiry from the JWT, surfaced so `inf whoami` can warn before a
    /// session lapses rather than after.
    pub expires_at: Option<i64>,
}

/// The subset of `~/.infisical/infisical-config.json` we care about.
/// The file deliberately holds no token — that lives in the OS keyring — so we
/// only read it to discover which instance the user last logged into.
#[derive(Deserialize, Default)]
struct InfisicalConfigFile {
    #[serde(rename = "LoggedInUserDomain")]
    logged_in_user_domain: Option<String>,
}

#[derive(Deserialize)]
struct JwtPayload {
    #[serde(rename = "organizationId")]
    organization_id: Option<String>,
    #[serde(rename = "authTokenType")]
    auth_token_type: Option<String>,
    exp: Option<i64>,
}

const DEFAULT_DOMAIN: &str = "https://app.infisical.com/api";

impl Config {
    pub async fn load(domain_override: Option<&str>) -> Result<Self> {
        let token = Self::read_cli_token().await?;
        let payload = Self::decode_payload(&token)?;

        // The CLI keeps a RefreshToken field but never uses it, so an expired
        // session cannot be recovered here. Fail with the fix instead of
        // letting the server answer with a bare 401.
        if let Some(exp) = payload.exp {
            let now = Self::now_unix();
            if exp <= now {
                return Err(IfpError::TokenExpired(Self::describe_age(now - exp)));
            }
        }

        // A token minted by `infisical login --method universal-auth` is an
        // identity token. Rejecting it here is `inf`'s own policy, not a
        // statement about what the API would accept: this tool reads the
        // organization id out of the JWT payload and its whole session model is
        // the user login, so a machine-identity token is outside what it is
        // built to drive. (Older backends also rejected identity tokens on the
        // identity routes outright; current upstream accepts them on some. The
        // policy stands on the first reason, not the second.)
        if let Some(kind) = payload.auth_token_type.as_deref() {
            if kind != "accessToken" {
                return Err(IfpError::Config(format!(
                    "session is a '{kind}' token, but inf drives the API as a logged-in user.\n\
                     Run `infisical login` (method: user) rather than a machine-identity login"
                )));
            }
        }

        let org_id = payload.organization_id.ok_or_else(|| {
            IfpError::Config(
                "session token is not scoped to an organization. Run `infisical login` and select one"
                    .into(),
            )
        })?;

        Ok(Self {
            api_url: Self::resolve_domain(domain_override)?,
            token,
            org_id,
            expires_at: payload.exp,
        })
    }

    /// Whole days until the session lapses. Negative values are impossible
    /// here because `load` rejects an already-expired token.
    pub fn days_until_expiry(&self) -> Option<i64> {
        self.expires_at.map(|exp| (exp - Self::now_unix()) / 86400)
    }

    /// How long to wait for the CLI before giving up. It normally answers in
    /// milliseconds; the ceiling exists because a wedged keyring agent would
    /// otherwise hang every `inf` invocation forever with no output at all —
    /// the worst possible failure for a caller that cannot see a terminal.
    /// Overridable for a genuinely slow credential backend.
    fn cli_timeout() -> std::time::Duration {
        let secs = std::env::var("INF_CLI_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(30);
        std::time::Duration::from_secs(secs)
    }

    /// Shell out rather than read the keyring directly. Infisical vendors a
    /// *forked* go-keyring whose API differs from upstream, and its `file`
    /// backend derives the passphrase from the config file; reimplementing
    /// either would couple us to internals that are free to change. The CLI
    /// owns credential storage; request deliberate private delivery for both backends.
    async fn read_cli_token() -> Result<String> {
        let timeout = Self::cli_timeout();
        // stdin is closed: if the CLI ever decides to prompt (for a keyring
        // passphrase, say) it must fail rather than block on a question no
        // agent is there to answer.
        let spawned = crate::credential_pipe::capture_session();

        let (output, token_bytes) = tokio::time::timeout(timeout, spawned)
            .await
            .map_err(|_| {
                IfpError::CliTimeout(format!(
                    "`infisical user get token` did not finish within {}s. \
                     Check the credential store, or raise INF_CLI_TIMEOUT_SECS",
                    timeout.as_secs()
                ))
            })?
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    IfpError::CliNotFound
                } else {
                    IfpError::Config(format!("failed to run `infisical`: {e}"))
                }
            })?;

        if !output.status.success() {
            // Not every failure is a missing session: a locked keyring or an
            // unreadable config file also exit non-zero. Sending those users to
            // `infisical login` wastes their time, so keep the CLI's own
            // failure kind, but never forward arbitrary captured stderr.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            return Err(if detail.is_empty() || Self::mentions_login(detail) {
                IfpError::NotLoggedIn
            } else {
                IfpError::CliFailed(
                    "Credential source failed; inspect infisical login status for safe diagnostics"
                        .into(),
                )
            });
        }

        let stdout = String::from_utf8_lossy(&token_bytes);
        Self::extract_jwt(&stdout).ok_or(IfpError::NotLoggedIn)
    }

    fn mentions_login(stderr: &str) -> bool {
        let lowered = stderr.to_lowercase();
        lowered.contains("logged in") || lowered.contains("login")
    }

    /// The CLI interleaves migration notices with command output, and `--silent`
    /// has not always suppressed them. Pick the line that is actually a JWT
    /// rather than trusting stdout to hold nothing else.
    ///
    /// Counting dots is not enough: `https://artifacts.infisical.com` is three
    /// non-empty dot-separated segments too, and being *first* it used to win —
    /// so a banner containing any URL broke every command with a bogus
    /// "cannot decode session token". A JWT's segments are base64url and its
    /// header decodes to a JSON object naming an algorithm; check that instead.
    fn extract_jwt(stdout: &str) -> Option<String> {
        stdout
            .lines()
            .map(str::trim)
            .find(|line| Self::is_jwt(line))
            .map(str::to_string)
    }

    fn is_jwt(candidate: &str) -> bool {
        let segments: Vec<&str> = candidate.split('.').collect();
        if segments.len() != 3 {
            return false;
        }
        if !segments
            .iter()
            .all(|seg| !seg.is_empty() && seg.bytes().all(Self::is_base64url_byte))
        {
            return false;
        }
        // The header is the cheapest segment to validate and the only one whose
        // shape is fixed by the JWT spec.
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segments[0])
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|header| header.get("alg").and_then(|a| a.as_str()).is_some())
    }

    fn is_base64url_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
    }

    fn decode_payload(token: &str) -> Result<JwtPayload> {
        let segment = token
            .split('.')
            .nth(1)
            .ok_or_else(|| IfpError::Config("session token is not a JWT".into()))?;

        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segment)
            .map_err(|e| IfpError::Config(format!("cannot decode session token: {e}")))?;

        serde_json::from_slice(&bytes)
            .map_err(|e| IfpError::Config(format!("cannot parse session token: {e}")))
    }

    /// Precedence: explicit flag, then env, then the instance the CLI last used.
    /// The flag exists so a single shell can address two instances without
    /// re-running `infisical login`.
    fn resolve_domain(override_value: Option<&str>) -> Result<String> {
        if let Some(domain) = override_value {
            return Ok(Self::normalize_domain(domain));
        }
        if let Ok(domain) = std::env::var("INFISICAL_DOMAIN") {
            if !domain.trim().is_empty() {
                return Ok(Self::normalize_domain(&domain));
            }
        }
        let file = Self::read_config_file().unwrap_or_default();
        Ok(Self::normalize_domain(
            file.logged_in_user_domain
                .as_deref()
                .unwrap_or(DEFAULT_DOMAIN),
        ))
    }

    /// The CLI stores the domain with `/api` already appended, but `--domain`
    /// and `INFISICAL_DOMAIN` are documented without it. Accept both so the
    /// same string works wherever the user copied it from.
    fn normalize_domain(domain: &str) -> String {
        let trimmed = domain.trim().trim_end_matches('/');
        if trimmed.ends_with("/api") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/api")
        }
    }

    fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".infisical")
            .join("infisical-config.json")
    }

    fn read_config_file() -> Result<InfisicalConfigFile> {
        let path = Self::config_path();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| IfpError::Config(format!("cannot read {}: {e}", path.display())))?;
        serde_json::from_str(&content)
            .map_err(|e| IfpError::Config(format!("cannot parse {}: {e}", path.display())))
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn describe_age(seconds: i64) -> String {
        match seconds {
            s if s < 60 => "just now".to_string(),
            s if s < 3600 => format!("{} minutes ago", s / 60),
            s if s < 86400 => format!("{} hours ago", s / 3600),
            s => format!("{} days ago", s / 86400),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn b64(value: &serde_json::Value) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    fn make_token(payload: &serde_json::Value) -> String {
        let header = b64(&serde_json::json!({"alg": "HS256", "typ": "JWT"}));
        format!("{header}.{}.c2lnbmF0dXJl", b64(payload))
    }

    #[test]
    fn normalize_domain_appends_api_when_absent() {
        assert_eq!(
            Config::normalize_domain("https://infisical.example.com"),
            "https://infisical.example.com/api"
        );
    }

    #[test]
    fn normalize_domain_preserves_existing_api_suffix() {
        assert_eq!(
            Config::normalize_domain("https://infisical.example.com/api"),
            "https://infisical.example.com/api"
        );
    }

    #[test]
    fn normalize_domain_tolerates_trailing_slash() {
        assert_eq!(
            Config::normalize_domain("https://infisical.example.com/api/"),
            "https://infisical.example.com/api"
        );
        assert_eq!(
            Config::normalize_domain("  https://eu.infisical.com/  "),
            "https://eu.infisical.com/api"
        );
    }

    #[test]
    fn extract_jwt_ignores_migration_notices() {
        let token = make_token(&serde_json::json!({"organizationId": "org-1"}));
        let stdout = format!(
            "Important: the Infisical CLI package repository is moving.\n\
             What to do: repoint your machine to the new host.\n\
             {token}\n"
        );
        assert_eq!(
            Config::extract_jwt(&stdout).as_deref(),
            Some(token.as_str())
        );
    }

    #[test]
    fn extract_jwt_skips_a_url_printed_before_the_token() {
        // A URL is three non-empty dot-separated segments, so a dot count picks
        // the banner instead of the token — and because it comes first, every
        // command then failed with "cannot decode session token" for a user who
        // was correctly logged in.
        let token = make_token(&serde_json::json!({"organizationId": "org-1"}));
        let stdout = format!(
            "Important: repoint your machine to https://artifacts-p.infisical.com\n{token}\n"
        );
        assert_eq!(
            Config::extract_jwt(&stdout).as_deref(),
            Some(token.as_str())
        );
    }

    #[test]
    fn extract_jwt_rejects_output_without_a_token() {
        assert!(Config::extract_jwt("no token here\njust noise\n").is_none());
        // A sentence with two full stops must not be mistaken for a JWT.
        assert!(Config::extract_jwt("a.b.\n").is_none());
        // Nor a bare hostname, version string, or IP address.
        assert!(Config::extract_jwt("see docs.infisical.com\n").is_none());
        assert!(Config::extract_jwt("infisical v0.43.116\n").is_none());
        assert!(Config::extract_jwt("connect to 192.0.2.10\n").is_none());
        // Three base64url-looking segments whose header is not a JWT header.
        assert!(Config::extract_jwt("aaa.bbb.ccc\n").is_none());
    }

    #[test]
    fn is_jwt_requires_an_alg_in_the_header() {
        let payload = b64(&serde_json::json!({"organizationId": "org-1"}));
        let headerless = b64(&serde_json::json!({"typ": "JWT"}));
        assert!(!Config::is_jwt(&format!("{headerless}.{payload}.sig")));

        let header = b64(&serde_json::json!({"alg": "HS256", "typ": "JWT"}));
        assert!(Config::is_jwt(&format!("{header}.{payload}.sig")));
    }

    #[test]
    fn decode_payload_reads_org_and_expiry() {
        let token = make_token(&serde_json::json!({
            "organizationId": "org-123",
            "authTokenType": "accessToken",
            "exp": 1786712129i64,
        }));
        let payload = Config::decode_payload(&token).unwrap();
        assert_eq!(payload.organization_id.as_deref(), Some("org-123"));
        assert_eq!(payload.auth_token_type.as_deref(), Some("accessToken"));
        assert_eq!(payload.exp, Some(1786712129));
    }

    #[test]
    fn decode_payload_rejects_non_jwt() {
        assert!(Config::decode_payload("not-a-jwt").is_err());
    }

    #[test]
    fn describe_age_scales_units() {
        assert_eq!(Config::describe_age(30), "just now");
        assert_eq!(Config::describe_age(600), "10 minutes ago");
        assert_eq!(Config::describe_age(7200), "2 hours ago");
        assert_eq!(Config::describe_age(172_800), "2 days ago");
    }
}
