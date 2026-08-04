use serde::{Deserialize, Serialize};

/// Every struct here decodes a subset of the API response. Infisical adds
/// fields freely between releases, so unknown keys are ignored and everything
/// beyond the identifying fields is optional — a new field upstream must never
/// turn into a parse failure here.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "type", default)]
    pub project_type: Option<String>,
    #[serde(rename = "hasDeleteProtection", default)]
    pub has_delete_protection: bool,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub environments: Vec<Environment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub position: Option<i64>,
}

/// An identity's membership in an organization. The API nests the identity
/// proper under `identity`, while the role and the id used by every other
/// route live on the outer object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMembership {
    #[serde(rename = "identityId", default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    #[serde(rename = "lastLoginTime", default)]
    pub last_login_time: Option<String>,
    /// Present on project memberships. Organization memberships use the
    /// scalar `role` above instead.
    #[serde(default)]
    pub roles: Vec<MembershipRole>,
    pub identity: Identity,
}

/// A project membership role. The API always sends both keys and nulls the
/// unused one, so a presence check picks the null; only a non-null string
/// counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipRole {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(rename = "customRoleSlug", default)]
    pub custom_role_slug: Option<String>,
}

impl MembershipRole {
    pub fn label(&self) -> Option<&str> {
        self.custom_role_slug
            .as_deref()
            .or(self.role.as_deref())
            .filter(|s| !s.is_empty())
    }
}

impl IdentityMembership {
    /// `identityId` is the field every identity route takes. It is absent from
    /// some responses, where the nested object's own id is the same value.
    pub fn id(&self) -> &str {
        self.identity_id.as_deref().unwrap_or(&self.identity.id)
    }

    pub fn name(&self) -> &str {
        &self.identity.name
    }

    /// Organization memberships carry one scalar role; project memberships
    /// carry a list. Render whichever the response actually used.
    pub fn role(&self) -> String {
        if let Some(role) = self.role.as_deref() {
            return role.to_string();
        }
        let labels: Vec<&str> = self
            .roles
            .iter()
            .filter_map(MembershipRole::label)
            .collect();
        if labels.is_empty() {
            "-".to_string()
        } else {
            labels.join(",")
        }
    }

    /// Auth methods are what make an identity usable; one with none cannot log
    /// in at all, which is worth surfacing in listings.
    pub fn auth_methods(&self) -> String {
        if self.identity.auth_methods.is_empty() {
            "none".to_string()
        } else {
            self.identity.auth_methods.join(",")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
    #[serde(rename = "authMethods", default)]
    pub auth_methods: Vec<String>,
    #[serde(rename = "hasDeleteProtection", default)]
    pub has_delete_protection: bool,
}

/// Returned once, at creation. The secret itself is never retrievable again,
/// which is why the create path must print it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSecret {
    #[serde(rename = "clientSecret")]
    pub client_secret: String,
    #[serde(rename = "clientSecretData", default)]
    pub data: Option<ClientSecretData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSecretData {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "clientSecretPrefix", default)]
    pub prefix: Option<String>,
    #[serde(rename = "clientSecretTTL", default)]
    pub ttl: Option<i64>,
    #[serde(rename = "clientSecretNumUsesLimit", default)]
    pub num_uses_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalAuthConfig {
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "accessTokenTTL", default)]
    pub access_token_ttl: Option<i64>,
    #[serde(rename = "accessTokenMaxTTL", default)]
    pub access_token_max_ttl: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_decodes_and_ignores_unknown_fields() {
        let json = serde_json::json!({
            "id": "p1",
            "name": "billing-api",
            "slug": "billing-api",
            "description": null,
            "type": "secret-manager",
            "hasDeleteProtection": true,
            "createdAt": "2026-08-01T00:00:00Z",
            "environments": [{"id": "e1", "name": "Development", "slug": "dev", "position": 1}],
            "someFieldAddedNextRelease": 42
        });
        let project: Project = serde_json::from_value(json).unwrap();
        assert_eq!(project.name, "billing-api");
        assert!(project.has_delete_protection);
        assert_eq!(project.environments.len(), 1);
        assert_eq!(project.environments[0].slug, "dev");
    }

    #[test]
    fn project_decodes_without_optional_fields() {
        let json = serde_json::json!({"id": "p1", "name": "n", "slug": "s"});
        let project: Project = serde_json::from_value(json).unwrap();
        assert!(project.environments.is_empty());
        assert!(!project.has_delete_protection);
        assert!(project.description.is_none());
    }

    #[test]
    fn membership_prefers_outer_identity_id() {
        let json = serde_json::json!({
            "identityId": "outer-id",
            "role": "admin",
            "identity": {"id": "inner-id", "name": "ci", "authMethods": ["universal-auth"]}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.id(), "outer-id");
        assert_eq!(m.name(), "ci");
        assert_eq!(m.role(), "admin");
        assert_eq!(m.auth_methods(), "universal-auth");
    }

    #[test]
    fn membership_falls_back_to_nested_id() {
        let json = serde_json::json!({
            "identity": {"id": "inner-id", "name": "ci", "authMethods": []}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.id(), "inner-id");
        assert_eq!(m.role(), "-");
        // An identity with no auth method cannot log in; say so rather than
        // rendering an empty cell.
        assert_eq!(m.auth_methods(), "none");
    }

    #[test]
    fn project_membership_reads_role_from_the_roles_array() {
        // Project memberships carry `roles[]` and no scalar `role`. The API
        // sends both keys inside each entry and nulls the unused one, so a
        // naive presence check picks the null and reports no role at all.
        let json = serde_json::json!({
            "identityId": "i1",
            "roles": [{"customRoleSlug": null, "role": "viewer", "isTemporary": false}],
            "identity": {"id": "i1", "name": "ci", "authMethods": []}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.role(), "viewer");
    }

    #[test]
    fn custom_role_slug_wins_over_builtin() {
        let role = MembershipRole {
            role: None,
            custom_role_slug: Some("deployer".into()),
        };
        assert_eq!(role.label(), Some("deployer"));
    }

    #[test]
    fn role_is_dash_when_neither_form_is_present() {
        let json = serde_json::json!({
            "identity": {"id": "i1", "name": "ci", "authMethods": []}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.role(), "-");
    }

    #[test]
    fn scalar_org_role_still_takes_precedence() {
        // Organization memberships use the scalar form; it must not be shadowed
        // by an empty roles array.
        let json = serde_json::json!({
            "identityId": "i1",
            "role": "admin",
            "roles": [],
            "identity": {"id": "i1", "name": "ci", "authMethods": []}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.role(), "admin");
    }

    #[test]
    fn multiple_project_roles_are_joined() {
        let json = serde_json::json!({
            "roles": [
                {"role": "viewer", "customRoleSlug": null},
                {"role": null, "customRoleSlug": "auditor"}
            ],
            "identity": {"id": "i1", "name": "ci", "authMethods": []}
        });
        let m: IdentityMembership = serde_json::from_value(json).unwrap();
        assert_eq!(m.role(), "viewer,auditor");
    }

    #[test]
    fn client_secret_decodes() {
        let json = serde_json::json!({
            "clientSecret": "sk-abc",
            "clientSecretData": {
                "id": "cs1", "description": "ci", "clientSecretPrefix": "sk-a",
                "clientSecretTTL": 0, "clientSecretNumUsesLimit": 0
            }
        });
        let cs: ClientSecret = serde_json::from_value(json).unwrap();
        assert_eq!(cs.client_secret, "sk-abc");
        assert_eq!(cs.data.unwrap().description, "ci");
    }
}
