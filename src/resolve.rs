use crate::error::{Candidate, IfpError, Result};
use crate::models::{Environment, IdentityMembership, Project};

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
                .map(|p| Candidate {
                    name: p.name.clone(),
                    id: p.id.clone(),
                    slug: Some(p.slug.clone()),
                })
                .collect(),
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
                .map(|i| Candidate {
                    name: i.name().to_string(),
                    id: i.id().to_string(),
                    slug: None,
                })
                .collect(),
        }),
    }
}

/// Environments are scoped to one project, so an exact slug, id or name is
/// unambiguous. A case-insensitive name is accepted as a convenience — but it
/// is resolved under the same rule as everything else here: two environments
/// differing only in case are an error, not a coin flip.
///
/// Substrings are deliberately *not* accepted. `dev` would otherwise match
/// `dev` and `development`, and the losing case is a deleted environment.
pub fn environment<'a>(project: &'a Project, query: &str) -> Result<&'a Environment> {
    if let Some(exact) = project
        .environments
        .iter()
        .find(|e| e.slug == query || e.id == query || e.name == query)
    {
        return Ok(exact);
    }

    let lowered = query.to_lowercase();
    let matches: Vec<&Environment> = project
        .environments
        .iter()
        .filter(|e| e.name.to_lowercase() == lowered || e.slug.to_lowercase() == lowered)
        .collect();

    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(IfpError::NoMatch(format!(
            "no environment '{}' in project '{}' (have: {})",
            query,
            project.name,
            project
                .environments
                .iter()
                .map(|e| e.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
        _ => Err(IfpError::AmbiguousMatch {
            query: query.to_string(),
            candidates: matches
                .iter()
                .map(|e| Candidate {
                    name: e.name.clone(),
                    id: e.id.clone(),
                    slug: Some(e.slug.clone()),
                })
                .collect(),
        }),
    }
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

    fn project_with_envs(envs: serde_json::Value) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": "p1", "name": "billing", "slug": "billing", "environments": envs
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

        // Agents disambiguate by id, so the ids must survive into the payload.
        let json = err.to_json();
        assert_eq!(json["candidates"][0]["id"], "p1");
        assert_eq!(json["candidates"][1]["id"], "p2");
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
        let err = identity(&identities, "ci-deployer").unwrap_err();
        assert!(matches!(err, IfpError::AmbiguousMatch { .. }));
        // Identities have no slug, so the candidate carries only name and id.
        let json = err.to_json();
        assert_eq!(json["candidates"][0]["id"], "i1");
        assert!(json["candidates"][0].get("slug").is_none());
    }

    #[test]
    fn environment_resolves_by_slug_and_lists_options_when_missing() {
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "Development", "slug": "dev"},
            {"id": "e2", "name": "Production", "slug": "prod"}
        ]));

        assert_eq!(environment(&project, "dev").unwrap().id, "e1");
        assert_eq!(environment(&project, "Production").unwrap().id, "e2");

        let err = environment(&project, "staging").unwrap_err();
        assert!(err.to_string().contains("dev, prod"));
    }

    #[test]
    fn environment_matches_name_case_insensitively() {
        // This fallback previously had no coverage at all.
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "Development", "slug": "dev"}
        ]));
        assert_eq!(environment(&project, "development").unwrap().id, "e1");
        assert_eq!(environment(&project, "DEVELOPMENT").unwrap().id, "e1");
    }

    #[test]
    fn environment_case_collision_is_ambiguous_not_first_match() {
        // The fallback used to take the first match silently, contradicting
        // this module's own rule — and the losing case is a deleted
        // environment full of secrets.
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "QA", "slug": "qa"},
            {"id": "e2", "name": "qa", "slug": "qa-eu"}
        ]));
        let err = environment(&project, "Qa").unwrap_err();
        assert!(matches!(err, IfpError::AmbiguousMatch { .. }));
        let json = err.to_json();
        assert_eq!(json["candidates"][0]["id"], "e1");
        assert_eq!(json["candidates"][1]["id"], "e2");
    }

    #[test]
    fn environment_exact_match_wins_over_a_case_collision() {
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "QA", "slug": "qa"},
            {"id": "e2", "name": "qa", "slug": "qa-eu"}
        ]));
        assert_eq!(environment(&project, "qa").unwrap().id, "e1");
        assert_eq!(environment(&project, "qa-eu").unwrap().id, "e2");
    }

    #[test]
    fn environment_does_not_accept_substrings() {
        // `dev` must not silently select `development`.
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "Development", "slug": "development"}
        ]));
        assert!(environment(&project, "dev").is_err());
    }
}
