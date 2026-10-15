# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-10-15

### Added

- Published on crates.io as `ifp`. The name `inf` is taken there, so `cargo install ifp` installs the `inf` binary.
  The crate ships only the source, lockfile, README, changelog and licence.

### Changed

- Dependency advisories are checked with `cargo deny` over the crates that can be compiled; `cargo audit` is no
  longer run, because it also reported optional crates that are never built.

## [0.2.3] - 2026-10-09

### Security

- Updated dependencies to clear RUSTSEC-2026-0258 (h2) and RUSTSEC-2026-0285 (rustls) with a compatible lockfile
  refresh. `cargo audit` with warnings denied and a Trivy scan are clean.

## [0.2.2] - 2026-09-30

### Changed

- Workstation authentication uses the agent-safe Infisical fork's private descriptor handoff instead of token stdout.
- One-time identity credentials require a new private file, writable private descriptor, or explicit reveal and
  exposure acknowledgement before issuance. Human formatting cannot bypass this policy.
- API error bodies and arbitrary credential-source stderr are withheld from diagnostics, preserving error kinds
  and HTTP status without copying credentials into agent histories.

### Validation

- Hermetic CLI tests cover private session handoff, credential delivery, acknowledgement gates and error redaction.
- Servers, containers, CI and deployed automation retain the official Infisical CLI.

## [0.2.1] - 2026-08-05

### Added

- SSH project recovery controls: list, rename, enable or disable certificate
  authorities; retrieve their public keys; and list or create constrained
  User-certificate templates.
- A fixed least-privilege SSH issuer role whose only permission is
  `ssh-certificates:create`.
- A bounded Universal Auth profile for SSH issuers with 900-second access
  tokens and lockout protection.
- Project creation controls for omitting irrelevant default environments and
  enabling deletion protection immediately.

### Fixed

- SSH CA public keys are decoded from the API's plain-text response rather
  than incorrectly treating them as JSON.

## [0.2.0] - 2026-08-04

Correctness release from a four-round cross-model review. Everything here is a
fix, but several change the observable contract, so this is a minor bump rather
than a patch — see **Changed** before upgrading a script.

### Fixed

- **Commands could hang indefinitely.** The HTTP client had no timeout, so an
  instance that accepted a connection and then never answered stalled until TCP
  keepalive noticed — roughly two hours on Linux — emitting no stdout, no
  stderr and no exit code. A refused connection failed instantly, which is why
  this went unnoticed. Bounded at 10s connect and 30s request
  (`INF_HTTP_TIMEOUT_SECS`). The same hole on the `infisical` subprocess is
  bounded at 30s (`INF_CLI_TIMEOUT_SECS`), and that call now runs with stdin
  closed so a prompting CLI cannot block either.
- **Queries could resolve to the wrong object.** All three resolvers matched
  id, slug and name in one pass and took the first hit, so the winner depended
  on the order the API returned rows. Infisical enforces uniqueness on ids and
  slugs but not on names, so two projects — or two identities left by a failed
  create — can share one. `inf identity delete ci --yes` could therefore revoke
  a live credential instead of the orphan. Exact matching is now tiered (id,
  then slug, then name) and a collision within a tier is an error.
- **A URL in the CLI's output was mistaken for the session token.**
  `https://artifacts.infisical.com` is three non-empty dot-separated segments,
  and printed first it won, so any banner broke every command with
  "cannot decode session token" for a correctly logged-in user.
- **Identity listings silently truncated.** The project-membership route
  defaults to 20 rows; identities past the twentieth vanished and then resolved
  as "no match" rather than an error. The org route does not paginate at all,
  so an early fix that walked it by page count re-fetched the same response and
  duplicated every row. Both now terminate on `totalCount`.
- **`identity rename` failed for every identity `inf` creates.** It used the
  organization route, which rejects project-scoped identities with "Identity is
  managed by project" — the same guard the delete route has. `--project` now
  selects the route, not just the search space.
- **`identity get` never reported `clientId`.** The Universal Auth lookup was
  gated on `authMethods`, which the project-membership route returns empty even
  when the method is attached, so the one field a script wants was permanently
  null for project-scoped identities.
- **A failed `identity create` left an unusable identity behind**, silently. A
  retry then produced a duplicate name, after which every command resolving by
  name was ambiguous. The partial identity is now removed, and if cleanup also
  fails the id is reported.
- **`inf project ls | head -1` panicked** with exit 101 and a Rust backtrace on
  stderr. `inf` now dies from `SIGPIPE` (141) like any other Unix tool.
- Delete protection was read from the project listing rather than the detail
  route, so the guard could be skipped exactly when it mattered.
