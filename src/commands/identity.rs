use crate::client::InfisicalClient;
use crate::commands::confirm;
use crate::error::{IfpError, Result};
use crate::models::IdentityMembership;
use crate::output::{self, Format};
use crate::resolve;
use colored::Colorize;
use serde_json::json;

/// Where to look for an identity.
///
/// Project-scoped identities do **not** appear in the organization listing, so
/// a command that only ever searched the org would be unable to manage the very
/// identities this tool creates. `--project` selects the project listing;
/// without it we fall back to the organization, which is how pre-existing
/// org-level identities remain manageable (and deletable).
struct Scope {
    identity: IdentityMembership,
    project: Option<ProjectRef>,
}

struct ProjectRef {
    id: String,
    name: String,
}

async fn resolve_scope(
    client: &InfisicalClient,
    project: Option<&str>,
    query: &str,
) -> Result<Scope> {
    match project {
        Some(project_query) => {
            let (project, identity) = resolve_in_project(client, project_query, query).await?;
            Ok(Scope {
                identity,
                project: Some(project),
            })
        }
        None => {
            let identities = client.list_identities().await?;
            let identity = resolve::identity(&identities, query)?.clone();
            Ok(Scope {
                identity,
                project: None,
            })
        }
    }
}

/// Resolve within a named project. Commands that require a project call this
/// directly, so the "no project" case never has to be handled downstream as an
/// error that could not actually occur.
async fn resolve_in_project(
    client: &InfisicalClient,
    project_query: &str,
    query: &str,
) -> Result<(ProjectRef, IdentityMembership)> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, project_query)?;
    let identities = client.list_project_identities(&matched.id).await?;
    let identity = resolve::identity(&identities, query)?.clone();
    Ok((
        ProjectRef {
            id: matched.id.clone(),
            name: matched.name.clone(),
        },
        identity,
    ))
}

pub async fn list(client: &InfisicalClient, project: Option<&str>, format: Format) -> Result<()> {
    let mut identities = match project {
        Some(project_query) => {
            let projects = client.list_projects().await?;
            let matched = resolve::project(&projects, project_query)?;
            client.list_project_identities(&matched.id).await?
        }
        None => client.list_identities().await?,
    };
    identities.sort_by_key(|i| i.name().to_lowercase());

    // The raw membership shape differs between the two routes: the org listing
    // carries a scalar `role`, the project listing a `roles[]` array. Emitting
    // a computed `role` alongside means a script written against one listing
    // does not break silently against the other.
    let payload: Vec<serde_json::Value> = identities
        .iter()
        .map(|i| {
            let mut value = serde_json::to_value(i).unwrap_or_default();
            value["id"] = json!(i.id());
            value["name"] = json!(i.name());
            value["role"] = json!(i.role());
            value
        })
        .collect();

    output::emit(format, &payload, || match project {
        Some(_) => output::project_identities_table(&identities),
        None => {
            output::identities_table(&identities);
            println!(
                "{}",
                "Organization-level identities only. Use --project to list a project's identities."
                    .dimmed()
            );
        }
    });
    Ok(())
}

pub async fn get(
    client: &InfisicalClient,
    query: &str,
    project: Option<&str>,
    format: Format,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let identity = &scope.identity;

    // The Universal Auth client id is the half of the credential pair that is
    // safe to display and is required for every machine login — it is the one
    // field a script calling `identity get` actually wants, so it belongs in
    // the JSON too, not only in the human rendering.
    let attached = identity
        .identity
        .auth_methods
        .iter()
        .any(|m| m == "universal-auth");
    let mut universal_auth = None;
    let mut universal_auth_error = None;
    if attached {
        match client.get_universal_auth(identity.id()).await {
            Ok(config) => universal_auth = Some(config),
            // Never swallow this. Dropping the section on error told the reader
            // the identity had no client id, while `authMethods` two lines
            // above said it did.
            Err(err) => universal_auth_error = Some(err.to_string()),
        }
    }

    let mut payload = serde_json::to_value(identity).unwrap_or_default();
    payload["id"] = json!(identity.id());
    payload["name"] = json!(identity.name());
    payload["role"] = json!(identity.role());
    payload["authMethods"] = json!(identity.identity.auth_methods);
    match &scope.project {
        Some(project) => {
            payload["scope"] = json!("project");
            payload["projectId"] = json!(project.id);
            payload["projectName"] = json!(project.name);
        }
        None => payload["scope"] = json!("organization"),
    }
    payload["clientId"] = json!(universal_auth.as_ref().map(|c| c.client_id.clone()));
    payload["universalAuth"] = json!(universal_auth);
    if let Some(err) = &universal_auth_error {
        payload["universalAuthError"] = json!(err);
    }

    if let Some(err) = &universal_auth_error {
        output::warn(
            format,
            &format!("could not read Universal Auth configuration: {err}"),
        );
    }

    output::emit(format, &payload, || {
        println!("{} {}", "Name:".bold(), identity.name());
        println!("{} {}", "ID:".bold(), identity.id());
        match &scope.project {
            Some(project) => {
                println!("{} project '{}'", "Scope:".bold(), project.name);
                println!("{} {}", "Project role:".bold(), identity.role());
            }
            None => {
                println!("{} {}", "Scope:".bold(), "organization".yellow());
                println!("{} {}", "Org role:".bold(), identity.role());
            }
        }
        println!("{} {}", "Auth methods:".bold(), identity.auth_methods());
        if let Some(created) = &identity.created_at {
            println!("{} {}", "Created:".bold(), created);
        }
        println!(
            "{} {}",
            "Last login:".bold(),
            identity.last_login_time.as_deref().unwrap_or("never")
        );
        if let Some(config) = &universal_auth {
            println!("\n{}", "Universal Auth".bold());
            println!("{} {}", "  Client ID:".bold(), config.client_id);
            if let Some(ttl) = config.access_token_ttl {
                println!("{} {}s", "  Access token TTL:".bold(), ttl);
            }
        }
    });
    Ok(())
}

