use crate::config::Config;
use crate::error::{IfpError, Result};
use crate::models::{ClientSecret, IdentityMembership, Project, UniversalAuthConfig};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

pub struct InfisicalClient {
    http: reqwest::Client,
    api_url: String,
    token: String,
    org_id: String,
}

impl InfisicalClient {
    pub fn new(config: Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_url: config.api_url,
            token: config.token,
            org_id: config.org_id,
        }
    }

    /// Build a client from explicit parts, bypassing the `infisical` CLI.
    /// Used by tests to point at a mock server.
    #[cfg(test)]
    pub fn with_parts(api_url: &str, token: &str, org_id: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_url: api_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            org_id: org_id.to_string(),
        }
    }

    async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<String> {
        let url = format!("{}{}", self.api_url, path);
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if status.is_success() {
            return Ok(text);
        }

        // Infisical answers 403 — not 401 — when a route rejects the caller's
        // auth mode, so a permission problem and a wrong-token-type problem
        // look identical without this hint.
        Err(match status {
            StatusCode::FORBIDDEN => IfpError::Forbidden(format!(
                "{}\nThis route may require an organization role with the necessary permission, \
                 or a user login rather than a machine identity.",
                Self::extract_message(&text)
            )),
            StatusCode::UNAUTHORIZED => IfpError::NotLoggedIn,
            _ => IfpError::Api(format!("{} {}", status, Self::extract_message(&text))),
        })
    }

    /// API errors arrive as JSON with a `message`. Show that rather than the
    /// whole envelope, but fall back to the raw body when the shape is unknown.
    fn extract_message(body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str().map(str::to_string))
                    .or_else(|| v.get("error").and_then(|e| e.as_str().map(str::to_string)))
            })
            .unwrap_or_else(|| body.trim().to_string())
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let text = self.request(Method::GET, path, None).await?;
        Self::parse(&text)
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &Value,
    ) -> Result<T> {
        let text = self.request(method, path, Some(body)).await?;
        Self::parse(&text)
    }

    fn parse<T: DeserializeOwned>(text: &str) -> Result<T> {
        serde_json::from_str(text)
            .map_err(|e| IfpError::Api(format!("unexpected response shape: {e}")))
    }

    // -- Projects --

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        #[derive(serde::Deserialize)]
        struct Response {
            projects: Vec<Project>,
        }
        let response: Response = self.get_json("/v1/projects").await?;
        Ok(response.projects)
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Project> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(alias = "workspace")]
            project: Project,
        }
        let response: Response = self.get_json(&format!("/v1/projects/{project_id}")).await?;
        Ok(response.project)
    }

    pub async fn create_project(
        &self,
        name: &str,
        slug: Option<&str>,
        description: Option<&str>,
        project_type: &str,
    ) -> Result<Project> {
        let mut body = json!({
            "projectName": name,
            "type": project_type,
        });
        if let Some(slug) = slug {
            body["slug"] = json!(slug);
        }
        if let Some(description) = description {
            body["projectDescription"] = json!(description);
        }

        #[derive(serde::Deserialize)]
        struct Response {
            project: Project,
        }
        let response: Response = self.send_json(Method::POST, "/v1/projects", &body).await?;
        Ok(response.project)
    }

    /// The update route names the field `name`, while create names it
    /// `projectName`. Keeping both spellings straight is the caller's whole
    /// reason for going through this method.
    pub async fn update_project(
        &self,
        project_id: &str,
        name: Option<&str>,
        slug: Option<&str>,
        description: Option<&str>,
        has_delete_protection: Option<bool>,
    ) -> Result<Project> {
        let mut body = json!({});
        if let Some(name) = name {
            body["name"] = json!(name);
        }
        if let Some(slug) = slug {
            body["slug"] = json!(slug);
        }
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if let Some(protection) = has_delete_protection {
            body["hasDeleteProtection"] = json!(protection);
        }

        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(alias = "workspace")]
            project: Project,
        }
        let response: Response = self
            .send_json(Method::PATCH, &format!("/v1/projects/{project_id}"), &body)
            .await?;
        Ok(response.project)
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<()> {
        self.request(Method::DELETE, &format!("/v1/projects/{project_id}"), None)
            .await?;
        Ok(())
    }

    // -- Environments --

    pub async fn create_environment(
        &self,
        project_id: &str,
        name: &str,
        slug: &str,
    ) -> Result<Value> {
        let body = json!({ "name": name, "slug": slug });
        self.send_json(
            Method::POST,
            &format!("/v1/projects/{project_id}/environments"),
            &body,
        )
        .await
    }

    pub async fn update_environment(
        &self,
        project_id: &str,
        env_id: &str,
        name: Option<&str>,
        slug: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({});
        if let Some(name) = name {
            body["name"] = json!(name);
        }
        if let Some(slug) = slug {
            body["slug"] = json!(slug);
        }
        self.send_json(
            Method::PATCH,
            &format!("/v1/projects/{project_id}/environments/{env_id}"),
            &body,
        )
        .await
    }

    pub async fn delete_environment(&self, project_id: &str, env_id: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/v1/projects/{project_id}/environments/{env_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    // -- Machine identities --

    pub async fn list_identities(&self) -> Result<Vec<IdentityMembership>> {
        #[derive(serde::Deserialize)]
        struct Response {
            identities: Vec<IdentityMembership>,
        }
        let response: Response = self
            .get_json(&format!("/v1/identities?orgId={}", self.org_id))
            .await?;
        Ok(response.identities)
    }

    /// Create an identity scoped to a single project. Unlike an org identity
    /// it is confined to that project from the moment it exists, so a leaked
    /// credential cannot be pointed at anything else.
    pub async fn create_project_identity(
        &self,
        project_id: &str,
        name: &str,
        role: &str,
    ) -> Result<Value> {
        let body = json!({
            "name": name,
            "roles": [{ "role": role, "isTemporary": false }],
        });
        self.send_json(
            Method::POST,
            &format!("/v1/projects/{project_id}/identities"),
            &body,
        )
        .await
    }

    /// Rename only. The `role` field on this route sets the *organization*
    /// role, which `inf` never grants, so it is deliberately not exposed.
    pub async fn update_identity(&self, identity_id: &str, name: &str) -> Result<Value> {
        let body = json!({ "name": name });
        self.send_json(
            Method::PATCH,
            &format!("/v1/identities/{identity_id}"),
            &body,
        )
        .await
    }

    /// Delete a project-scoped identity. The organization route refuses these
    /// with "Identity is managed by project", so the scope must be honoured.
    pub async fn delete_project_identity(&self, project_id: &str, identity_id: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/v1/projects/{project_id}/identities/{identity_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn delete_identity(&self, identity_id: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/v1/identities/{identity_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    // -- Universal Auth --

    pub async fn attach_universal_auth(&self, identity_id: &str) -> Result<UniversalAuthConfig> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "identityUniversalAuth")]
            config: UniversalAuthConfig,
        }
        let response: Response = self
            .send_json(
                Method::POST,
                &format!("/v1/auth/universal-auth/identities/{identity_id}"),
                &json!({}),
            )
            .await?;
        Ok(response.config)
    }

    pub async fn get_universal_auth(&self, identity_id: &str) -> Result<UniversalAuthConfig> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "identityUniversalAuth")]
            config: UniversalAuthConfig,
        }
        let response: Response = self
            .get_json(&format!("/v1/auth/universal-auth/identities/{identity_id}"))
            .await?;
        Ok(response.config)
    }

    pub async fn create_client_secret(
        &self,
        identity_id: &str,
        description: &str,
        ttl: i64,
        num_uses_limit: i64,
    ) -> Result<ClientSecret> {
        let body = json!({
            "description": description,
            "ttl": ttl,
            "numUsesLimit": num_uses_limit,
        });
        self.send_json(
            Method::POST,
            &format!("/v1/auth/universal-auth/identities/{identity_id}/client-secrets"),
            &body,
        )
        .await
    }

    // -- Project membership for identities --

    pub async fn list_project_identities(
        &self,
        project_id: &str,
    ) -> Result<Vec<IdentityMembership>> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "identityMemberships")]
            memberships: Vec<IdentityMembership>,
        }
        let response: Response = self
            .get_json(&format!("/v1/projects/{project_id}/memberships/identities"))
            .await?;
        Ok(response.memberships)
    }

    /// The create route accepts a `roles` array but does not apply it — a new
    /// project identity always lands on `no-access`. Setting the role is a
    /// separate membership update, which this performs.
    pub async fn set_project_identity_role(
        &self,
        project_id: &str,
        identity_id: &str,
        role: &str,
    ) -> Result<Value> {
        self.send_json(
            Method::PATCH,
            &format!("/v1/projects/{project_id}/memberships/identities/{identity_id}"),
            &json!({ "roles": [{ "role": role, "isTemporary": false }] }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_message_prefers_message_field() {
        let body = r#"{"statusCode":403,"message":"Insufficient permission","error":"Forbidden"}"#;
        assert_eq!(
            InfisicalClient::extract_message(body),
            "Insufficient permission"
        );
    }

    #[test]
    fn extract_message_falls_back_to_error_field() {
        assert_eq!(
            InfisicalClient::extract_message(r#"{"error":"Forbidden"}"#),
            "Forbidden"
        );
    }

    #[test]
    fn extract_message_falls_back_to_raw_body() {
        assert_eq!(
            InfisicalClient::extract_message("upstream timeout\n"),
            "upstream timeout"
        );
    }

    #[tokio::test]
    async fn list_projects_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/projects")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"projects":[{"id":"p1","name":"billing","slug":"billing",
                   "environments":[{"id":"e1","name":"Dev","slug":"dev"}]}]}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "test-token", "org-1");
        let projects = client.list_projects().await.unwrap();

        mock.assert_async().await;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "billing");
        assert_eq!(projects[0].environments[0].slug, "dev");
    }

    #[tokio::test]
    async fn list_identities_scopes_by_org_from_token() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/identities?orgId=org-42")
            .with_status(200)
            .with_body(
                r#"{"identities":[{"identityId":"i1","role":"admin",
                   "identity":{"id":"i1","name":"ci","authMethods":["universal-auth"]}}]}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-42");
        let identities = client.list_identities().await.unwrap();

        mock.assert_async().await;
        assert_eq!(identities[0].name(), "ci");
        assert_eq!(identities[0].id(), "i1");
    }

    #[tokio::test]
    async fn create_project_sends_projectname_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/projects")
            .match_body(mockito::Matcher::PartialJson(
                json!({"projectName": "new-svc", "type": "secret-manager"}),
            ))
            .with_status(200)
            .with_body(r#"{"project":{"id":"p9","name":"new-svc","slug":"new-svc"}}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let project = client
            .create_project("new-svc", None, None, "secret-manager")
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(project.id, "p9");
    }

    #[tokio::test]
    async fn update_project_sends_name_not_projectname() {
        let mut server = mockito::Server::new_async().await;
        // The rename route uses `name`; sending `projectName` here silently
        // does nothing, so assert the exact key.
        let mock = server
            .mock("PATCH", "/v1/projects/p1")
            .match_body(mockito::Matcher::Json(json!({"name": "renamed"})))
            .with_status(200)
            .with_body(r#"{"project":{"id":"p1","name":"renamed","slug":"billing"}}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let project = client
            .update_project("p1", Some("renamed"), None, None, None)
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(project.name, "renamed");
    }

    #[tokio::test]
    async fn forbidden_is_reported_with_a_hint() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/v1/projects/p1/identities")
            .with_status(403)
            .with_body(r#"{"message":"Insufficient permission"}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let err = client
            .create_project_identity("p1", "ci", "viewer")
            .await
            .unwrap_err();

        assert_eq!(err.exit_code(), 1);
        let msg = err.to_string();
        assert!(msg.contains("Insufficient permission"));
        assert!(msg.contains("user login"));
    }

    #[tokio::test]
    async fn unauthorized_maps_to_not_logged_in() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/projects")
            .with_status(401)
            .with_body(r#"{"message":"token expired"}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let err = client.list_projects().await.unwrap_err();
        assert!(matches!(err, IfpError::NotLoggedIn));
    }

    #[tokio::test]
    async fn delete_project_tolerates_empty_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("DELETE", "/v1/projects/p1")
            .with_status(200)
            .with_body("")
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        client.delete_project("p1").await.unwrap();
        mock.assert_async().await;
    }
}
