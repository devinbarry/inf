//! End-to-end tests for the `inf` binary.
//!
//! These exist so the CLI's actual behaviour — what lands on stdout, what lands
//! on stderr, and the exit code — can be checked without a real Infisical
//! instance. Verifying any of this by running the binary against a live login
//! mutates whatever organization the caller happens to be logged into, which is
//! never acceptable for a test.
//!
//! Two things are faked: a shim named `infisical` on `PATH` that prints a
//! valid-looking session token, and a mock HTTP server the binary talks to via
//! `INFISICAL_DOMAIN`. Both are per-test and leave nothing behind.
//!
//! Unlike the unit tests these live in `tests/` because they need the compiled
//! binary, which Cargo exposes only to integration tests.

use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Far enough out that the session never expires mid-test, and fixed so the
/// tests do not depend on the clock.
const FAR_FUTURE: i64 = 4_102_444_800; // 2100-01-01

fn b64(value: &serde_json::Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
}

fn session_token() -> String {
    let header = b64(&serde_json::json!({"alg": "HS256", "typ": "JWT"}));
    let payload = b64(&serde_json::json!({
        "organizationId": "org-test",
        "authTokenType": "accessToken",
        "exp": FAR_FUTURE,
    }));
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

/// A stand-in for the official CLI. It prints a banner before the token on
/// purpose: `--silent` has not always suppressed those, and a banner containing
/// a URL used to be mistaken for the token itself.
fn install_shim(dir: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         echo 'Important: repoint your machine to https://artifacts-p.infisical.com'\n\
         echo '{}' >&3\n",
        session_token()
    );
    let path = dir.join("infisical");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A shim that never returns, standing in for a wedged keyring agent.
///
/// `exec` matters: dash does not exec-optimise the last command of a script, so
/// a plain `sleep 300` would be a *grandchild*. `kill_on_drop` reaches only the
/// direct child, so the sleeper would survive the test, be reparented to init,
/// and linger for five minutes. `exec` makes the sleeper the process we spawn,
/// which also makes this test assert the stronger thing.
fn install_hanging_shim(dir: &Path) {
    let path = dir.join("infisical");
    std::fs::write(&path, "#!/bin/sh\nexec sleep 300\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

struct Run {
    shim_dir: tempfile::TempDir,
    api_url: String,
    timeout_secs: Option<u64>,
    http_timeout_secs: Option<u64>,
}

impl Run {
    fn new(api_url: &str) -> Self {
        let shim_dir = tempfile::tempdir().unwrap();
        install_shim(shim_dir.path());
        Self {
            shim_dir,
            api_url: api_url.to_string(),
            timeout_secs: None,
            http_timeout_secs: None,
        }
    }

    /// A run whose `infisical` never answers, with the wait shortened so the
    /// test does not sit on the 30s production default.
    fn hanging() -> Self {
        let shim_dir = tempfile::tempdir().unwrap();
        install_hanging_shim(shim_dir.path());
        Self {
            shim_dir,
            api_url: "http://127.0.0.1:1".to_string(),
            timeout_secs: Some(1),
            http_timeout_secs: None,
        }
    }

    fn exec(&self, args: &[&str]) -> Output {
        // PATH and INFISICAL_DOMAIN are set on the child only, so tests stay
        // independent and never touch the parent process environment.
        let path = format!(
            "{}:{}",
            self.shim_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(bin());
        command
            .args(args)
            .env("PATH", path)
            .env("INFISICAL_DOMAIN", &self.api_url)
            // Colour codes would otherwise depend on whether the test harness
            // is attached to a terminal.
            .env("NO_COLOR", "1");
        if let Some(secs) = self.timeout_secs {
            command.env("INF_CLI_TIMEOUT_SECS", secs.to_string());
        }
        if let Some(secs) = self.http_timeout_secs {
            command.env("INF_HTTP_TIMEOUT_SECS", secs.to_string());
        }
        command.output().expect("failed to run inf")
    }
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_inf"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|e| {
        panic!(
            "stdout was not valid JSON ({e}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

fn stderr_json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stderr(output))
        .unwrap_or_else(|e| panic!("stderr was not valid JSON ({e}):\n{}", stderr(output)))
}

const TWO_PROJECTS: &str = r#"{"projects":[
    {"id":"p1","name":"billing","slug":"billing","type":"secret-manager",
     "environments":[{"id":"e1","name":"Development","slug":"dev"}]},
    {"id":"p2","name":"payments","slug":"payments","type":"ssh","environments":[]}
]}"#;

#[tokio::test]
async fn default_output_is_json_on_stdout_and_nothing_else() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "ls"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = stdout_json(&output);
    assert!(json.is_array());
    assert_eq!(json[0]["slug"], "billing");
    // No banner, no table, no status line — the whole point of the default mode.
    assert!(
        stderr(&output).is_empty(),
        "stderr should be empty on success, got: {}",
        stderr(&output)
    );
}

#[tokio::test]
async fn the_documented_jq_pipeline_works() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let json = stdout_json(&run.exec(&["project", "ls"]));

    // README teaches: inf project ls | jq -r '.[] | select(.type=="ssh") | .slug'
    let ssh: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["type"] == "ssh")
        .map(|p| p["slug"].as_str().unwrap())
        .collect();
    assert_eq!(ssh, vec!["payments"]);
}