/// Identities are always created scoped to one project. `inf` deliberately
/// offers no way to create an organization-level identity: such an identity is
/// visible org-wide and can be granted access to any project, so a leaked
/// credential reaches far beyond the job it was made for. A project-scoped
/// identity is confined for its entire life.
pub async fn create(
    client: &InfisicalClient,
    name: &str,
    project: &str,
    role: &str,
    with_universal_auth: bool,
    format: Format,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, project)?;
    let project_id = matched.id.clone();
    let project_name = matched.name.clone();
    let project_slug = matched.slug.clone();

    let created = client
        .create_project_identity(&project_id, name, role)
        .await?;

    let identity_id = created
        .get("identity")
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| IfpError::Api("create returned no identity id".into()))?
        .to_string();

    // Everything past this point can fail with an identity already on the
    // server. A half-built identity — wrong role, or no auth method at all — is
    // never what the caller asked for, and leaving one behind with a duplicate
    // name makes every later command ambiguous. So unwind it.

    // Servers before v0.162.0 strip the `roles` array from the create body and
    // land the identity on `no-access`; applying it explicitly is what makes
    // the requested role real there, and a harmless re-assert on newer servers.
    if let Err(err) = client
        .set_project_identity_role(&project_id, &identity_id, role)
        .await
    {
        return Err(unwind(client, &project_id, &identity_id, err).await);
    }

    let mut credentials = None;
    if with_universal_auth {
        // A fresh identity has no auth method and cannot log in. Attach
        // Universal Auth and mint one secret so the caller ends up with a
        // usable credential pair rather than a half-configured identity.
        match attach_and_mint(client, &identity_id).await {
            Ok(pair) => credentials = Some(pair),
            Err(err) => return Err(unwind(client, &project_id, &identity_id, err).await),
        }
    }

    let mut payload = json!({
        "id": identity_id,
        "name": name,
        "projectId": project_id,
        "projectName": project_name,
        "role": role,
        "scope": "project",
    });
    if let Some((client_id, client_secret)) = &credentials {
        payload["clientId"] = json!(client_id);
        payload["clientSecret"] = json!(client_secret);
    }

    output::success(
        format,
        &format!("created identity '{name}' scoped to project '{project_name}' as {role}"),
    );
    output::emit(format, &payload, || {
        println!("{} {}", "Identity ID: ".bold(), identity_id);
        match &credentials {
            Some((client_id, client_secret)) => {
                println!("{} {}", "Client ID:   ".bold(), client_id);
                println!("{} {}", "Client Secret:".bold(), client_secret);
            }
            None => println!(
                "{}",
                format!(
                    "Attach an auth method before use: \
                     inf identity client-secret {name} --project {project_slug}"
                )
                .dimmed()
            ),
        }
    });
    if credentials.is_some() {
        // Not retrievable after this response, so warn while it is still on
        // screen. In JSON mode the secret is in the payload and the caller
        // already knows to keep it.
        output::warn(
            format,
            "the client secret is shown once and cannot be retrieved again",
        );
    }
    Ok(())
}

async fn attach_and_mint(client: &InfisicalClient, identity_id: &str) -> Result<(String, String)> {
    let config = client.attach_universal_auth(identity_id).await?;
    let secret = client
        .create_client_secret(identity_id, "created by inf", 0, 0)
        .await?;
    Ok((config.client_id, secret.client_secret))
}

/// Remove an identity whose setup could not be completed, and fold the outcome
/// into the error the caller sees. Without this a failed `create` left a
/// `no-access` identity behind and said nothing about it — and a retry then
/// produced a second identity with the same name, after which every command
/// resolving by name reported an ambiguous match.
async fn unwind(
    client: &InfisicalClient,
    project_id: &str,
    identity_id: &str,
    cause: IfpError,
) -> IfpError {
    match client
        .delete_project_identity(project_id, identity_id)
        .await
    {
        Ok(()) => annotate(
            cause,
            "The partially created identity was removed; nothing was left behind.".to_string(),
        ),
        Err(cleanup) => annotate(
            cause,
            format!(
                "An identity was created but could not be configured or removed \
                 (cleanup failed: {cleanup}).\nDelete it by id:\n  \
                 inf identity delete {identity_id} --project {project_id} --yes"
            ),
        ),
    }
}

