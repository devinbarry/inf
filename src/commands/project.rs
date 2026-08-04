use crate::client::InfisicalClient;
use crate::commands::confirm;
use crate::error::{IfpError, Result};
use crate::output;
use crate::resolve;

pub async fn list(client: &InfisicalClient, json: bool) -> Result<()> {
    let mut projects = client.list_projects().await?;
    projects.sort_by_key(|p| p.name.to_lowercase());

    if json {
        output::json_raw(&serde_json::to_value(&projects).unwrap_or_default());
    } else {
        output::projects_table(&projects);
    }
    Ok(())
}

pub async fn get(client: &InfisicalClient, query: &str, json: bool) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // Re-fetch by id: the list route omits fields the detail route returns.
    let project = client.get_project(&matched.id).await?;

    if json {
        output::json_raw(&serde_json::to_value(&project).unwrap_or_default());
    } else {
        output::project_detail(&project);
    }
    Ok(())
}

pub async fn create(
    client: &InfisicalClient,
    name: &str,
    slug: Option<&str>,
    description: Option<&str>,
    project_type: &str,
    json: bool,
) -> Result<()> {
    let project = client
        .create_project(name, slug, description, project_type)
        .await?;

    if json {
        output::json_raw(&serde_json::to_value(&project).unwrap_or_default());
    } else {
        output::success(&format!(
            "created project '{}' ({})",
            project.name, project.slug
        ));
        output::project_detail(&project);
    }
    Ok(())
}

pub async fn rename(
    client: &InfisicalClient,
    query: &str,
    new_name: &str,
    json: bool,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let old_name = matched.name.clone();

    let project = client
        .update_project(&matched.id, Some(new_name), None, None, None)
        .await?;

    if json {
        output::json_raw(&serde_json::to_value(&project).unwrap_or_default());
    } else {
        output::success(&format!("renamed '{}' to '{}'", old_name, project.name));
    }
    Ok(())
}

pub async fn update(
    client: &InfisicalClient,
    query: &str,
    name: Option<&str>,
    slug: Option<&str>,
    description: Option<&str>,
    delete_protection: Option<bool>,
    json: bool,
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
            output::warn(&format!(
                "slug '{}' -> '{}': existing references to the old slug will stop resolving",
                matched.slug, slug
            ));
        }
    }

    let project = client
        .update_project(&matched.id, name, slug, description, delete_protection)
        .await?;

    if json {
        output::json_raw(&serde_json::to_value(&project).unwrap_or_default());
    } else {
        output::success(&format!("updated project '{}'", project.name));
        output::project_detail(&project);
    }
    Ok(())
}

pub async fn delete(client: &InfisicalClient, query: &str, assume_yes: bool) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // Fail before the prompt rather than after it: the API rejects this and a
    // user who has already typed "y" deserves to know why it could never work.
    if matched.has_delete_protection {
        return Err(IfpError::Config(format!(
            "project '{}' has delete protection enabled. Disable it first:\n  \
             inf project update {} --no-delete-protection",
            matched.name, matched.slug
        )));
    }

    confirm(
        assume_yes,
        &format!(
            "Delete project '{}' ({})? This removes all of its secrets and cannot be undone.",
            matched.name, matched.slug
        ),
    )?;

    client.delete_project(&matched.id).await?;
    output::success(&format!("deleted project '{}'", matched.name));
    Ok(())
}

/// Which machine identities can reach this project, and with what role.
/// Answering "who has access to X" otherwise means paging through the web UI.
pub async fn identities(client: &InfisicalClient, query: &str, json: bool) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let mut memberships = client.list_project_identities(&matched.id).await?;
    memberships.sort_by_key(|m| m.name().to_lowercase());

    if json {
        output::json_raw(&serde_json::to_value(&memberships).unwrap_or_default());
        return Ok(());
    }

    output::project_identities_table(&memberships);
    Ok(())
}

// -- Environments --

pub async fn env_list(client: &InfisicalClient, query: &str, json: bool) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let project = client.get_project(&matched.id).await?;

    if json {
        output::json_raw(&serde_json::to_value(&project.environments).unwrap_or_default());
    } else {
        let rows: Vec<Vec<String>> = project
            .environments
            .iter()
            .map(|e| vec![e.name.clone(), e.slug.clone(), e.id.clone()])
            .collect();
        output::table(&["NAME", "SLUG", "ID"], &rows);
    }
    Ok(())
}

pub async fn env_create(
    client: &InfisicalClient,
    query: &str,
    name: &str,
    slug: Option<&str>,
    json: bool,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;

    // The API requires a slug; derive a conventional one rather than making
    // the caller supply an obvious value.
    let derived = slugify(name);
    let slug = slug.unwrap_or(&derived);

    let response = client.create_environment(&matched.id, name, slug).await?;

    if json {
        output::json_raw(&response);
    } else {
        output::success(&format!(
            "created environment '{}' ({}) in project '{}'",
            name, slug, matched.name
        ));
    }
    Ok(())
}

pub async fn env_update(
    client: &InfisicalClient,
    query: &str,
    env_query: &str,
    name: Option<&str>,
    slug: Option<&str>,
    json: bool,
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

    let response = client
        .update_environment(&project.id, &env.id, name, slug)
        .await?;

    if json {
        output::json_raw(&response);
    } else {
        output::success(&format!("updated environment '{}'", env.slug));
    }
    Ok(())
}

pub async fn env_delete(
    client: &InfisicalClient,
    query: &str,
    env_query: &str,
    assume_yes: bool,
) -> Result<()> {
    let projects = client.list_projects().await?;
    let matched = resolve::project(&projects, query)?;
    let project = client.get_project(&matched.id).await?;
    let env = resolve::environment(&project, env_query)?;

    confirm(
        assume_yes,
        &format!(
            "Delete environment '{}' from project '{}'? All secrets in it are removed.",
            env.slug, project.name
        ),
    )?;

    client.delete_environment(&project.id, &env.id).await?;
    output::success(&format!("deleted environment '{}'", env.slug));
    Ok(())
}

/// Infisical environment slugs are lowercase and hyphen-separated. Collapse
/// runs of separators so "Staging  Env" does not become "staging--env".
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
}