#[tokio::test]
async fn human_mode_prints_a_table_and_no_json() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "ls", "--human"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("NAME"), "expected a table header: {text}");
    assert!(text.contains("billing"));
    assert!(
        serde_json::from_str::<serde_json::Value>(&text).is_err(),
        "human mode must not emit JSON"
    );
}

#[tokio::test]
async fn errors_are_json_on_stderr_with_a_stable_kind() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(403)
        .with_body(r#"{"message":"Insufficient permission"}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "ls"]);

    assert_eq!(output.status.code(), Some(1));
    let json = stderr_json(&output);
    assert_eq!(json["error"], "forbidden");
    assert_eq!(json["exitCode"], 1);
    assert!(json["message"].as_str().unwrap().contains("Insufficient"));
    // stdout must stay clean so a failed run does not corrupt a jq pipeline.
    assert!(stdout(&output).is_empty());
}

#[tokio::test]
async fn ambiguous_match_returns_structured_candidates() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(
            r#"{"projects":[
                {"id":"p1","name":"Billing API","slug":"billing-api","environments":[]},
                {"id":"p2","name":"Billing Web","slug":"billing-web","environments":[]}
            ]}"#,
        )
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "get", "billing"]);

    assert_eq!(output.status.code(), Some(2));
    let json = stderr_json(&output);
    assert_eq!(json["error"], "ambiguous_match");
    assert_eq!(json["query"], "billing");
    // An agent picks one of these by id and retries — no prose parsing.
    let ids: Vec<&str> = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["p1", "p2"]);
}

#[tokio::test]
async fn destructive_commands_refuse_without_yes_and_never_hang() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1")
        .with_status(200)
        .with_body(r#"{"project":{"id":"p1","name":"billing","slug":"billing","environments":[]}}"#)
        .create_async()
        .await;
    // Deliberately no DELETE mock: reaching it would fail the test loudly.
    let deleted = server
        .mock("DELETE", "/api/v1/projects/p1")
        .with_status(200)
        .expect(0)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "delete", "billing"]);

    assert_eq!(output.status.code(), Some(2));
    let json = stderr_json(&output);
    assert_eq!(json["error"], "confirmation_required");
    assert!(json["message"].as_str().unwrap().contains("--yes"));
    deleted.assert_async().await;
}

