use crate::client::InfisicalClient;
use crate::commands::confirm;
use crate::error::{IfpError, Result};
use crate::models::Environment;
use crate::output::{self, Format};
use crate::resolve;

pub async fn list(client: &InfisicalClient, format: Format) -> Result<()> {
    let mut projects = client.list_projects().await?;
    projects.sort_by_key(|p| p.name.to_lowercase());

    output::emit(format, &projects, || output::projects_table(&projects));
    Ok(())
}

pub async fn get(client: &InfisicalClient, query: &str, format: Format) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // Re-fetch by id: the list route omits fields the detail route returns.
    let project = client.get_project(&matched.id).await?;

    output::emit(format, &project, || output::project_detail(&project));
    Ok(())
}

pub async fn create(
    client: &InfisicalClient,
    name: &str,
    slug: Option<&str>,
    description: Option<&str>,
    project_type: &str,
    format: Format,
) -> Result<()> {
    let project = client
        .create_project(name, slug, description, project_type)
        .await?;

    output::success(
        format,
        &format!("created project '{}' ({})", project.name, project.slug),
    );
    output::emit(format, &project, || output::project_detail(&project));
    Ok(())
}

pub async fn rename(
    client: &InfisicalClient,
    query: &str,
    new_name: &str,
    format: Format,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let old_name = matched.name.clone();

    let project = client
        .update_project(&matched.id, Some(new_name), None, None, None)
        .await?;

    output::success(
        format,
        &format!("renamed '{}' to '{}'", old_name, project.name),
    );
    output::emit(format, &project, || {});
    Ok(())
}

pub async fn update(
    client: &InfisicalClient,
    query: &str,
    name: Option<&str>,
    slug: Option<&str>,
    description: Option<&str>,
    delete_protection: Option<bool>,
    format: Format,
) -> Result<()> {
    if name.is_none() && slug.is_none() && description.is_none() && delete_protection.is_none() {
        return Err(IfpError::Config(
            "nothing to update: pass at least one of --name, --slug, --description, \
             --delete-protection or --no-delete-protection"
                .into(),
        ));
    }

    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // Changing a slug breaks anything addressing the project by it — including
    // .infisical.json files and CI config — so it is worth an explicit warning.
    if let Some(slug) = slug {
        if slug != matched.slug {
            output::warn(
                format,
                &format!(
                    "slug '{}' -> '{}': existing references to the old slug will stop resolving",
                    matched.slug, slug
                ),
            );
        }
    }

    let project = client
        .update_project(&matched.id, name, slug, description, delete_protection)
        .await?;

    output::success(format, &format!("updated project '{}'", project.name));
    output::emit(format, &project, || output::project_detail(&project));
    Ok(())
}

pub async fn delete(
    client: &InfisicalClient,
    query: &str,
    assume_yes: bool,
    format: Format,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // Re-fetch by id before reading `hasDeleteProtection`. The list route omits
    // fields the detail route returns and the field defaults to `false` when
    // absent, so trusting the listing would skip the guard exactly when it
    // matters and hand the user an API rejection after they had already
    // confirmed.
    let project = client.get_project(&matched.id).await?;

    // Fail before the prompt rather than after it: the API rejects this and a
    // user who has already typed "y" deserves to know why it could never work.
    if project.has_delete_protection {
        return Err(IfpError::Config(format!(
            "project '{}' has delete protection enabled. Disable it first:\n  \
             inf project update {} --no-delete-protection",
            project.name, project.slug
        )));
    }

    confirm(
        format,
        assume_yes,
        &format!(
            "Delete project '{}' ({})? This removes all of its secrets and cannot be undone.",
            project.name, project.slug
        ),
    )?;

    client.delete_project(&project.id).await?;

    output::success(format, &format!("deleted project '{}'", project.name));
    output::emit(
        format,
        &serde_json::json!({
            "deleted": true,
            "id": project.id,
            "name": project.name,
            "slug": project.slug,
        }),
        || {},
    );
    Ok(())
}

/// Which machine identities can reach this project, and with what role.
/// Answering "who has access to X" otherwise means paging through the web UI.
pub async fn identities(client: &InfisicalClient, query: &str, format: Format) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let mut memberships = client.list_project_identities(&matched.id).await?;
    memberships.sort_by_key(|m| m.name().to_lowercase());

    output::emit(format, &memberships, || {
        output::project_identities_table(&memberships)
    });
    Ok(())
}

// -- Environments --

pub async fn env_list(client: &InfisicalClient, query: &str, format: Format) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let project = client.get_project(&matched.id).await?;

    output::emit(format, &project.environments, || {
        let rows: Vec<Vec<String>> = project
            .environments
            .iter()
            .map(|e| vec![e.name.clone(), e.slug.clone(), e.id.clone()])
            .collect();
        output::table(&["NAME", "SLUG", "ID"], &rows);
    });
    Ok(())
}

