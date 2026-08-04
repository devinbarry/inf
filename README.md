# inf

An Infisical **control-plane** CLI: manage projects, environments and machine
identities from the terminal.

The official `infisical` CLI manages secrets but has no project or identity
commands — those live only in the REST API and the web UI. `inf` fills that gap.

```
crate: ifp        binary: inf
```

## Why

Verified against `infisical` v0.43.116, the official CLI has real CRUD for exactly
three things: secrets, folders and dynamic-secret leases. It cannot create, rename,
list or delete a project; it cannot manage environments; and it cannot create or
manage machine identities. `infisical init` only links a local directory to a
project that already exists.

`inf` covers the missing control plane. Secrets stay where they belong:

| Task | Tool |
|---|---|
| Read/write secrets, folders, `run`, `export` | `infisical` |
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

## Usage

Every command that takes a project or identity accepts an id, a slug, an exact name,
or a unique substring. An exact match always wins; an ambiguous substring is an error
listing the candidates, never a silent "first match".

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

`--type` selects the product: `secret-manager` (default), `cert-manager`, `kms`,
`ssh`, `secret-scanning`, `pam`, `ai`.

Projects with delete protection are rejected before the confirmation prompt, with
the command needed to clear it.

### Environments

```sh
inf project env ls billing
inf project env create billing --name "QA (EU)"      # slug derived: qa-eu
inf project env update billing qa-eu --name "QA Europe"
inf project env delete billing qa-eu --yes
```

### Machine identities

Identities are **always scoped to a single project**, and default to a **read-only**
role. `--project` is mandatory.

```sh
inf identity create --name ci-reader --project billing --with-universal-auth
inf identity ls --project billing
inf identity get ci-reader --project billing
inf identity set-role ci-reader --project billing --role viewer
inf identity rename ci-reader billing-reader --project billing
inf identity client-secret ci-reader --project billing --ttl 3600
inf identity delete ci-reader --project billing --yes
```

`--project` is how a project-scoped identity is found: the API does **not** return
them from the organization listing, so `inf identity ls` without `--project` shows
only organization-level identities (typically pre-existing ones you may want to
audit or remove).

`--with-universal-auth` attaches Universal Auth and mints a client secret in one
step, so the new identity is immediately usable rather than left unable to log in.
`inf identity client-secret` attaches the method first if it is missing.

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
`admin`. Prefer `viewer` — machine identities usually read secrets rather than
write them.

### Scripting

`--json` on any read or mutating command emits machine-readable output:

```sh
inf project ls --json | jq -r '.[] | select(.type=="ssh") | .slug'
inf identity create --name ci --project billing --with-universal-auth --json | jq -r .clientSecret
```

Exit codes: `0` success, `1` the server refused an authenticated request,
`2` usage, config or resolution error.

Destructive commands prompt for confirmation. When stdin is not a terminal they
refuse outright unless `--yes` is given — EOF is never treated as consent.

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

See [AGENTS.md](AGENTS.md) for architecture and the API quirks worth not
rediscovering.

## License

MIT