#[tokio::test]
async fn delete_protection_is_checked_against_the_detail_route() {
    let mut server = mockito::Server::new_async().await;
    // The listing omits `hasDeleteProtection`; only the detail route has it.
    // Reading the guard off the listing skipped it exactly when it mattered.
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(
            r#"{"projects":[{"id":"p1","name":"billing","slug":"billing","environments":[]}]}"#,
        )
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1")
        .with_status(200)
        .with_body(
            r#"{"project":{"id":"p1","name":"billing","slug":"billing",
               "hasDeleteProtection":true,"environments":[]}}"#,
        )
        .create_async()
        .await;
    let deleted = server
        .mock("DELETE", "/api/v1/projects/p1")
        .with_status(200)
        .expect(0)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "delete", "billing", "--yes"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("delete protection"));
    deleted.assert_async().await;
}

#[tokio::test]
async fn delete_emits_json_so_it_is_scriptable() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1")
        .with_status(200)
        .with_body(r#"{"project":{"id":"p1","name":"billing","slug":"billing","environments":[]}}"#)
        .create_async()
        .await;
    server
        .mock("DELETE", "/api/v1/projects/p1")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "delete", "billing", "--yes"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = stdout_json(&output);
    assert_eq!(json["deleted"], true);
    assert_eq!(json["id"], "p1");
}

#[tokio::test]
async fn a_banner_before_the_token_does_not_break_the_session() {
    // The shim always prints a URL-bearing banner first. Counting dots picked
    // the banner, and every command then died with "cannot decode session
    // token" for a user who was correctly logged in.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "ls"]);
    assert!(
        output.status.success(),
        "banner broke session parsing: {}",
        stderr(&output)
    );
}

