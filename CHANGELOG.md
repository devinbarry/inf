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
  required and `--role` defaults to the read-only `viewer`.
- `inf identity ls|get|rename|set-role|delete|client-secret` — identity lifecycle,
  each accepting `--project` to address project-scoped identities.
- `--json` on every command for scripting.
- Query resolution accepting id, slug, exact name or unique substring, with exact
  matches taking precedence and ambiguity reported as an error listing candidates.
- Session reuse from `infisical login`, including up-front expiry detection.

### Security

- No organization-level resource can be created by this tool. There is no
  org-level identity creation, no organization role assignment, and no way to
  grant an identity access to a project it is not scoped to. Identities are
  confined to one project for their entire life, so a leaked credential cannot
  reach beyond the job it was created for. `inf identity delete` is the
  revocation path.

### Notes

- Requires the official `infisical` CLI and a user login. Identity-management
  routes accept a user JWT only, so a machine identity cannot drive this tool.
