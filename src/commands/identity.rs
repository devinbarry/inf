use crate::client::InfisicalClient;
use crate::commands::confirm;
use crate::error::{IfpError, Result};
use crate::models::IdentityMembership;
use crate::output;
use crate::resolve;
use colored::Colorize;

/// Where to look for an identity.
///
/// Project-scoped identities do **not** appear in the organization listing, so
/// a command that only ever searched the org would be unable to manage the very
/// identities this tool creates. `--project` selects the project listing;
/// without it we fall back to the organization, which is how pre-existing
/// org-level identities remain manageable (and deletable).
struct Scope {
    identity: IdentityMembership,
    project_id: Option<String>,
    project_name: Option<String>,
}

async fn resolve_scope(
    client: &InfisicalClient,
    project: Option<&str>,
    query: &str,
) -> Result<Scope> {
    match project {
        Some(project_query) => {
            let projects = client.list_projects().await?;
            let matched = resolve::project(&projects, project_query)?;
            let identities = client.list_project_identities(&matched.id).await?;
            let identity = resolve::identity(&identities, query)?.clone();
            Ok(Scope {
                identity,
                project_id: Some(matched.id.clone()),
                project_name: Some(matched.name.clone()),
            })
        }
        None => {
            let identities = client.list_identities().await?;
            let identity = resolve::identity(&identities, query)?.clone();
            Ok(Scope {
                identity,
                project_id: None,
                project_name: None,
            })
        }
    }
}

pub async fn list(client: &InfisicalClient, project: Option<&str>, json: bool) -> Result<()> {
    let mut identities = match project {
        Some(project_query) => {
            let projects = client.list_projects().await?;
            let matched = resolve::project(&projects, project_query)?;
            client.list_project_identities(&matched.id).await?
        }
        None => client.list_identities().await?,
    };
    identities.sort_by_key(|i| i.name().to_lowercase());

    if json {
        output::json_raw(&serde_json::to_value(&identities).unwrap_or_default());
        return Ok(());
    }

    match project {
        Some(_) => output::project_identities_table(&identities),
        None => output::identities_table(&identities),
    }
    if project.is_none() {
        println!(
            "{}",
            "Organization-level identities only. Use --project to list a project's identities."
                .dimmed()
        );
    }
    Ok(())
}