#[tokio::test]
async fn whoami_is_answered_without_touching_the_api() {
    // Nothing is mocked: whoami must be served entirely from the decoded
    // session, so it keeps working while the instance is unreachable.
    let run = Run::new("http://127.0.0.1:1");
    let output = run.exec(&["whoami"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = stdout_json(&output);
    assert_eq!(json["organizationId"], "org-test");
    assert_eq!(json["expiresAt"], FAR_FUTURE);
}

#[tokio::test]
async fn network_failure_is_a_request_failure_not_a_usage_error() {
    let run = Run::new("http://127.0.0.1:1");
    let output = run.exec(&["project", "ls"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused connection is not a usage error"
    );
    assert_eq!(stderr_json(&output)["error"], "http_error");
}

#[tokio::test]
async fn identity_listings_are_paginated_not_truncated_at_twenty() {
    let mut server = mockito::Server::new_async().await;
    // 25 identities: an unpaginated call returns the server's default page of
    // 20 and the last five silently resolve as "no match".
    let identities: Vec<String> = (0..25)
        .map(|n| {
            format!(
                r#"{{"identityId":"i{n}","role":"viewer","identity":{{"id":"i{n}","name":"identity-{n:02}","authMethods":[]}}}}"#
            )
        })
        .collect();
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::UrlEncoded("limit".into(), "1000".into()))
        .with_status(200)
        .with_body(format!(r#"{{"identities":[{}]}}"#, identities.join(",")))
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "ls"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = stdout_json(&output);
    assert_eq!(json.as_array().unwrap().len(), 25);
    // The 21st onward are exactly the ones a default page would have dropped.
    assert_eq!(json[24]["name"], "identity-24");
}

#[tokio::test]
async fn identity_listings_expose_a_computed_role_on_both_routes() {
    let mut server = mockito::Server::new_async().await;
    // The project route sends `roles[]` with the unused key nulled; the org
    // route sends a scalar `role`. A script must not have to know which.
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1/memberships/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identityMemberships":[{"identityId":"i1",
               "roles":[{"role":"viewer","customRoleSlug":null}],
               "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
        )
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let json = stdout_json(&run.exec(&["identity", "ls", "--project", "billing"]));

    assert_eq!(json[0]["role"], "viewer");
    assert_eq!(json[0]["id"], "i1");
}

#[tokio::test]
async fn renaming_a_project_scoped_identity_uses_the_project_route() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1/memberships/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identityMemberships":[{"identityId":"i1","roles":[{"role":"viewer"}],
               "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
        )
        .create_async()
        .await;
    // The org route rejects project-scoped identities with
    // "Identity is managed by project", so it must never be called here.
    let org_route = server
        .mock("PATCH", "/api/v1/identities/i1")
        .with_status(400)
        .expect(0)
        .create_async()
        .await;
    let project_route = server
        .mock("PATCH", "/api/v1/projects/p1/identities/i1")
        .with_status(200)
        .with_body(r#"{"identity":{"id":"i1","name":"ci-renamed"}}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&[
        "identity",
        "rename",
        "ci",
        "ci-renamed",
        "--project",
        "billing",
    ]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    project_route.assert_async().await;
    org_route.assert_async().await;
    assert_eq!(stdout_json(&output)["name"], "ci-renamed");
}

#[tokio::test]
async fn a_failed_role_assignment_unwinds_the_new_identity() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("POST", "/api/v1/projects/p1/identities")
        .with_status(200)
        .with_body(r#"{"identity":{"id":"i9","name":"ci"}}"#)
        .create_async()
        .await;
    // A typo'd role is rejected only by the server, after the identity exists.
    server
        .mock("PATCH", "/api/v1/projects/p1/memberships/identities/i9")
        .with_status(400)
        .with_body(r#"{"message":"Invalid role"}"#)
        .create_async()
        .await;
    let cleanup = server
        .mock("DELETE", "/api/v1/projects/p1/identities/i9")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&[
        "identity",
        "create",
        "--name",
        "ci",
        "--project",
        "billing",
        "--role",
        "admn",
    ]);

    assert_eq!(output.status.code(), Some(1));
    // Without the unwind, a retry creates a second "ci" and every later
    // command resolving by name becomes ambiguous.
    cleanup.assert_async().await;
    let json = stderr_json(&output);
    assert!(json["message"].as_str().unwrap().contains("400"));
    assert!(json["message"].as_str().unwrap().contains("removed"));
}

#[tokio::test]
async fn client_secret_does_not_mistake_an_expired_session_for_a_missing_method() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identities":[{"identityId":"i1","role":"admin",
               "identity":{"id":"i1","name":"ci","authMethods":["universal-auth"]}}]}"#,
        )
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(401)
        .with_body(r#"{"message":"token expired"}"#)
        .create_async()
        .await;
    // Falling back on *any* error attached UA to an identity that already had
    // it, and reported "already configured" for what was really a dead session.
    let attach = server
        .mock("POST", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(400)
        .expect(0)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&[
        "identity",
        "client-secret",
        "ci",
        "--reveal",
        "--acknowledge-secret-exposure",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr_json(&output)["error"], "not_logged_in");
    attach.assert_async().await;
}

#[tokio::test]
async fn identity_get_reports_universal_auth_for_a_project_scoped_identity() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    // The project membership route reports authMethods: [] even when Universal
    // Auth is attached. Gating the lookup on that field made clientId
    // permanently null for the only kind of identity `inf` creates.
    server
        .mock("GET", "/api/v1/projects/p1/memberships/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identityMemberships":[{"identityId":"i1","roles":[{"role":"viewer"}],
               "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
        )
        .create_async()
        .await;
    let ua = server
        .mock("GET", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(200)
        .with_body(r#"{"identityUniversalAuth":{"clientId":"cid-123","accessTokenTTL":2592000}}"#)
        .expect(1)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "get", "ci", "--project", "billing"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    ua.assert_async().await;
    let json = stdout_json(&output);
    assert_eq!(json["clientId"], "cid-123");
    assert_eq!(json["role"], "viewer");
    assert_eq!(json["scope"], "project");
}

#[tokio::test]
async fn a_closed_stdout_does_not_panic() {
    // `inf project ls | head -1` is an ordinary agent invocation. Rust ignores
    // SIGPIPE by default, which turned the closed pipe into an EPIPE that
    // `println!` answered with a panic: exit 101 — outside the documented set —
    // and a Rust backtrace on stderr, which is not JSON.
    let mut server = mockito::Server::new_async().await;
    // Enough projects that the pretty-printed array comfortably exceeds the
    // 64 KiB pipe buffer. With a two-row payload the whole response fits in the
    // buffer, the write never fails, and the test proves nothing.
    let many: Vec<String> = (0..5000)
        .map(|n| {
            format!(
                r#"{{"id":"p{n}","name":"project-{n}","slug":"project-{n}","environments":[]}}"#
            )
        })
        .collect();
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(format!(r#"{{"projects":[{}]}}"#, many.join(",")))
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let path = format!(
        "{}:{}",
        run.shim_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("'{}' project ls | head -1", bin().display()))
        .env("PATH", path)
        .env("INFISICAL_DOMAIN", &run.api_url)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run pipeline");

    // Prove the command actually ran and emitted JSON. Without this the
    // assertions below are satisfied by an early config error that merely
    // failed to panic.
    assert_eq!(
        stdout(&output).trim(),
        "[",
        "pipeline produced no JSON; stderr: {}",
        stderr(&output)
    );
    let err = stderr(&output);
    assert!(
        !err.contains("panicked"),
        "closed stdout panicked instead of exiting: {err}"
    );
    assert!(
        !err.contains("Broken pipe"),
        "leaked an EPIPE message: {err}"
    );
}

#[tokio::test]
async fn a_half_open_socket_times_out_instead_of_hanging() {
    // A refused connection fails instantly; a server that ACCEPTs and then
    // never answers does not. On Linux that socket can sit for ~2 hours before
    // keepalive notices — no stdout, no stderr, no exit code, nothing for a
    // caller to branch on. This is the same failure class the `infisical`
    // subprocess timeout closes, on the path that is actually hot.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = std::thread::spawn(move || {
        // Accept and hold the connection open without replying.
        let _conn = listener.accept();
        // Outlive the client's 1s timeout without slowing the suite.
        std::thread::sleep(std::time::Duration::from_secs(3));
    });

    let mut run = Run::new(&format!("http://{addr}"));
    run.http_timeout_secs = Some(1);

    let started = std::time::Instant::now();
    let output = run.exec(&["project", "ls"]);
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "command did not give up promptly: {elapsed:?}"
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_json(&output)["error"], "http_timeout");
    assert!(stdout(&output).is_empty());
    let _ = accepted.join();
}

#[tokio::test]
async fn a_wedged_infisical_cli_times_out_instead_of_hanging() {
    // Without a timeout a stuck credential backend hung every inf invocation
    // forever, producing no output at all — the worst failure for a caller that
    // cannot see a terminal and has nothing to branch on.
    let run = Run::hanging();
    let started = std::time::Instant::now();
    let output = run.exec(&["project", "ls"]);
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "command did not give up promptly: {elapsed:?}"
    );
    // Exit 1, not 2: nothing about the command line would have helped, and the
    // remedy is to retry — the same rule that puts a refused connection in 1.
    assert_eq!(output.status.code(), Some(1));
    let json = stderr_json(&output);
    assert_eq!(json["error"], "cli_timeout");
    assert!(stdout(&output).is_empty());
}

#[tokio::test]
async fn identity_get_fails_when_the_universal_auth_lookup_errors() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identities":[{"identityId":"i1","role":"admin",
               "identity":{"id":"i1","name":"ci","authMethods":["universal-auth"]}}],"totalCount":1}"#,
        )
        .create_async()
        .await;
    // A 500 is not "no client id" — reporting clientId: null with exit 0 would
    // be a silent wrong answer for the one field this command exists to give.
    server
        .mock("GET", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(500)
        .with_body(r#"{"message":"internal error"}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "get", "ci"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_json(&output)["error"], "api_error");
    assert!(stdout(&output).is_empty());
}

#[tokio::test]
async fn identity_get_treats_a_404_as_no_universal_auth() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identities":[{"identityId":"i1","role":"admin",
               "identity":{"id":"i1","name":"ci","authMethods":[]}}],"totalCount":1}"#,
        )
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(404)
        .with_body(r#"{"message":"not configured"}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "get", "ci"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = stdout_json(&output);
    // A missing method is a real result, not an error: the command succeeds and
    // reports the absence.
    assert!(json["clientId"].is_null());
    assert!(json["universalAuth"].is_null());
}

