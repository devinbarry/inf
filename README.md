# inf

An Infisical **control-plane** CLI, built to be driven by agents: manage
projects, environments and machine identities from a script, an LLM agent, or a
CI job.

The official `infisical` CLI manages secrets but has no project or identity
commands — those live only in the REST API and the web UI. `inf` fills that gap.

```
crate: ifp        binary: inf
```

## Built for agents, not for eyes

This is the design constraint everything else follows from. The expected caller
is a program, so:

- **stdout is always JSON, and only JSON.** No tables, no colour, no banners, no
  status lines. `inf project ls | jq '.[].slug'` works with nothing to strip.
- **Errors are JSON on stderr**, with a stable `error` slug to branch on, so
  failure handling never means pattern-matching English prose.
- **stdout stays empty when a command fails**, so a broken run cannot feed
  half-written garbage into a pipeline.
- **Destructive commands never prompt.** They require `--yes` and fail
  immediately without it — an agent gets a deterministic refusal instead of a
  process blocked on a question nobody will answer.
- **`--human` is the explicit opt-out** for a person at a terminal: tables,
  colour, and an interactive confirmation prompt.

## Why it exists

Verified against `infisical` v0.43.116, the official CLI has real CRUD for exactly
three things: secrets, folders and dynamic-secret leases. It cannot create, rename,
list or delete a project; it cannot manage environments; and it cannot create or
manage machine identities. `infisical init` only links a local directory to a
project that already exists.

`inf` covers the missing control plane. Secrets stay where they belong:

| Task | Tool |
|---|---|
| Read/write secrets, folders, `run`, `export`, agents, scanning | `infisical` |
| Projects, environments, machine identities | `inf` |

## Install

```sh
cargo install --path .
```

Requires the official `infisical` CLI on `PATH` and an active login.

## Authentication

`inf` stores no credentials. It reuses the session created by `infisical login`:

```sh
infisical login          # once, as a user
inf whoami               # confirms instance, org and remaining session life
```

This is required, not merely convenient. Infisical's identity-management routes
accept a **user JWT only** — a Universal Auth machine identity is rejected with 403.
Because `inf` manages identities, it has to run as a user.

Sessions last 10 days and the official CLI never refreshes them, so `inf` detects
expiry up front and tells you to log in again rather than surfacing an opaque 401.

The instance URL is taken from `--domain`, then `$INFISICAL_DOMAIN`, then whichever
instance `infisical login` last used.

## Global flags

| Flag | Effect |
|---|---|
| `--human` | Tables and colour instead of JSON. Also enables the interactive confirmation prompt. |
| `--domain <URL>` | Target a specific instance. Accepts the URL with or without the `/api` suffix. |

Both are accepted before or after the subcommand.

## Usage

Projects and identities accept an id, an exact name, or a unique substring;
projects also accept a slug. An exact match always wins; an ambiguous substring
is an error listing the candidates, never a silent "first match".

Environments are matched exactly — by slug, id, or name (case-insensitively).
Substrings are deliberately not accepted there, because `dev` would otherwise
also match `development` and the losing case is a deleted environment.

### Projects

```sh
inf project ls
inf project get billing
inf project create --name "Billing API" --description "invoicing"
inf project rename billing "Payments API"
inf project update billing --slug payments-api
inf project delete billing --yes
inf project identities billing        # who can reach this project
```

Aliases: `ls`/`list`, `delete`/`rm`.

`--type` selects the product: `secret-manager` (default), `cert-manager`, `kms`,
`ssh`, `secret-scanning`, `pam`, `ai`. Note that `ssh` and `ai` were removed from
Infisical in v0.162.16 — servers at or above that version reject them, while
older servers and Infisical Cloud still accept them.

When `--slug` is omitted the server derives one from the name **and appends a
random suffix**, so `--name "Billing API"` yields something like
`billing-api-g3-k8`, not `billing-api`. Pass `--slug` when the value matters.

Projects with delete protection are rejected before any confirmation, with the
command needed to clear it.

### Environments

```sh
inf project env ls billing
inf project env create billing --name "QA (EU)"      # slug derived: qa-eu
inf project env update billing qa-eu --name "QA Europe"
inf project env delete billing qa-eu --yes
```

Derived slugs keep ASCII letters and digits only. A name with none of those —
`"Продакшн"`, `"本番"` — is rejected with a pointer to `--slug` rather than
sending an empty slug to the API.

### Machine identities

Identities are **always scoped to a single project**, and default to a
**read-only** role.

`--project` is *required* for `create` and `set-role`. It is *optional* for
`ls`, `get`, `rename`, `delete` and `client-secret`: with it, `inf` searches the
project's identities; without it, the organization's.

```sh
inf identity create --name ci-reader --project billing --with-universal-auth
inf identity ls --project billing
inf identity get ci-reader --project billing
inf identity set-role ci-reader --project billing --role viewer
inf identity rename ci-reader billing-reader --project billing
inf identity client-secret ci-reader --project billing --ttl 3600
inf identity delete ci-reader --project billing --yes
```