pub async fn get(
    client: &InfisicalClient,
    query: &str,
    project: Option<&str>,
    json: bool,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let identity = &scope.identity;

    if json {
        output::json_raw(&serde_json::to_value(identity).unwrap_or_default());
        return Ok(());
    }

    println!("{} {}", "Name:".bold(), identity.name());
    println!("{} {}", "ID:".bold(), identity.id());
    match (&scope.project_name, identity.role()) {
        (Some(project_name), role) => {
            println!("{} project '{}'", "Scope:".bold(), project_name);
            println!("{} {}", "Project role:".bold(), role);
        }
        (None, role) => {
            println!("{} {}", "Scope:".bold(), "organization".yellow());
            println!("{} {}", "Org role:".bold(), role);
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

    // The Universal Auth client id is the half of the credential pair that is
    // safe to display and is required for every machine login.
    if identity
        .identity
        .auth_methods
        .iter()
        .any(|m| m == "universal-auth")
    {
        if let Ok(config) = client.get_universal_auth(identity.id()).await {
            println!("\n{}", "Universal Auth".bold());
            println!("{} {}", "  Client ID:".bold(), config.client_id);
            if let Some(ttl) = config.access_token_ttl {
                println!("{} {}s", "  Access token TTL:".bold(), ttl);
            }
        }
    }
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
    json: bool,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, project)?;

    let created = client
        .create_project_identity(&matched.id, name, role)
        .await?;

    let identity_id = created
        .get("identity")
        .and_then(|i| i.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| IfpError::Api("create returned no identity id".into()))?
        .to_string();

    // The create route accepts a `roles` array but ignores it, leaving the
    // identity on `no-access`. Apply the requested role explicitly so the
    // identity is actually usable at the level asked for.
    client
        .set_project_identity_role(&matched.id, &identity_id, role)
        .await?;

    if !with_universal_auth {
        if json {
            output::json_raw(&created);
        } else {
            output::success(&format!(
                "created identity '{name}' scoped to project '{}' as {role}",
                matched.name
            ));
            println!("{} {}", "Identity ID:".bold(), identity_id);
            println!(
                "{}",
                format!(
                    "Attach an auth method before use: \
                     inf identity client-secret {name} --project {}",
                    matched.slug
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    // A fresh identity has no auth method and cannot log in. Attach Universal
    // Auth and mint one secret so the caller ends up with a usable credential
    // pair rather than a half-configured identity.
    let config = client.attach_universal_auth(&identity_id).await?;
    let secret = client
        .create_client_secret(&identity_id, "created by inf", 0, 0)
        .await?;

    if json {
        output::json_raw(&serde_json::json!({
            "identity": created.get("identity"),
            "projectId": matched.id,
            "role": role,
            "clientId": config.client_id,
            "clientSecret": secret.client_secret,
        }));
        return Ok(());
    }

    output::success(&format!(
        "created identity '{name}' scoped to project '{}' as {role}",
        matched.name
    ));
    println!("{} {}", "Identity ID: ".bold(), identity_id);
    println!("{} {}", "Client ID:   ".bold(), config.client_id);
    println!("{} {}", "Client Secret:".bold(), secret.client_secret);
    // Not retrievable after this response, so warn while it is still on screen.
    output::warn("the client secret is shown once and cannot be retrieved again");
    Ok(())
}

/// Rename only. Organization roles are never granted by this tool, so the
/// underlying route's `role` field is not exposed.
pub async fn rename(
    client: &InfisicalClient,
    query: &str,
    new_name: &str,
    project: Option<&str>,
    json: bool,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;
    let old_name = scope.identity.name().to_string();

    let response = client
        .update_identity(scope.identity.id(), new_name)
        .await?;

    if json {
        output::json_raw(&response);
    } else {
        output::success(&format!("renamed identity '{old_name}' to '{new_name}'"));
    }
    Ok(())
}

/// Change an identity's role within the project it is scoped to.
pub async fn set_role(
    client: &InfisicalClient,
    query: &str,
    project: &str,
    role: &str,
    json: bool,
) -> Result<()> {
    let scope = resolve_scope(client, Some(project), query).await?;
    let project_id = scope
        .project_id
        .as_deref()
        .ok_or_else(|| IfpError::Config("a project is required to set a role".into()))?;

    let response = client
        .set_project_identity_role(project_id, scope.identity.id(), role)
        .await?;

    if json {
        output::json_raw(&response);
    } else {
        output::success(&format!(
            "set identity '{}' to role '{role}' in project '{}'",
            scope.identity.name(),
            scope.project_name.as_deref().unwrap_or("?")
        ));
    }
    Ok(())
}

pub async fn delete(
    client: &InfisicalClient,
    query: &str,
    project: Option<&str>,
    assume_yes: bool,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;

    confirm(
        assume_yes,
        &format!(
            "Delete identity '{}' ({})? Anything authenticating as it will immediately lose access.",
            scope.identity.name(),
            scope.identity.id()
        ),
    )?;

    // A project-scoped identity must be deleted through its project; the
    // organization route rejects it with "Identity is managed by project".
    match scope.project_id.as_deref() {
        Some(project_id) => {
            client
                .delete_project_identity(project_id, scope.identity.id())
                .await?
        }
        None => client.delete_identity(scope.identity.id()).await?,
    }

    output::success(&format!("deleted identity '{}'", scope.identity.name()));
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
    json: bool,
) -> Result<()> {
    let scope = resolve_scope(client, project, query).await?;

    // Minting against an identity without Universal Auth attached fails deep in
    // the API; attach it first so the command is idempotent for the caller.
    let config = match client.get_universal_auth(scope.identity.id()).await {
        Ok(config) => config,
        Err(_) => client.attach_universal_auth(scope.identity.id()).await?,
    };

    let secret = client
        .create_client_secret(scope.identity.id(), description, ttl, num_uses)
        .await?;

    if json {
        output::json_raw(&serde_json::json!({
            "identityId": scope.identity.id(),
            "clientId": config.client_id,
            "clientSecret": secret.client_secret,
        }));
        return Ok(());
    }

    output::success(&format!(
        "created client secret for identity '{}'",
        scope.identity.name()
    ));
    println!("{} {}", "Client ID:    ".bold(), config.client_id);
    println!("{} {}", "Client Secret:".bold(), secret.client_secret);
    output::warn("the client secret is shown once and cannot be retrieved again");
    Ok(())
}
