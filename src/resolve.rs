//! Resolving a human-typed query to exactly one object is the difference
//! between a safe delete and a destructive one.
//!
//! The rule is deliberately strict: exact matches are tried in precedence
//! order — id, then slug, then name — and a substring is only accepted when it
//! matches a single candidate. Anything else is an error listing the
//! candidates, never a silent "first match".
//!
//! The tiers matter because Infisical enforces uniqueness on ids and slugs but
//! **not** on names: two projects may legitimately share a name, and one
//! project's slug may equal another's name. Matching all three fields in one
//! pass and taking the first hit made the winner depend on the order the API
//! happened to return rows — and on `delete`, that is a silent wrong target.

use crate::error::{Candidate, IfpError, Result};
use crate::models::{Environment, IdentityMembership, Project};

/// Collapse one precedence tier into a decision: nothing here, exactly one, or
/// an ambiguity the caller must resolve.
fn one_of<'a, T>(
    matches: Vec<&'a T>,
    query: &str,
    describe: impl Fn(&T) -> Candidate,
) -> Resolution<'a, T> {
    match matches.len() {
        0 => Resolution::None,
        1 => Resolution::One(matches[0]),
        _ => Resolution::Many(IfpError::AmbiguousMatch {
            query: query.to_string(),
            candidates: matches.into_iter().map(describe).collect(),
        }),
    }
}

enum Resolution<'a, T> {
    None,
    One(&'a T),
    Many(IfpError),
}