pub async fn env_create(
    client: &InfisicalClient,
    query: &str,
    name: &str,
    slug: Option<&str>,
    format: Format,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // The API requires a slug; derive a conventional one rather than making
    // the caller supply an obvious value.
    let derived;
    let slug = match slug {
        Some(slug) => slug,
        None => {
            derived = slugify(name);
            if derived.is_empty() {
                // Names with no ASCII alphanumerics — "Продакшн", "本番" —
                // cannot produce a slug. Say so instead of posting an empty
                // one and relaying whatever the API makes of it.
                return Err(IfpError::Config(format!(
                    "cannot derive a slug from '{name}': it has no ASCII letters or digits. \
                     Pass --slug explicitly"
                )));
            }
            &derived
        }
    };

    client.create_environment(&matched.id, name, slug).await?;

    // Report the environment as it now exists rather than the create response,
    // whose envelope differs by version and carries no id the caller can use.
    let created = reread_environment(client, &matched.id, slug).await?;

    output::success(
        format,
        &format!(
            "created environment '{}' ({}) in project '{}'",
            created.name, created.slug, matched.name
        ),
    );
    output::emit(format, &created, || {});
    Ok(())
}

pub async fn env_update(
    client: &InfisicalClient,
    query: &str,
    env_query: &str,
    name: Option<&str>,
    slug: Option<&str>,
    format: Format,
) -> Result<()> {
    if name.is_none() && slug.is_none() {
        return Err(IfpError::Config(
            "nothing to update: pass --name or --slug".into(),
        ));
    }

    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let project = client.get_project(&matched.id).await?;
    let env = resolve::environment(&project, env_query)?;
    let env_id = env.id.clone();

    // An environment slug is part of every secret path in it, so changing one
    // breaks more references than a project slug does.
    if let Some(slug) = slug {
        if slug != env.slug {
            output::warn(
                format,
                &format!(
                    "slug '{}' -> '{}': secret paths and CI config referencing the old slug \
                     will stop resolving",
                    env.slug, slug
                ),
            );
        }
    }

    client
        .update_environment(&project.id, &env_id, name, slug)
        .await?;

    // Re-read so the reported slug is the new one; reporting `env.slug` here
    // echoed the value the user had just replaced.
    let project = client.get_project(&matched.id).await?;
    let updated = project
        .environments
        .iter()
        .find(|e| e.id == env_id)
        .cloned()
        .ok_or_else(|| IfpError::Api("environment vanished after update".into()))?;

    output::success(format, &format!("updated environment '{}'", updated.slug));
    output::emit(format, &updated, || {});
    Ok(())
}

pub async fn env_delete(
    client: &InfisicalClient,
    query: &str,
    env_query: &str,
    assume_yes: bool,
    format: Format,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let project = client.get_project(&matched.id).await?;
    let env = resolve::environment(&project, env_query)?.clone();

    confirm(
        format,
        assume_yes,
        &format!(
            "Delete environment '{}' from project '{}'? All secrets in it are removed.",
            env.slug, project.name
        ),
    )?;

    client.delete_environment(&project.id, &env.id).await?;

    output::success(format, &format!("deleted environment '{}'", env.slug));
    output::emit(
        format,
        &serde_json::json!({
            "deleted": true,
            "id": env.id,
            "name": env.name,
            "slug": env.slug,
            "projectId": project.id,
        }),
        || {},
    );
    Ok(())
}

async fn reread_environment(
    client: &InfisicalClient,
    project_id: &str,
    slug: &str,
) -> Result<Environment> {
    let project = client.get_project(project_id).await?;
    project
        .environments
        .iter()
        .find(|e| e.slug == slug)
        .cloned()
        .ok_or_else(|| IfpError::Api(format!("environment '{slug}' not present after create")))
}

/// Infisical environment slugs are lowercase and hyphen-separated. Collapse
/// runs of separators so "Staging  Env" does not become "staging--env".
///
/// Non-ASCII characters are separators, not letters: "Café" derives `cafe`
/// only if the accent is already decomposed, so callers get `caf`. That is
/// lossy but valid; a name with *no* ASCII alphanumerics yields an empty
/// string, which `env_create` rejects rather than sending.
fn slugify(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut pending_hyphen = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_lowercases_and_hyphenates() {
        assert_eq!(slugify("Staging"), "staging");
        assert_eq!(slugify("Pre Production"), "pre-production");
    }

    #[test]
    fn slugify_collapses_runs_of_separators() {
        assert_eq!(slugify("Staging  Env"), "staging-env");
        assert_eq!(slugify("a__b--c"), "a-b-c");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_separators() {
        assert_eq!(slugify("  Staging  "), "staging");
        assert_eq!(slugify("-dev-"), "dev");
    }

    #[test]
    fn slugify_drops_non_ascii_punctuation() {
        assert_eq!(slugify("QA (EU)"), "qa-eu");
    }

    #[test]
    fn slugify_yields_empty_for_names_with_no_ascii_alphanumerics() {
        // env_create turns this into an actionable error instead of posting an
        // empty slug.
        assert_eq!(slugify("Продакшн"), "");
        assert_eq!(slugify("本番"), "");
        assert_eq!(slugify("!!!"), "");
    }
}