/// Add context without discarding the error kind — agents match on `kind`, so
/// collapsing everything to a generic API error would lose the reason.
fn annotate(err: IfpError, note: String) -> IfpError {
    match err {
        IfpError::Api(m) => IfpError::Api(format!("{m}\n{note}")),
        IfpError::NotFound(m) => IfpError::NotFound(format!("{m}\n{note}")),
        IfpError::Forbidden(m) => IfpError::Forbidden(format!("{m}\n{note}")),
        IfpError::Config(m) => IfpError::Config(format!("{m}\n{note}")),
        other => IfpError::Api(format!("{other}\n{note}")),
    }
}

/// Rename only. Organization roles are never granted by this tool, so the
/// underlying route's `role` field is not exposed.
pub async fn rename(
    client: &InfisicalClient,
    query: &str,
    new_name: &str,
    project: Option<&str>,
    format: Format,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let old_name = scope.identity.name().to_string();
    let identity_id = scope.identity.id().to_string();

    // A project-scoped identity must be renamed through its project. The
    // organization route rejects it with "Identity is managed by project",
    // exactly as the organization delete route does — so `--project` has to
    // pick the route, not merely locate the identity.
    match &scope.project {
        Some(project) => {
            client
                .update_project_identity(&project.id, &identity_id, new_name)
                .await?
        }
        None => client.update_identity(&identity_id, new_name).await?,
    };

    output::success(
        format,
        &format!("renamed identity '{old_name}' to '{new_name}'"),
    );
    output::emit(
        format,
        &json!({
            "id": identity_id,
            "name": new_name,
            "previousName": old_name,
            "scope": if scope.project.is_some() { "project" } else { "organization" },
        }),
        || {},
    );
    Ok(())
}

/// Change an identity's role within the project it is scoped to.
pub async fn set_role(
    client: &InfisicalClient,
    query: &str,
    project: &str,
    role: &str,
    format: Format,
) -> Result<()> {
    let (project, identity) = resolve_in_project(client, project, query).await?;

    client
        .set_project_identity_role(&project.id, identity.id(), role)
        .await?;

    output::success(
        format,
        &format!(
            "set identity '{}' to role '{role}' in project '{}'",
            identity.name(),
            project.name
        ),
    );
    output::emit(
        format,
        &json!({
            "id": identity.id(),
            "name": identity.name(),
            "projectId": project.id,
            "projectName": project.name,
            "role": role,
        }),
        || {},
    );
    Ok(())
}

pub async fn delete(
    client: &InfisicalClient,
    query: &str,
    project: Option<&str>,
    assume_yes: bool,
    format: Format,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let name = scope.identity.name().to_string();
    let identity_id = scope.identity.id().to_string();

    confirm(
        format,
        assume_yes,
        &format!(
            "Delete identity '{name}' ({identity_id})? \
             Anything authenticating as it will immediately lose access."
        ),
    )?;

    // A project-scoped identity must be deleted through its project; the
    // organization route rejects it with "Identity is managed by project".
    match &scope.project {
        Some(project) => {
            client
                .delete_project_identity(&project.id, &identity_id)
                .await?
        }
        None => client.delete_identity(&identity_id).await?,
    }

    output::success(format, &format!("deleted identity '{name}'"));
    output::emit(
        format,
        &json!({
            "deleted": true,
            "id": identity_id,
            "name": name,
            "scope": if scope.project.is_some() { "project" } else { "organization" },
        }),
        || {},
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn client_secret(
    client: &InfisicalClient,
    query: &str,
    project: Option<&str>,
    description: &str,
    ttl: i64,
    num_uses: i64,
    format: Format,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let identity_id = scope.identity.id().to_string();

    // Minting against an identity without Universal Auth attached fails deep in
    // the API, so attach it first and the command becomes idempotent.
    //
    // Only a 404 means "not attached yet". Treating *every* error as that led
    // to an expired session or a connection reset being answered with an attach
    // attempt, which the API then rejected with "already configured" — telling
    // the user the opposite of what had gone wrong.
    let config = match client.get_universal_auth(&identity_id).await {
        Ok(config) => config,
        Err(IfpError::NotFound(_)) => client.attach_universal_auth(&identity_id).await?,
        Err(err) => return Err(err),
    };

    let secret = client
        .create_client_secret(&identity_id, description, ttl, num_uses)
        .await?;

    output::success(
        format,
        &format!(
            "created client secret for identity '{}'",
            scope.identity.name()
        ),
    );
    output::emit(
        format,
        &json!({
            "id": identity_id,
            "name": scope.identity.name(),
            "clientId": config.client_id,
            "clientSecret": secret.client_secret,
        }),
        || {
            println!("{} {}", "Client ID:    ".bold(), config.client_id);
            println!("{} {}", "Client Secret:".bold(), secret.client_secret);
        },
    );
    output::warn(
        format,
        "the client secret is shown once and cannot be retrieved again",
    );
    Ok(())
}
