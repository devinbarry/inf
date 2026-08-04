use crate::error::{IfpError, Result};
use crate::models::{IdentityMembership, Project};

/// Resolving a human-typed query to exactly one object is the difference
/// between a safe delete and a destructive one. The rule is deliberately
/// strict: an exact id, slug or name always wins outright, and a substring is
/// only accepted when it matches a single candidate. Anything else is an error
/// listing the candidates — never a silent "first match".
pub fn project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project> {
    if let Some(exact) = projects
        .iter()
        .find(|p| p.id == query || p.slug == query || p.name == query)
    {
        return Ok(exact);
    }

    let lowered = query.to_lowercase();
    let matches: Vec<&Project> = projects
        .iter()
        .filter(|p| {
            p.name.to_lowercase().contains(&lowered) || p.slug.to_lowercase().contains(&lowered)
        })
        .collect();

    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(IfpError::NoMatch(format!("no project matching '{query}'"))),
        _ => Err(IfpError::AmbiguousMatch {
            query: query.to_string(),
            candidates: matches
                .iter()
                .map(|p| format!("  {} ({})", p.name, p.slug))
                .collect::<Vec<_>>()
                .join("\n"),
        }),
    }
}

pub fn identity<'a>(
    identities: &'a [IdentityMembership],
    query: &str,
) -> Result<&'a IdentityMembership> {
    if let Some(exact) = identities
        .iter()
        .find(|i| i.id() == query || i.name() == query)
    {
        return Ok(exact);
    }

    let lowered = query.to_lowercase();
    let matches: Vec<&IdentityMembership> = identities
        .iter()
        .filter(|i| i.name().to_lowercase().contains(&lowered))
        .collect();

    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(IfpError::NoMatch(format!("no identity matching '{query}'"))),
        _ => Err(IfpError::AmbiguousMatch {
            query: query.to_string(),
            candidates: matches
                .iter()
                .map(|i| format!("  {} ({})", i.name(), i.id()))
                .collect::<Vec<_>>()
                .join("\n"),
        }),
    }
}

/// Environments are scoped to one project, so a slug match is unambiguous and
/// the name is accepted only as a convenience.
pub fn environment<'a>(
    project: &'a Project,
    query: &str,
) -> Result<&'a crate::models::Environment> {
    project
        .environments
        .iter()
        .find(|e| e.slug == query || e.id == query || e.name == query)
        .or_else(|| {
            let lowered = query.to_lowercase();
            project
                .environments
                .iter()
                .find(|e| e.name.to_lowercase() == lowered)
        })
        .ok_or_else(|| {
            IfpError::NoMatch(format!(
                "no environment '{}' in project '{}' (have: {})",
                query,
                project.name,
                project
                    .environments
                    .iter()
                    .map(|e| e.slug.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_fixture(id: &str, name: &str, slug: &str) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "slug": slug
        }))
        .unwrap()
    }

    fn identity_fixture(id: &str, name: &str) -> IdentityMembership {
        serde_json::from_value(serde_json::json!({
            "identityId": id,
            "identity": {"id": id, "name": name, "authMethods": []}
        }))
        .unwrap()
    }

    #[test]
    fn exact_slug_beats_substring_of_another_project() {
        // "api" is an exact slug and also a substring of "api-gateway".
        // Without the exact-match rule this would be ambiguous and a delete
        // would be blocked, or worse, hit the wrong project.
        let projects = vec![
            project_fixture("p1", "API", "api"),
            project_fixture("p2", "API Gateway", "api-gateway"),
        ];
        assert_eq!(project(&projects, "api").unwrap().id, "p1");
    }

    #[test]
    fn exact_id_resolves() {
        let projects = vec![project_fixture("p1", "billing", "billing")];
        assert_eq!(project(&projects, "p1").unwrap().slug, "billing");
    }

    #[test]
    fn unique_substring_resolves_case_insensitively() {
        let projects = vec![
            project_fixture("p1", "Billing API", "billing-api"),
            project_fixture("p2", "Payments", "payments"),
        ];
        assert_eq!(project(&projects, "BILL").unwrap().id, "p1");
    }

    #[test]
    fn ambiguous_substring_errors_and_lists_candidates() {
        let projects = vec![
            project_fixture("p1", "Billing API", "billing-api"),
            project_fixture("p2", "Billing Web", "billing-web"),
        ];
        let err = project(&projects, "billing").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("billing-api"));
        assert!(msg.contains("billing-web"));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn no_match_errors() {
        let projects = vec![project_fixture("p1", "billing", "billing")];
        assert!(matches!(
            project(&projects, "nonexistent"),
            Err(IfpError::NoMatch(_))
        ));
    }

    #[test]
    fn empty_list_is_no_match_not_panic() {
        assert!(project(&[], "anything").is_err());
        assert!(identity(&[], "anything").is_err());
    }

    #[test]
    fn identity_resolves_by_name_and_id() {
        let identities = vec![
            identity_fixture("i1", "ci-deployer"),
            identity_fixture("i2", "backup-agent"),
        ];
        assert_eq!(identity(&identities, "ci-deployer").unwrap().id(), "i1");
        assert_eq!(identity(&identities, "i2").unwrap().name(), "backup-agent");
        assert_eq!(identity(&identities, "backup").unwrap().id(), "i2");
    }

    #[test]
    fn identity_ambiguity_is_rejected() {
        let identities = vec![
            identity_fixture("i1", "ci-deployer-a"),
            identity_fixture("i2", "ci-deployer-b"),
        ];
        assert!(matches!(
            identity(&identities, "ci-deployer"),
            Err(IfpError::AmbiguousMatch { .. })
        ));
    }

    #[test]
    fn environment_resolves_by_slug_and_lists_options_when_missing() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "id": "p1", "name": "billing", "slug": "billing",
            "environments": [
                {"id": "e1", "name": "Development", "slug": "dev"},
                {"id": "e2", "name": "Production", "slug": "prod"}
            ]
        }))
        .unwrap();

        assert_eq!(environment(&project, "dev").unwrap().id, "e1");
        assert_eq!(environment(&project, "Production").unwrap().id, "e2");

        let err = environment(&project, "staging").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("dev, prod"));
    }
}