- A `204 No Content` on a mutation was reported as a parse failure *after* the
  change had been applied.
- `client-secret` treated every error as "Universal Auth not attached", so an
  expired session was answered with "already configured".
- A response body that stalled mid-transfer became an empty string, surfacing
  as a parse error rather than a timeout.
- `env create` derived an empty slug from a name with no ASCII alphanumerics
  and sent it; it now fails with a pointer to `--slug`.
- Table columns were sized in bytes, so non-ASCII and CJK names misaligned.
- A locked keyring reported as "not logged in", sending users to a login that
  would not fix it.

### Changed

Breaking for anything matching on the error contract:

- `config_error` split into `confirmation_required`, `delete_protected` and
  `nothing_to_update`. The first is the one failure an agent can recover from
  mechanically, and it was previously indistinguishable from a bad flag
  combination.
- New kinds: `usage_error` (malformed command line, previously clap's English
  usage text), `http_timeout`, `cli_timeout`, `not_found`, `cli_failed`.
- `identity get` now **fails** when the Universal Auth lookup errors, instead of
  exiting 0 with `clientId: null`. A null there now means "not attached", not
  "something went wrong".
- `project identities` emits the same shape as `identity ls` — a computed
  `role`, `id` and `name` alongside the raw fields. It previously returned the
  raw membership, so the README's own `jq` example read `null`.
- Exit code `141` is now possible, when stdout is closed downstream.

### Added

- `INF_HTTP_TIMEOUT_SECS` and `INF_CLI_TIMEOUT_SECS` to tune the two waits.
- End-to-end tests (`tests/cli.rs`) that drive the compiled binary against a
  mock server with a fake `infisical` shim on `PATH`. 123 tests total, none of
  which touch a real instance — verifying this CLI by running it against a live
  login mutates whatever organization the caller is logged into.

## [0.1.0] - 2026-08-04

Initial release.

### Added

- `inf whoami` — active instance, organization and remaining session lifetime,
  answered from the decoded session so it works while the API is unreachable.
- `inf project ls|get|create|rename|update|delete` — full project lifecycle.
- `inf project identities` — machine identities with access to a project.
- `inf project env ls|create|update|delete` — environment management, with slugs
  derived from names when omitted.
- `inf identity create` — creates a **project-scoped** identity; `--project` is
  required and `--role` defaults to the read-only `viewer`. If any step after
  creation fails, the partially created identity is removed rather than left
  behind.
- `inf identity ls|get|rename|set-role|delete|client-secret` — identity lifecycle.
  `--project` is required for `set-role` and optional elsewhere, where it selects
  the project's identities instead of the organization's — and, for `rename` and
  `delete`, selects the API route that accepts a project-scoped identity.
- Query resolution accepting id, name or unique substring (plus slug for
  projects), with exact matches taking precedence and ambiguity reported as an
  error listing structured candidates. Environments match exactly only.
- Session reuse from `infisical login`, including up-front expiry detection.
- Global `--domain` to target a specific instance.

### Output

- **JSON on stdout is the default and the only thing written there.** `inf` is
  built to be driven by agents and scripts, so there are no tables, colour or
  status lines to strip, and stdout stays empty when a command fails.
- **Errors are JSON on stderr** with a stable `error` slug — `ambiguous_match`,
  `forbidden`, `not_logged_in` and so on — plus `message` and `exitCode`.
  Ambiguous matches carry structured `candidates` with ids.
- **`--human`** is the explicit opt-in for tables, colour and the interactive
  confirmation prompt. Column widths are display widths, so non-ASCII and CJK
  names stay aligned.
- Exit codes: `0` success, `1` the request failed (refused by the server, or not
  completable — including network failures), `2` the invocation was wrong.

### Security

- No organization-level resource can be created by this tool. There is no
  org-level identity creation, no organization role assignment, and no way to
  grant an identity access to a project it is not scoped to. Identities are
  confined to one project for their entire life, so a leaked credential cannot
  reach beyond the job it was created for. `inf identity delete` is the
  revocation path.
- Destructive commands never prompt in the default mode: they require `--yes`
  and fail immediately without it, so EOF or a non-interactive run can never be
  read as consent.

### Notes

- Requires the official `infisical` CLI and a user login. Identity-management
  routes accept a user JWT only, so a machine identity cannot drive this tool.
- Identity listings are paginated; the API defaults to 20 rows per page, so
  unpaginated callers silently lose identities past the twentieth.
- `--type ssh` and `--type ai` are accepted but were removed from Infisical in
  v0.162.16; servers at or above that version reject them.

[0.2.0]: https://example.invalid/tools/inf/-/tags/v0.2.0
[0.1.0]: https://example.invalid/tools/inf/-/tags/v0.1.0
