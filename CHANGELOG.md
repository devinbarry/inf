# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.0]: https://example.invalid/tools/inf/-/tags/v0.1.0
