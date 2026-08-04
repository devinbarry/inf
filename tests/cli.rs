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
         echo '{}'\n",
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

struct Run {
    shim_dir: tempfile::TempDir,
    api_url: String,
}

impl Run {
    fn new(api_url: &str) -> Self {
        let shim_dir = tempfile::tempdir().unwrap();
        install_shim(shim_dir.path());
        Self {
            shim_dir,
            api_url: api_url.to_string(),
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
        Command::new(bin())
            .args(args)
            .env("PATH", path)
            .env("INFISICAL_DOMAIN", &self.api_url)
            // Colour codes would otherwise depend on whether the test harness
            // is attached to a terminal.
            .env("NO_COLOR", "1")
            .output()
            .expect("failed to run inf")
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
    assert_eq!(json["error"], "config_error");
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
    assert!(json["message"].as_str().unwrap().contains("Invalid role"));
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
    let output = run.exec(&["identity", "client-secret", "ci"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr_json(&output)["error"], "not_logged_in");
    attach.assert_async().await;
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