#[tokio::test]
async fn project_identities_matches_the_identity_ls_shape() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1/memberships/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identityMemberships":[{"identityId":"i1",
               "roles":[{"role":"admin","customRoleSlug":null}],
               "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
        )
        .expect(2)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let via_project = stdout_json(&run.exec(&["project", "identities", "billing"]));
    let via_identity = stdout_json(&run.exec(&["identity", "ls", "--project", "billing"]));

    // The README invites `jq '.[] | select(.role=="admin") | .name'` on identity
    // listings; emitting the raw membership here returned null for both fields.
    assert_eq!(via_project[0]["role"], "admin");
    assert_eq!(via_project[0]["name"], "ci");
    assert_eq!(via_project[0]["id"], "i1");
    assert_eq!(via_project, via_identity);
}

#[tokio::test]
async fn deleting_a_project_scoped_identity_uses_the_project_route() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1/memberships/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identityMemberships":[{"identityId":"i1","roles":[{"role":"viewer"}],
               "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
        )
        .create_async()
        .await;
    // The org route rejects project-scoped identities; revocation is the
    // documented purpose of this command, so the branch must be pinned.
    let org_route = server
        .mock("DELETE", "/api/v1/identities/i1")
        .with_status(400)
        .expect(0)
        .create_async()
        .await;
    let project_route = server
        .mock("DELETE", "/api/v1/projects/p1/identities/i1")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "delete", "ci", "--project", "billing", "--yes"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    project_route.assert_async().await;
    org_route.assert_async().await;
    let json = stdout_json(&output);
    assert_eq!(json["deleted"], true);
    assert_eq!(json["scope"], "project");
}

