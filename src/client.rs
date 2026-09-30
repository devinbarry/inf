use crate::config::Config;
use crate::error::{IfpError, Result};
use crate::models::{
    ClientSecret, IdentityMembership, Project, ProjectRole, SshCa, SshCertificateTemplate,
    UniversalAuthConfig,
};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// The project-membership listing paginates and defaults to 20 rows — small
/// enough that an organization of any size silently loses identities from the
/// tail, which then resolve as "no match". 1000 is the server's maximum page
/// size.
///
/// The org-level listing (`GET /v1/identities`) does *not* paginate: its
/// querystring schema accepts `orgId` alone, so zod strips `limit`/`offset` and
/// the route returns everything in one response. Both routes do return
/// `totalCount`, so that — not a short page — is what terminates the walk. A
/// short-page rule looked right but re-fetched the identical full response
/// forever on any org with at least `PAGE_LIMIT` identities.
const PAGE_LIMIT: usize = 1000;

/// Backstop against a server that ignores `limit` and returns a full page
/// forever. At 1000 rows a page this is far beyond any real organization.
const MAX_PAGES: usize = 100;

pub struct InfisicalClient {
    http: reqwest::Client,
    api_url: String,
    token: String,
    org_id: String,
}

impl InfisicalClient {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            http: Self::build_http()?,
            api_url: config.api_url,
            token: config.token,
            org_id: config.org_id,
        })
    }

    /// A half-open socket is the one failure that produces *nothing* — no
    /// output, no exit code — and on Linux it can sit there for roughly two
    /// hours before TCP keepalive notices. That is the same hole the
    /// `infisical` subprocess timeout closes, on the path that is actually hot,
    /// so it gets the same treatment.
    fn build_http() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(Self::request_timeout())
            .build()
            .map_err(|e| IfpError::Config(format!("cannot build HTTP client: {e}")))
    }

    fn request_timeout() -> std::time::Duration {
        let secs = std::env::var("INF_HTTP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(30);
        std::time::Duration::from_secs(secs)
    }

    /// Build a client from explicit parts, bypassing the `infisical` CLI.
    /// Used by tests to point at a mock server.
    #[cfg(test)]
    pub fn with_parts(api_url: &str, token: &str, org_id: &str) -> Self {
        Self {
            http: Self::build_http().expect("test HTTP client"),
            api_url: api_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            org_id: org_id.to_string(),
        }
    }

    async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<String> {
        let url = format!("{}{}", self.api_url, path);
        let method_label = method.to_string();
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req
            .send()
            .await
            .map_err(|e| Self::transport_error(e, &method_label, &url))?;
        let status = response.status();

        // The body is a second place the transfer can stall. Discarding this
        // error turned a timeout mid-body into an empty string, which then
        // surfaced as "unexpected response shape: EOF" — an `api_error` for
        // what was really `http_timeout`, and the wrong advice to the caller.
        let text = match response.text().await {
            Ok(text) => text,
            Err(e) => {
                // An unreadable body on an error status still leaves the status
                // itself, which is the more useful diagnosis.
                if !status.is_success() {
                    return Err(Self::status_error(status, ""));
                }
                return Err(Self::transport_error(e, &method_label, &url));
            }
        };

        if status.is_success() {
            return Ok(text);
        }

        Err(Self::status_error(status, &text))
    }

    fn transport_error(e: reqwest::Error, method: &str, url: &str) -> IfpError {
        if e.is_timeout() {
            IfpError::HttpTimeout(format!(
                "{method} {url} did not complete within {}s. \
                 Retry, or raise INF_HTTP_TIMEOUT_SECS",
                Self::request_timeout().as_secs()
            ))
        } else {
            IfpError::Http(e)
        }
    }

    fn status_error(status: StatusCode, _text: &str) -> IfpError {
        // Error bodies are untrusted credential-bearing data, including otherwise useful message fields.
        // Keep the stable error kind and HTTP status without returning the server's arbitrary strings.
        match status {
            StatusCode::FORBIDDEN => IfpError::Forbidden(
                "Insufficient permission. This route may require an organization role with the necessary permission, or a user login rather than a machine identity.".into()),
            StatusCode::UNAUTHORIZED => IfpError::NotLoggedIn,
            StatusCode::NOT_FOUND => IfpError::NotFound("Resource not found".into()),
            _ => IfpError::Api(format!("{status}; response details withheld")),
        }
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

    /// For routes whose body we pass straight through. A mutation that answers
    /// `204 No Content` is a success, not a parse failure — treating an empty
    /// body as an error made successful PATCHes report
    /// "unexpected response shape: EOF while parsing a value".
    async fn send_value(&self, method: Method, path: &str, body: &Value) -> Result<Value> {
        let text = self.request(method, path, Some(body)).await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
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
        create_default_envs: bool,
        has_delete_protection: bool,
    ) -> Result<Project> {
        let mut body = json!({
            "projectName": name,
            "type": project_type,
            "shouldCreateDefaultEnvs": create_default_envs,
            "hasDeleteProtection": has_delete_protection,
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

    // -- SSH certificate authorities --

    pub async fn list_ssh_cas(&self, project_id: &str) -> Result<Vec<SshCa>> {
        #[derive(serde::Deserialize)]
        struct Response {
            cas: Vec<SshCa>,
        }
        let response: Response = self
            .get_json(&format!("/v2/workspace/{project_id}/ssh-cas"))
            .await?;
        Ok(response.cas)
    }

    pub async fn update_ssh_ca(
        &self,
        ca_id: &str,
        friendly_name: &str,
        status: &str,
    ) -> Result<SshCa> {
        #[derive(serde::Deserialize)]
        struct Response {
            ca: SshCa,
        }
        let response: Response = self
            .send_json(
                Method::PATCH,
                &format!("/v1/ssh/ca/{ca_id}"),
                &json!({"friendlyName": friendly_name, "status": status}),
            )
            .await?;
        Ok(response.ca)
    }

    pub async fn get_ssh_ca_public_key(&self, ca_id: &str) -> Result<String> {
        let key = self
            .request(Method::GET, &format!("/v1/ssh/ca/{ca_id}/public-key"), None)
            .await?;
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(IfpError::Api("SSH CA public-key response was empty".into()));
        }
        Ok(key)
    }

    pub async fn list_ssh_certificate_templates(
        &self,
        project_id: &str,
    ) -> Result<Vec<SshCertificateTemplate>> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "certificateTemplates")]
            certificate_templates: Vec<SshCertificateTemplate>,
        }
        let response: Response = self
            .get_json(&format!(
                "/v2/workspace/{project_id}/ssh-certificate-templates"
            ))
            .await?;
        Ok(response.certificate_templates)
    }

    pub async fn create_ssh_user_certificate_template(
        &self,
        ca_id: &str,
        name: &str,
        principals: &[String],
        ttl: &str,
        max_ttl: &str,
    ) -> Result<SshCertificateTemplate> {
        self.send_json(
            Method::POST,
            "/v1/ssh/certificate-templates",
            &json!({
                "sshCaId": ca_id,
                "name": name,
                "allowedUsers": principals,
                "allowedHosts": [],
                "allowUserCertificates": true,
                "allowHostCertificates": false,
                "allowCustomKeyIds": false,
                "ttl": ttl,
                "maxTTL": max_ttl,
            }),
        )
        .await
    }

    pub async fn list_ssh_issuer_roles(&self, project_id: &str) -> Result<Vec<ProjectRole>> {
        #[derive(serde::Deserialize)]
        struct Response {
            projects: Vec<Project>,
        }
        let response: Response = self
            .get_json("/v1/projects?includeRoles=true&type=ssh")
            .await?;
        response
            .projects
            .into_iter()
            .find(|project| project.id == project_id)
            .map(|project| project.roles)
            .ok_or_else(|| IfpError::NotFound(format!("SSH project {project_id}")))
    }

    /// Create the sole role used by an SSH certificate issuer. The permission
    /// is intentionally fixed so this command cannot accidentally grant
    /// secret, project, or administrative access.
    pub async fn create_ssh_issuer_role(
        &self,
        project_id: &str,
        name: &str,
        slug: &str,
    ) -> Result<ProjectRole> {
        #[derive(serde::Deserialize)]
        struct Response {
            role: ProjectRole,
        }
        let response: Response = self
            .send_json(
                Method::POST,
                &format!("/v1/projects/{project_id}/roles"),
                &json!({
                    "name": name,
                    "slug": slug,
                    "description": "May only request SSH certificates in this isolated project",
                    "permissions": [{"subject": "ssh-certificates", "action": "create"}],
                }),
            )
            .await?;
        Ok(response.role)
    }

    // -- Environments --

    pub async fn create_environment(
        &self,
        project_id: &str,
        name: &str,
        slug: &str,
    ) -> Result<Value> {
        let body = json!({ "name": name, "slug": slug });
        self.send_value(
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
        self.send_value(
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

    /// The org listing returns every identity in one response — see the note on
    /// [`PAGE_LIMIT`]. The walk is kept anyway so this stays correct if the
    /// route gains pagination, but it terminates on `totalCount`.
    pub async fn list_identities(&self) -> Result<Vec<IdentityMembership>> {
        #[derive(serde::Deserialize)]
        struct Response {
            identities: Vec<IdentityMembership>,
            #[serde(rename = "totalCount", default)]
            total_count: Option<usize>,
        }
        let mut all = Vec::new();
        for _ in 0..MAX_PAGES {
            let page: Response = self
                .get_json(&format!(
                    "/v1/identities?orgId={}&limit={PAGE_LIMIT}&offset={}",
                    self.org_id,
                    all.len()
                ))
                .await?;
            let count = page.identities.len();
            all.extend(page.identities);
            if Self::walk_is_done(count, all.len(), page.total_count) {
                break;
            }
        }
        Ok(all)
    }

    /// Stop when the server says we have everything, when a page came back
    /// short, or when a page was empty. `totalCount` is authoritative and is
    /// the only rule that is correct for a non-paginating route, where every
    /// request returns the same full set.
    fn walk_is_done(page_count: usize, collected: usize, total: Option<usize>) -> bool {
        if page_count == 0 {
            return true;
        }
        match total {
            Some(total) => collected >= total,
            None => page_count < PAGE_LIMIT,
        }
    }

    /// Create an identity scoped to a single project. Unlike an org identity
    /// it is confined to that project from the moment it exists, so a leaked
    /// credential cannot be pointed at anything else.
    ///
    /// `roles` is honoured from Infisical v0.162.0 onward. Older servers accept
    /// the field, strip it, and land the identity on `no-access` — which is why
    /// the caller still applies the role explicitly afterwards. Sending it here
    /// costs nothing and gets the role right in one call on current servers.
    pub async fn create_project_identity(
        &self,
        project_id: &str,
        name: &str,
        role: &str,
    ) -> Result<Value> {
        let body = json!({
            "name": name,
            "roles": [{ "role": role, "isTemporary": false }],
            "hasDeleteProtection": true,
        });
        self.send_json(
            Method::POST,
            &format!("/v1/projects/{project_id}/identities"),
            &body,
        )
        .await
    }

    /// Rename an organization-level identity.
    ///
    /// This route refuses project-scoped identities outright — `updateIdentity`
    /// in the backend rejects any identity with a `projectId` with
    /// "Identity is managed by project", exactly as the delete route does. Use
    /// [`update_project_identity`] for those.
    ///
    /// The `role` field on this route sets the *organization* role, which `inf`
    /// never grants, so it is deliberately not exposed.
    pub async fn update_identity(&self, identity_id: &str, name: &str) -> Result<Value> {
        let body = json!({ "name": name });
        self.send_value(
            Method::PATCH,
            &format!("/v1/identities/{identity_id}"),
            &body,
        )
        .await
    }

    /// Rename a project-scoped identity. The organization route rejects these,
    /// so scope decides which of the two is correct.
    pub async fn update_project_identity(
        &self,
        project_id: &str,
        identity_id: &str,
        name: &str,
    ) -> Result<Value> {
        let body = json!({ "name": name });
        self.send_value(
            Method::PATCH,
            &format!("/v1/projects/{project_id}/identities/{identity_id}"),
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

    pub async fn attach_bounded_ssh_universal_auth(
        &self,
        identity_id: &str,
    ) -> Result<UniversalAuthConfig> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "identityUniversalAuth")]
            config: UniversalAuthConfig,
        }
        let response: Response = self
            .send_json(
                Method::POST,
                &format!("/v1/auth/universal-auth/identities/{identity_id}"),
                &json!({
                    "clientSecretTrustedIps": [
                        {"ipAddress": "0.0.0.0/0"},
                        {"ipAddress": "::/0"}
                    ],
                    "accessTokenTrustedIps": [
                        {"ipAddress": "0.0.0.0/0"},
                        {"ipAddress": "::/0"}
                    ],
                    "accessTokenTTL": 900,
                    "accessTokenMaxTTL": 900,
                    "accessTokenNumUsesLimit": 0,
                    "accessTokenPeriod": 0,
                    "lockoutEnabled": true,
                    "lockoutThreshold": 3,
                    "lockoutDurationSeconds": 300,
                    "lockoutCounterResetSeconds": 30
                }),
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

    /// This route genuinely paginates: `offset` and `limit` are in its
    /// querystring schema and `limit` defaults to 20, so an unpaginated call
    /// drops the 21st identity onward — and a dropped identity resolves as
    /// "no match", not as an error.
    pub async fn list_project_identities(
        &self,
        project_id: &str,
    ) -> Result<Vec<IdentityMembership>> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(rename = "identityMemberships")]
            memberships: Vec<IdentityMembership>,
            #[serde(rename = "totalCount", default)]
            total_count: Option<usize>,
        }
        let mut all = Vec::new();
        for _ in 0..MAX_PAGES {
            let page: Response = self
                .get_json(&format!(
                    "/v1/projects/{project_id}/memberships/identities?limit={PAGE_LIMIT}&offset={}",
                    all.len()
                ))
                .await?;
            let count = page.memberships.len();
            all.extend(page.memberships);
            if Self::walk_is_done(count, all.len(), page.total_count) {
                break;
            }
        }
        Ok(all)
    }

    /// Applying the role is a separate membership update. On servers older than
    /// v0.162.0 the `roles` array on the create route is stripped and the
    /// identity lands on `no-access`, so this call is what actually grants the
    /// requested role there; on newer servers it is a harmless no-op re-assert.
    pub async fn set_project_identity_role(
        &self,
        project_id: &str,
        identity_id: &str,
        role: &str,
    ) -> Result<Value> {
        self.send_value(
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
    fn error_bodies_never_disclose_credentials() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            for body in [
                r#"{"message":"synthetic-private-credential"}"#,
                "synthetic-private-credential",
            ] {
                let error = InfisicalClient::status_error(status, body);
                assert!(!error
                    .to_json()
                    .to_string()
                    .contains("synthetic-private-credential"));
            }
        }
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
    async fn ssh_ca_public_key_accepts_the_plain_text_api_response() {
        let mut server = mockito::Server::new_async().await;
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest fleet-ca";
        let mock = server
            .mock("GET", "/v1/ssh/ca/ca-1/public-key")
            .with_status(200)
            .with_body(format!("{key}\n"))
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        assert_eq!(client.get_ssh_ca_public_key("ca-1").await.unwrap(), key);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn ssh_issuer_role_has_exactly_one_fixed_permission() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/projects/p1/roles")
            .match_body(mockito::Matcher::Json(json!({
                "name": "Issuer",
                "slug": "issuer",
                "description": "May only request SSH certificates in this isolated project",
                "permissions": [{"subject": "ssh-certificates", "action": "create"}],
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"role":{"id":"r1","name":"Issuer","slug":"issuer",
                    "permissions":[{"action":["create"],"subject":"ssh-certificates"}]}}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let role = client
            .create_ssh_issuer_role("p1", "Issuer", "issuer")
            .await
            .unwrap();
        assert_eq!(role.slug, "issuer");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn bounded_ssh_auth_sends_the_recovery_profile() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/auth/universal-auth/identities/i1")
            .match_body(mockito::Matcher::PartialJson(json!({
                "accessTokenTTL": 900,
                "accessTokenMaxTTL": 900,
                "accessTokenNumUsesLimit": 0,
                "accessTokenPeriod": 0,
                "lockoutEnabled": true,
                "lockoutThreshold": 3,
                "lockoutDurationSeconds": 300,
                "lockoutCounterResetSeconds": 30,
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"identityUniversalAuth":{"clientId":"client-1",
                    "accessTokenTTL":900,"accessTokenMaxTTL":900}}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let config = client
            .attach_bounded_ssh_universal_auth("i1")
            .await
            .unwrap();
        assert_eq!(config.access_token_ttl, Some(900));
        assert_eq!(config.access_token_max_ttl, Some(900));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_identities_scopes_by_org_and_requests_a_full_page() {
        let mut server = mockito::Server::new_async().await;
        // The default page is 20 rows; without an explicit limit the 21st
        // identity onward vanishes and then resolves as "no match".
        let mock = server
            .mock("GET", "/v1/identities")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("orgId".into(), "org-42".into()),
                mockito::Matcher::UrlEncoded("limit".into(), PAGE_LIMIT.to_string()),
                mockito::Matcher::UrlEncoded("offset".into(), "0".into()),
            ]))
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
    async fn list_identities_walks_every_page() {
        let mut server = mockito::Server::new_async().await;
        // A full page means "there may be more"; a short page ends the walk.
        let full: Vec<String> = (0..PAGE_LIMIT)
            .map(|n| {
                format!(
                    r#"{{"identityId":"i{n}","identity":{{"id":"i{n}","name":"id-{n}","authMethods":[]}}}}"#
                )
            })
            .collect();
        let first = server
            .mock("GET", "/v1/identities")
            .match_query(mockito::Matcher::UrlEncoded("offset".into(), "0".into()))
            .with_status(200)
            .with_body(format!(r#"{{"identities":[{}]}}"#, full.join(",")))
            .create_async()
            .await;
        let second = server
            .mock("GET", "/v1/identities")
            .match_query(mockito::Matcher::UrlEncoded(
                "offset".into(),
                PAGE_LIMIT.to_string(),
            ))
            .with_status(200)
            .with_body(
                r#"{"identities":[{"identityId":"tail","identity":{"id":"tail","name":"last","authMethods":[]}}]}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let identities = client.list_identities().await.unwrap();

        first.assert_async().await;
        second.assert_async().await;
        assert_eq!(identities.len(), PAGE_LIMIT + 1);
        assert_eq!(identities.last().unwrap().name(), "last");
    }

    #[test]
    fn walk_stops_on_total_count_even_when_the_page_is_full() {
        // The org route ignores limit/offset and returns everything, so a full
        // page is not evidence that more exists. Without totalCount as the
        // authority the walk re-fetched the identical response until MAX_PAGES.
        assert!(InfisicalClient::walk_is_done(
            PAGE_LIMIT,
            PAGE_LIMIT,
            Some(PAGE_LIMIT)
        ));
        // Genuinely more to fetch.
        assert!(!InfisicalClient::walk_is_done(
            PAGE_LIMIT,
            PAGE_LIMIT,
            Some(PAGE_LIMIT + 5)
        ));
        // No totalCount: fall back to the short-page rule.
        assert!(!InfisicalClient::walk_is_done(PAGE_LIMIT, PAGE_LIMIT, None));
        assert!(InfisicalClient::walk_is_done(3, 3, None));
        // An empty page always terminates, whatever totalCount claims.
        assert!(InfisicalClient::walk_is_done(0, 5, Some(99)));
    }

    #[tokio::test]
    async fn a_non_paginating_route_is_fetched_exactly_once() {
        let mut server = mockito::Server::new_async().await;
        // Exactly PAGE_LIMIT rows with a matching totalCount: the boundary case
        // where a short-page rule loops forever and duplicates every row.
        let rows: Vec<String> = (0..PAGE_LIMIT)
            .map(|n| {
                format!(
                    r#"{{"identityId":"i{n}","identity":{{"id":"i{n}","name":"id-{n}","authMethods":[]}}}}"#
                )
            })
            .collect();
        let mock = server
            .mock("GET", "/v1/identities")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(format!(
                r#"{{"identities":[{}],"totalCount":{PAGE_LIMIT}}}"#,
                rows.join(",")
            ))
            .expect(1)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let identities = client.list_identities().await.unwrap();

        mock.assert_async().await;
        assert_eq!(identities.len(), PAGE_LIMIT, "rows must not be duplicated");
    }

    #[tokio::test]
    async fn list_project_identities_requests_a_full_page() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/projects/p1/memberships/identities")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("limit".into(), PAGE_LIMIT.to_string()),
                mockito::Matcher::UrlEncoded("offset".into(), "0".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"identityMemberships":[{"identityId":"i1","roles":[{"role":"viewer","customRoleSlug":null}],
                   "identity":{"id":"i1","name":"ci","authMethods":[]}}]}"#,
            )
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let memberships = client.list_project_identities("p1").await.unwrap();

        mock.assert_async().await;
        assert_eq!(memberships[0].role(), "viewer");
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
            .create_project("new-svc", None, None, "secret-manager", true, false)
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
    async fn project_scoped_rename_uses_the_project_route() {
        let mut server = mockito::Server::new_async().await;
        // The org route rejects project-scoped identities with
        // "Identity is managed by project", so scope must pick the route.
        let mock = server
            .mock("PATCH", "/v1/projects/p1/identities/i1")
            .match_body(mockito::Matcher::Json(json!({"name": "renamed"})))
            .with_status(200)
            .with_body(r#"{"identity":{"id":"i1","name":"renamed"}}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        client
            .update_project_identity("p1", "i1", "renamed")
            .await
            .unwrap();
        mock.assert_async().await;
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
    async fn not_found_is_its_own_error_kind() {
        let mut server = mockito::Server::new_async().await;
        // `client-secret` distinguishes "Universal Auth not attached yet" from
        // "your session died"; both used to arrive as a generic Api error.
        server
            .mock("GET", "/v1/auth/universal-auth/identities/i1")
            .with_status(404)
            .with_body(r#"{"message":"not configured"}"#)
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let err = client.get_universal_auth("i1").await.unwrap_err();
        assert!(matches!(err, IfpError::NotFound(_)));
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

    #[tokio::test]
    async fn mutations_tolerate_an_empty_body() {
        let mut server = mockito::Server::new_async().await;
        // A 204 is a successful PATCH. Parsing it as JSON reported
        // "unexpected response shape: EOF while parsing a value" *after* the
        // change had already been applied.
        let mock = server
            .mock("PATCH", "/v1/projects/p1/memberships/identities/i1")
            .with_status(204)
            .with_body("")
            .create_async()
            .await;

        let client = InfisicalClient::with_parts(&server.url(), "t", "org-1");
        let value = client
            .set_project_identity_role("p1", "i1", "viewer")
            .await
            .unwrap();
        mock.assert_async().await;
        assert!(value.is_null());
    }
}
