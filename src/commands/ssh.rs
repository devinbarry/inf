use crate::client::InfisicalClient;
use crate::error::{Candidate, IfpError, Result};
use crate::models::SshCa;
use crate::output::{self, Format};
use crate::resolve;

async fn project_id(client: &InfisicalClient, query: &str) -> Result<String> {
    let projects = client.list_projects().await?;
    let project = resolve::project(&projects, query)?;
    if project.project_type.as_deref() != Some("ssh") {
        return Err(IfpError::Config(format!(
            "project '{}' is not an SSH project",
            project.name
        )));
    }
    Ok(project.id.clone())
}

fn resolve_ca<'a>(cas: &'a [SshCa], query: &str) -> Result<&'a SshCa> {
    let exact: Vec<&SshCa> = cas
        .iter()
        .filter(|ca| ca.id == query || ca.friendly_name == query)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let lowered = query.to_lowercase();
    let matches: Vec<&SshCa> = cas
        .iter()
        .filter(|ca| ca.friendly_name.to_lowercase().contains(&lowered))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(IfpError::NoMatch(format!("no SSH CA matching '{query}'"))),
        _ => Err(IfpError::AmbiguousMatch {
            query: query.to_string(),
            candidates: matches
                .into_iter()
                .map(|ca| Candidate {
                    name: ca.friendly_name.clone(),
                    id: ca.id.clone(),
                    slug: None,
                })
                .collect(),
        }),
    }
}

pub async fn ca_list(client: &InfisicalClient, project: &str, format: Format) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let cas = client.list_ssh_cas(&project_id).await?;
    output::emit(format, &cas, || output::ssh_cas_table(&cas));
    Ok(())
}

pub async fn ca_update(
    client: &InfisicalClient,
    project: &str,
    ca: &str,
    name: &str,
    status: &str,
    format: Format,
) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let cas = client.list_ssh_cas(&project_id).await?;
    let matched = resolve_ca(&cas, ca)?;
    let updated = client.update_ssh_ca(&matched.id, name, status).await?;
    output::emit(format, &updated, || {
        output::ssh_cas_table(std::slice::from_ref(&updated))
    });
    Ok(())
}

pub async fn ca_public_key(
    client: &InfisicalClient,
    project: &str,
    ca: &str,
    format: Format,
) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let cas = client.list_ssh_cas(&project_id).await?;
    let matched = resolve_ca(&cas, ca)?;
    let public_key = client.get_ssh_ca_public_key(&matched.id).await?;
    output::emit(
        format,
        &serde_json::json!({
            "caId": matched.id,
            "projectId": matched.project_id,
            "friendlyName": matched.friendly_name,
            "publicKey": public_key,
        }),
        || println!("{public_key}"),
    );
    Ok(())
}

pub async fn template_list(client: &InfisicalClient, project: &str, format: Format) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let templates = client.list_ssh_certificate_templates(&project_id).await?;
    output::emit(format, &templates, || {
        output::ssh_templates_table(&templates)
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn template_create(
    client: &InfisicalClient,
    project: &str,
    ca: &str,
    name: &str,
    principals: &[String],
    ttl: &str,
    max_ttl: &str,
    format: Format,
) -> Result<()> {
    if principals.is_empty() {
        return Err(IfpError::Config(
            "at least one --principal is required".into(),
        ));
    }
    let project_id = project_id(client, project).await?;
    let existing = client.list_ssh_certificate_templates(&project_id).await?;
    if existing.iter().any(|template| template.name == name) {
        return Err(IfpError::Config(format!(
            "SSH certificate template '{name}' already exists"
        )));
    }
    let cas = client.list_ssh_cas(&project_id).await?;
    let matched = resolve_ca(&cas, ca)?;
    let template = client
        .create_ssh_user_certificate_template(&matched.id, name, principals, ttl, max_ttl)
        .await?;
    output::emit(format, &template, || {
        output::ssh_templates_table(std::slice::from_ref(&template))
    });
    Ok(())
}

pub async fn issuer_role_list(
    client: &InfisicalClient,
    project: &str,
    format: Format,
) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let roles = client.list_ssh_issuer_roles(&project_id).await?;
    output::emit(format, &roles, || {
        for role in &roles {
            println!("{}\t{}\t{}", role.id, role.slug, role.name);
        }
    });
    Ok(())
}

pub async fn issuer_role_create(
    client: &InfisicalClient,
    project: &str,
    name: &str,
    slug: &str,
    format: Format,
) -> Result<()> {
    let project_id = project_id(client, project).await?;
    let roles = client.list_ssh_issuer_roles(&project_id).await?;
    if roles
        .iter()
        .any(|role| role.slug == slug || role.name == name)
    {
        return Err(IfpError::Config(format!(
            "SSH issuer role '{name}' or slug '{slug}' already exists"
        )));
    }
    let role = client
        .create_ssh_issuer_role(&project_id, name, slug)
        .await?;
    output::emit(format, &role, || {
        println!("{}\t{}\t{}", role.id, role.slug, role.name)
    });
    Ok(())
}