pub fn project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project> {
    let describe = |p: &Project| Candidate {
        name: p.name.clone(),
        id: p.id.clone(),
        slug: Some(p.slug.clone()),
    };

    for field in [
        (|p: &Project, q: &str| p.id == q) as fn(&Project, &str) -> bool,
        |p, q| p.slug == q,
        |p, q| p.name == q,
    ] {
        let tier: Vec<&Project> = projects.iter().filter(|p| field(p, query)).collect();
        match one_of(tier, query, describe) {
            Resolution::One(found) => return Ok(found),
            Resolution::Many(err) => return Err(err),
            Resolution::None => {}
        }
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
    let describe = |i: &IdentityMembership| Candidate {
        name: i.name().to_string(),
        id: i.id().to_string(),
        slug: None,
    };

    // Identities have no slug, so the tiers are id then name. Names are not
    // unique — a failed `create` whose cleanup also failed leaves two
    // identities sharing one — and that is exactly when a `delete` must refuse
    // rather than guess.
    for field in [
        (|i: &IdentityMembership, q: &str| i.id() == q) as fn(&IdentityMembership, &str) -> bool,
        |i, q| i.name() == q,
    ] {
        let tier: Vec<&IdentityMembership> =
            identities.iter().filter(|i| field(i, query)).collect();
        match one_of(tier, query, describe) {
            Resolution::One(found) => return Ok(found),
            Resolution::Many(err) => return Err(err),
            Resolution::None => {}
        }
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

/// Environments are scoped to one project, so an exact id, slug or name is
/// unambiguous.
///
/// The three tiers are **case-sensitive**, and only once all of them miss is a
/// case-insensitive pass made over slug and name together. So an exact match
/// always beats a case-differing one, whatever tier the latter is in: with an
/// environment slugged `qa` beside one *named* `QA`, the query `QA` returns the
/// one actually named that. What the case-insensitive pass must never do is
/// guess — two environments differing only in case are an error, not a coin
/// flip.
///
/// Substrings are deliberately *not* accepted. `dev` would otherwise match
/// `dev` and `development`, and the losing case is a deleted environment.
pub fn environment<'a>(project: &'a Project, query: &str) -> Result<&'a Environment> {
    let describe = |e: &Environment| Candidate {
        name: e.name.clone(),
        id: e.id.clone(),
        slug: Some(e.slug.clone()),
    };

    // Slug outranks name deliberately: the slug is what appears in every secret
    // path, so it is what a caller typing `dev` almost always means. Without
    // the tiers, an environment named `qa` sitting next to one *slugged* `qa`
    // resolved by list order.
    for field in [
        (|e: &Environment, q: &str| e.id == q) as fn(&Environment, &str) -> bool,
        |e, q| e.slug == q,
        |e, q| e.name == q,
    ] {
        let tier: Vec<&Environment> = project
            .environments
            .iter()
            .filter(|e| field(e, query))
            .collect();
        match one_of(tier, query, describe) {
            Resolution::One(found) => return Ok(found),
            Resolution::Many(err) => return Err(err),
            Resolution::None => {}
        }
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
    fn an_exact_name_beats_a_case_differing_slug() {
        // The tiers are case-sensitive, so a query that exactly names one
        // environment wins over one that only case-insensitively matches
        // another's slug. Pinned because the reverse is a defensible reading of
        // "id, then slug, then name" and the choice should not drift silently.
        let project = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "Staging", "slug": "qa"},
            {"id": "e2", "name": "QA", "slug": "eu"}
        ]));
        assert_eq!(environment(&project, "QA").unwrap().id, "e2");
        // Lowercase `qa` is e1's exact slug, so the slug tier wins there.
        assert_eq!(environment(&project, "qa").unwrap().id, "e1");
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
    fn environment_slug_outranks_another_environments_name() {
        // e1's *slug* is "qa"; e2's *name* is "qa". The slug is what appears in
        // every secret path, so it wins — by the precedence rule, not by list
        // order. This test used to pass only because of the fixture ordering:
        // swapping the two entries flipped the answer.
        let forward = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "QA", "slug": "qa"},
            {"id": "e2", "name": "qa", "slug": "qa-eu"}
        ]));
        let reversed = project_with_envs(serde_json::json!([
            {"id": "e2", "name": "qa", "slug": "qa-eu"},
            {"id": "e1", "name": "QA", "slug": "qa"}
        ]));

        for project in [&forward, &reversed] {
            assert_eq!(environment(project, "qa").unwrap().id, "e1");
            assert_eq!(environment(project, "qa-eu").unwrap().id, "e2");
        }
    }

    #[test]
    fn duplicate_exact_names_are_ambiguous_not_first_match() {
        // Infisical enforces uniqueness on ids and slugs but not on names, and
        // a `create` whose cleanup also failed leaves two identities sharing
        // one. Guessing here means deleting the wrong one.
        let projects = vec![
            project_fixture("p1", "Billing", "billing-a"),
            project_fixture("p2", "Billing", "billing-b"),
        ];
        assert!(matches!(
            project(&projects, "Billing"),
            Err(IfpError::AmbiguousMatch { .. })
        ));

        let identities = vec![identity_fixture("i1", "ci"), identity_fixture("i2", "ci")];
        let err = identity(&identities, "ci").unwrap_err();
        assert!(matches!(err, IfpError::AmbiguousMatch { .. }));
        let json = err.to_json();
        assert_eq!(json["candidates"][0]["id"], "i1");
        assert_eq!(json["candidates"][1]["id"], "i2");

        let envs = project_with_envs(serde_json::json!([
            {"id": "e1", "name": "QA", "slug": "qa-a"},
            {"id": "e2", "name": "QA", "slug": "qa-b"}
        ]));
        assert!(matches!(
            environment(&envs, "QA"),
            Err(IfpError::AmbiguousMatch { .. })
        ));
    }

    #[test]
    fn an_id_outranks_another_records_slug_or_name() {
        // An id is the least ambiguous thing a caller can pass, so it must win
        // outright even when the same string is someone else's slug.
        let projects = vec![
            project_fixture("shared", "Alpha", "alpha"),
            project_fixture("p2", "Beta", "shared"),
        ];
        assert_eq!(project(&projects, "shared").unwrap().name, "Alpha");
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