Aliases: `inf id` for `inf identity`; `ls`/`list`, `delete`/`rm`.

`--project` is how a project-scoped identity is found *and how the right API
route is chosen*: the API does not return project-scoped identities from the
organization listing, and its organization rename/delete routes reject them
outright. `inf identity ls` without `--project` shows only organization-level
identities (typically pre-existing ones you may want to audit or remove).

`--with-universal-auth` attaches Universal Auth and mints a client secret in one
step, so the new identity is immediately usable rather than left unable to log in.
`inf identity client-secret` attaches the method first if it is missing.

If any step after creation fails, `inf` removes the partially created identity
rather than leaving an unusable one behind — a retry would otherwise produce a
second identity with the same name, after which every command resolving by name
reports an ambiguous match.

**Client secrets are shown once and cannot be retrieved again.**

#### No organization-level identities — by design

`inf` provides no way to create an organization-level identity, and no way to grant
an existing identity access to an additional project. This is deliberate and not an
oversight:

- An org-level identity is visible organization-wide and can be granted access to
  **any** project in the org. A leaked credential then has blast radius far beyond
  the one job it was created for.
- A project-scoped identity is confined to its project from the moment it exists.
  There is no call that widens it.

`--role` on create is the *project* role and defaults to `viewer`. To revoke an
identity, delete it.

Built-in project roles, least to most privileged: `no-access`, `viewer`, `member`,
`admin` (plus `cryptographic-operator` in KMS projects). Custom role slugs are
accepted too, which is why the value is validated by the server rather than by
`inf`. Prefer `viewer` — machine identities usually read secrets rather than
write them.

## Output contract

Every command writes JSON to stdout:

```sh
inf project ls | jq -r '.[] | select(.type=="ssh") | .slug'
inf identity create --name ci --project billing --with-universal-auth | jq -r .clientSecret
inf project delete billing --yes | jq -r .deleted
```

Listings are JSON arrays; single objects are returned bare. Deletes return
`{"deleted": true, "id": ..., "name": ...}`. Identity listings carry a computed
`role`, `id` and `name` alongside the raw API fields, because the organization
route returns a scalar `role` while the project route returns a `roles[]` array —
a script should not have to know which it is talking to.

Errors are JSON on stderr:

```json
{
  "error": "ambiguous_match",
  "message": "Ambiguous match 'billing', candidates:\n  Billing API (billing-api)\n  Billing Web (billing-web)",
  "exitCode": 2,
  "query": "billing",
  "candidates": [
    { "name": "Billing API", "id": "p1", "slug": "billing-api" },
    { "name": "Billing Web", "id": "p2", "slug": "billing-web" }
  ]
}
```

`error` is a stable slug — match on it, not on `message`. The values are:
`api_error`, `not_found`, `http_error`, `config_error`, `usage_error`,
`confirmation_required`, `delete_protected`, `nothing_to_update`,
`cli_not_found`, `not_logged_in`, `cli_failed`, `token_expired`, `forbidden`,
`no_match`, `ambiguous_match`, `aborted`.

The mechanically recoverable ones are worth special handling:
`confirmation_required` (retry with `--yes`), `ambiguous_match` (pick a
candidate id and retry), `not_logged_in` / `token_expired` (re-authenticate),
and `nothing_to_update` (the command was a no-op).

A malformed command line — an unknown flag, a bad `--type` — is
`usage_error`, not clap's English usage text. `--help` and `--version` are the
one deliberate exception: they are human-invoked, exit 0, and print plain text.

Exit codes:

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | The request failed — the server refused it (`api_error`, `forbidden`, `not_found`) or it could not be completed (`http_error`) |
| `2` | The invocation was wrong and something local must change — usage, config, session, or an unresolved/ambiguous query |

The split is "would changing my command line have helped?". A refused connection
would not have, so it is `1`, not `2`.

## Scope

Deliberately narrow: the endpoints the official CLI omits and that day-to-day work
needs.

Not implemented — add as required: roles and groups, secret approval policies,
webhooks, integrations, secret syncs, app connections, PKI/cert-manager, KMS, PAM.

Deliberately excluded, not merely unimplemented: organization-level identity
creation, organization role assignment, org member invitations, and granting an
identity access to a project it is not scoped to. See the note above.

## Development

```sh
just check     # fmt --check + clippy -D warnings + test
just install
```

Tests never touch a real instance. Unit tests live inline; `tests/cli.rs` drives
the compiled binary against a mock HTTP server with a fake `infisical` shim on
`PATH`, which is how the CLI surface and the JSON contract are verified without
mutating anyone's organization.

A `pre-commit` hook (via `cargo-husky`) runs fmt, clippy and the tests.

See [AGENTS.md](AGENTS.md) for architecture and the API quirks worth not
rediscovering.

## License

MIT