#[tokio::test]
async fn deleting_an_org_identity_uses_the_org_route() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identities":[{"identityId":"i1","role":"admin",
               "identity":{"id":"i1","name":"legacy","authMethods":[]}}],"totalCount":1}"#,
        )
        .create_async()
        .await;
    let org_route = server
        .mock("DELETE", "/api/v1/identities/i1")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "delete", "legacy", "--yes"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    org_route.assert_async().await;
    assert_eq!(stdout_json(&output)["scope"], "organization");
}

#[tokio::test]
async fn client_secret_attaches_universal_auth_when_absent() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/identities")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_body(
            r#"{"identities":[{"identityId":"i1","role":"admin",
               "identity":{"id":"i1","name":"ci","authMethods":[]}}],"totalCount":1}"#,
        )
        .create_async()
        .await;
    // 404 is the one error that means "not attached yet" — the whole reason the
    // NotFound variant exists.
    server
        .mock("GET", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(404)
        .with_body(r#"{"message":"not configured"}"#)
        .create_async()
        .await;
    let attach = server
        .mock("POST", "/api/v1/auth/universal-auth/identities/i1")
        .with_status(200)
        .with_body(r#"{"identityUniversalAuth":{"clientId":"cid-9"}}"#)
        .expect(1)
        .create_async()
        .await;
    server
        .mock(
            "POST",
            "/api/v1/auth/universal-auth/identities/i1/client-secrets",
        )
        .with_status(200)
        .with_body(r#"{"clientSecret":"sk-secret","clientSecretData":{"id":"cs1"}}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("credential.json");
    let output = run.exec(&[
        "identity",
        "client-secret",
        "ci",
        "--human",
        "--output-file",
        path.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    attach.assert_async().await;
    assert!(!stdout(&output).contains("sk-secret"));
    assert!(!stderr(&output).contains("sk-secret"));
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(json["clientId"], "cid-9");
    assert_eq!(json["clientSecret"], "sk-secret");
}

#[tokio::test]
async fn a_malformed_invocation_fails_as_json() {
    // clap used to print English usage text and exit before the JSON machinery
    // ran, so an agent generating a slightly wrong command line got something
    // it could not parse.
    let run = Run::new("http://127.0.0.1:1");
    let output = run.exec(&["project", "create", "--name", "x", "--type", "nope"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr_json(&output)["error"], "usage_error");
    assert!(stdout(&output).is_empty());
}

#[tokio::test]
async fn recoverable_failures_have_their_own_error_kinds() {
    // An agent distinguishes "retry with --yes" from "this can never work"
    // without reading prose.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("GET", "/api/v1/projects/p1")
        .with_status(200)
        .with_body(r#"{"project":{"id":"p1","name":"billing","slug":"billing","environments":[]}}"#)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    assert_eq!(
        stderr_json(&run.exec(&["project", "delete", "billing"]))["error"],
        "confirmation_required"
    );
    assert_eq!(
        stderr_json(&run.exec(&["project", "update", "billing"]))["error"],
        "nothing_to_update"
    );
}

#[tokio::test]
async fn a_mid_command_failure_leaves_stdout_empty() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    server
        .mock("POST", "/api/v1/projects/p1/identities")
        .with_status(200)
        .with_body(r#"{"identity":{"id":"i9","name":"ci"}}"#)
        .create_async()
        .await;
    server
        .mock("PATCH", "/api/v1/projects/p1/memberships/identities/i9")
        .with_status(401)
        .with_body(r#"{"message":"token expired"}"#)
        .create_async()
        .await;
    server
        .mock("DELETE", "/api/v1/projects/p1/identities/i9")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["identity", "create", "--name", "ci", "--project", "billing"]);

    // A partial write would show up here, mid-pipeline.
    assert!(
        stdout(&output).is_empty(),
        "stdout leaked on failure: {}",
        stdout(&output)
    );
    // Annotating the failure must not relabel it: an agent branching on
    // not_logged_in has to re-authenticate, not retry.
    let json = stderr_json(&output);
    assert_eq!(json["error"], "not_logged_in");
    assert_eq!(json["exitCode"], 2);
    assert_eq!(output.status.code(), Some(2));
    assert!(json["message"].as_str().unwrap().contains("removed"));
}

#[tokio::test]
async fn env_create_rejects_a_name_that_cannot_produce_a_slug() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/v1/projects")
        .with_status(200)
        .with_body(TWO_PROJECTS)
        .create_async()
        .await;
    let created = server
        .mock("POST", "/api/v1/projects/p1/environments")
        .with_status(200)
        .expect(0)
        .create_async()
        .await;

    let run = Run::new(&server.url());
    let output = run.exec(&["project", "env", "create", "billing", "--name", "Продакшн"]);

    assert_eq!(output.status.code(), Some(2));
    let json = stderr_json(&output);
    assert_eq!(json["error"], "config_error");
    assert!(json["message"].as_str().unwrap().contains("--slug"));
    // An empty slug must never reach the API.
    created.assert_async().await;
}

#[test]
fn credential_issuance_requires_a_destination_before_authentication() {
    let run = Run::new("http://127.0.0.1:1");
    for args in [
        vec!["identity", "client-secret", "ci"],
        vec![
            "identity",
            "create",
            "--name",
            "ci",
            "--project",
            "example",
            "--with-universal-auth",
        ],
    ] {
        let output = run.exec(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(stdout(&output).is_empty());
        assert_eq!(stderr_json(&output)["error"], "secret_delivery_required");
    }
    let output = run.exec(&["identity", "client-secret", "ci", "--reveal"]);
    assert_eq!(
        stderr_json(&output)["error"],
        "secret_exposure_acknowledgement_required"
    );
}
