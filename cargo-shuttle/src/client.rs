use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use headers::{Authorization, HeaderMapExt};
use percent_encoding::utf8_percent_encode;
use reqwest::header::HeaderMap;
use reqwest::{Request, Response};
use reqwest_middleware::{ClientWithMiddleware, Middleware, Next, RequestBuilder};
use serde::{Deserialize, Serialize};
use shuttle_common::constants::headers::X_CARGO_SHUTTLE_VERSION;
use shuttle_common::log::{LogsRange, LogsResponseBeta};
use shuttle_common::models::deployment::{
    DeploymentRequest, DeploymentRequestBeta, UploadArchiveResponseBeta,
};
use shuttle_common::models::{deployment, project, service, team, user, ToJson};
use shuttle_common::{resource, ApiKey, LogItem, VersionInfo};
use task_local_extensions::Extensions;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Clone)]
pub struct ShuttleApiClient {
    client: ClientWithMiddleware,
    api_url: String,
    api_key: Option<ApiKey>,
}

impl ShuttleApiClient {
    pub fn new(api_url: String, api_key: Option<ApiKey>) -> Self {
        let client = reqwest::Client::builder()
            .default_headers(
                HeaderMap::try_from(&HashMap::from([(
                    X_CARGO_SHUTTLE_VERSION.clone(),
                    crate::VERSION.to_owned(),
                )]))
                .unwrap(),
            )
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        let client = reqwest_middleware::ClientBuilder::new(client)
            .with(LoggingMiddleware)
            .build();
        Self {
            client,
            api_url,
            api_key,
        }
    }

    pub fn set_api_key(&mut self, api_key: ApiKey) {
        self.api_key = Some(api_key);
    }

    fn set_auth_bearer(&self, builder: RequestBuilder) -> RequestBuilder {
        if let Some(ref api_key) = self.api_key {
            builder.bearer_auth(api_key.as_ref())
        } else {
            builder
        }
    }

    pub async fn get_api_versions(&self) -> Result<VersionInfo> {
        let url = format!("{}/versions", self.api_url);

        self.client
            .get(url)
            .send()
            .await?
            .json()
            .await
            .context("parsing API version info")
    }

    pub async fn check_project_name(&self, project_name: &str) -> Result<bool> {
        let url = format!("{}/projects/name/{project_name}", self.api_url);

        self.client
            .get(url)
            .send()
            .await
            .context("failed to check project name availability")?
            .to_json()
            .await
            .context("parsing name check response")
    }

    pub async fn check_project_name_beta(&self, project_name: &str) -> Result<bool> {
        let url = format!("{}/projects/{project_name}/name", self.api_url);

        self.client
            .get(url)
            .send()
            .await
            .context("failed to check project name availability")?
            .to_json()
            .await
            .context("parsing name check response")
    }

    pub async fn get_current_user(&self) -> Result<user::Response> {
        self.get("/users/me".to_owned()).await
    }

    pub async fn deploy(
        &self,
        project: &str,
        deployment_req: DeploymentRequest,
    ) -> Result<deployment::Response> {
        let path = format!("/projects/{project}/services/{project}");
        let deployment_req = rmp_serde::to_vec(&deployment_req)
            .context("serialize DeploymentRequest as a MessagePack byte vector")?;

        let url = format!("{}{}", self.api_url, path);
        let mut builder = self.client.post(url);
        builder = self.set_auth_bearer(builder);

        builder
            .header("Transfer-Encoding", "chunked")
            .body(deployment_req)
            .send()
            .await
            .context("failed to send deployment to the Shuttle server")?
            .to_json()
            .await
    }

    pub async fn deploy_beta(
        &self,
        project: &str,
        deployment_req: DeploymentRequestBeta,
    ) -> Result<deployment::ResponseBeta> {
        let path = format!("/projects/{project}/deployments");
        self.post(path, Some(deployment_req))
            .await
            .context("failed to start deployment")?
            .to_json()
            .await
    }

    pub async fn upload_archive_beta(
        &self,
        project: &str,
        data: Vec<u8>,
    ) -> Result<UploadArchiveResponseBeta> {
        let path = format!("/projects/{project}/deployments/archives");

        let url = format!("{}{}", self.api_url, path);
        let mut builder = self.client.post(url);
        builder = self.set_auth_bearer(builder);

        builder
            .body(data)
            .send()
            .await
            .context("failed to upload archive")?
            .to_json()
            .await
    }

    pub async fn stop_service(&self, project: &str) -> Result<service::Summary> {
        let path = format!("/projects/{project}/services/{project}");

        self.delete(path).await
    }

    pub async fn get_service(&self, project: &str) -> Result<service::Summary> {
        let path = format!("/projects/{project}/services/{project}");

        self.get(path).await
    }

    pub async fn get_service_resources(&self, project: &str) -> Result<Vec<resource::Response>> {
        self.get(format!("/projects/{project}/services/{project}/resources"))
            .await
    }
    pub async fn get_service_resources_beta(
        &self,
        project: &str,
    ) -> Result<Vec<resource::Response>> {
        self.get(format!("/projects/{project}/resources")).await
    }

    pub async fn delete_service_resource(
        &self,
        project: &str,
        resource_type: &resource::Type,
    ) -> Result<()> {
        let r#type = resource_type.to_string();
        let r#type = utf8_percent_encode(&r#type, percent_encoding::NON_ALPHANUMERIC).to_owned();

        self.delete(format!(
            "/projects/{project}/services/{project}/resources/{}",
            r#type
        ))
        .await
    }
    pub async fn delete_service_resource_beta(
        &self,
        project: &str,
        resource_type: &resource::Type,
    ) -> Result<()> {
        let r#type = resource_type.to_string();
        let r#type = utf8_percent_encode(&r#type, percent_encoding::NON_ALPHANUMERIC).to_owned();

        self.delete(format!("/projects/{project}/resources/{}", r#type))
            .await
    }

    pub async fn create_project(
        &self,
        project: &str,
        config: &project::Config,
    ) -> Result<project::Response> {
        self.post(format!("/projects/{project}"), Some(config))
            .await
            .context("failed to make create project request")?
            .to_json()
            .await
    }
    pub async fn create_project_beta(&self, name: &str) -> Result<project::ResponseBeta> {
        self.post(format!("/projects/{name}"), None::<()>)
            .await
            .context("failed to make create project request")?
            .to_json()
            .await
    }

    pub async fn clean_project(&self, project: &str) -> Result<String> {
        let path = format!("/projects/{project}/clean");

        self.post(path, Option::<String>::None)
            .await
            .context("failed to get clean output")?
            .to_json()
            .await
    }

    pub async fn get_project(&self, project: &str) -> Result<project::Response> {
        self.get(format!("/projects/{project}")).await
    }
    pub async fn get_project_beta(&self, project: &str) -> Result<project::ResponseBeta> {
        self.get(format!("/projects/{project}")).await
    }

    pub async fn get_projects_list(&self) -> Result<Vec<project::Response>> {
        self.get("/projects".to_owned()).await
    }
    pub async fn get_projects_list_beta(&self) -> Result<project::ResponseListBeta> {
        self.get("/projects".to_owned()).await
    }

    pub async fn stop_project(&self, project: &str) -> Result<project::Response> {
        let path = format!("/projects/{project}");

        self.delete(path).await
    }

    pub async fn delete_project(&self, project: &str) -> Result<String> {
        let path = format!("/projects/{project}/delete");
        let url = format!("{}{}", self.api_url, path);
        let mut builder = self.client.delete(url);
        builder = self.set_auth_bearer(builder);
        // project delete on alpha can take a while
        builder = builder.timeout(Duration::from_secs(60 * 5));

        builder
            .send()
            .await
            .context("failed to make delete request")?
            .to_json()
            .await
    }
    pub async fn delete_project_beta(&self, project: &str) -> Result<String> {
        self.delete(format!("/projects/{project}")).await
    }

    pub async fn get_teams_list(&self) -> Result<Vec<team::Response>> {
        self.get("/teams".to_string()).await
    }

    pub async fn get_team_projects_list(&self, team_id: &str) -> Result<Vec<project::Response>> {
        self.get(format!("/teams/{team_id}/projects")).await
    }
    pub async fn get_team_projects_list_beta(
        &self,
        team_id: &str,
    ) -> Result<project::ResponseListBeta> {
        self.get(format!("/teams/{team_id}/projects")).await
    }

    pub async fn get_logs(
        &self,
        project: &str,
        deployment_id: &str,
        range: LogsRange,
    ) -> Result<Vec<LogItem>> {
        let mut path = format!("/projects/{project}/deployments/{deployment_id}/logs");
        Self::add_range_query(range, &mut path);

        self.get(path)
            .await
            .context("Failed parsing logs. Is your cargo-shuttle outdated?")
    }
    pub async fn get_deployment_logs_beta(
        &self,
        project: &str,
        deployment_id: &str,
    ) -> Result<LogsResponseBeta> {
        let path = format!("/projects/{project}/deployments/{deployment_id}/logs");

        self.get(path).await.context("Failed parsing logs.")
    }
    pub async fn get_project_logs_beta(&self, project: &str) -> Result<LogsResponseBeta> {
        let path = format!("/projects/{project}/logs");

        self.get(path).await.context("Failed parsing logs.")
    }

    pub async fn get_logs_ws(
        &self,
        project: &str,
        deployment_id: &str,
        range: LogsRange,
    ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let mut path = format!("/projects/{project}/ws/deployments/{deployment_id}/logs");
        Self::add_range_query(range, &mut path);

        self.ws_get(path).await
    }

    fn add_range_query(range: LogsRange, path: &mut String) {
        match range {
            LogsRange::Head(n) => {
                path.push_str("?head=");
                path.push_str(&n.to_string())
            }
            LogsRange::Tail(n) => {
                path.push_str("?tail=");
                path.push_str(&n.to_string())
            }
            _ => {}
        };
    }

    pub async fn get_deployments(
        &self,
        project: &str,
        page: u32,
        limit: u32,
    ) -> Result<Vec<deployment::Response>> {
        let path = format!(
            "/projects/{project}/deployments?page={}&limit={}",
            page.saturating_sub(1),
            limit,
        );

        self.get(path).await
    }
    pub async fn get_deployments_beta(
        &self,
        project: &str,
    ) -> Result<Vec<deployment::ResponseBeta>> {
        let path = format!("/projects/{project}/deployments");

        self.get(path).await
    }
    pub async fn get_current_deployment_beta(
        &self,
        project: &str,
    ) -> Result<Option<deployment::ResponseBeta>> {
        let path = format!("/projects/{project}/deployments/current");

        self.get(path).await
    }

    pub async fn get_deployment_beta(
        &self,
        project: &str,
        deployment_id: &str,
    ) -> Result<deployment::ResponseBeta> {
        let path = format!("/projects/{project}/deployments/{deployment_id}");

        self.get(path).await
    }

    pub async fn get_deployment_details(
        &self,
        project: &str,
        deployment_id: &Uuid,
    ) -> Result<deployment::Response> {
        let path = format!("/projects/{project}/deployments/{deployment_id}");

        self.get(path).await
    }

    pub async fn reset_api_key(&self) -> Result<Response> {
        self.put("/users/reset-api-key".into(), Option::<()>::None)
            .await
    }

    async fn ws_get(&self, path: String) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let ws_url = self.api_url.clone().replace("http", "ws");
        let url = format!("{ws_url}{path}");
        let mut request = url.into_client_request()?;

        if let Some(ref api_key) = self.api_key {
            let auth_header = Authorization::bearer(api_key.as_ref())?;
            request.headers_mut().typed_insert(auth_header);
        }

        let (stream, _) = connect_async(request).await.with_context(|| {
            error!("failed to connect to websocket");
            "could not connect to websocket"
        })?;

        Ok(stream)
    }

    async fn get<M>(&self, path: String) -> Result<M>
    where
        M: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.api_url, path);

        let mut builder = self.client.get(url);
        builder = self.set_auth_bearer(builder);

        builder
            .send()
            .await
            .context("failed to make get request")?
            .to_json()
            .await
    }

    async fn post<T: Serialize>(&self, path: String, body: Option<T>) -> Result<Response> {
        let url = format!("{}{}", self.api_url, path);

        let mut builder = self.client.post(url);
        builder = self.set_auth_bearer(builder);

        if let Some(body) = body {
            let body = serde_json::to_string(&body)?;
            debug!("Outgoing body: {}", body);
            builder = builder.body(body);
            builder = builder.header("Content-Type", "application/json");
        }

        Ok(builder.send().await?)
    }

    async fn put<T: Serialize>(&self, path: String, body: Option<T>) -> Result<Response> {
        let url = format!("{}{}", self.api_url, path);

        let mut builder = self.client.put(url);
        builder = self.set_auth_bearer(builder);

        if let Some(body) = body {
            let body = serde_json::to_string(&body)?;
            debug!("Outgoing body: {}", body);
            builder = builder.body(body);
            builder = builder.header("Content-Type", "application/json");
        }

        Ok(builder.send().await?)
    }

    async fn delete<M>(&self, path: String) -> Result<M>
    where
        M: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.api_url, path);

        let mut builder = self.client.delete(url);
        builder = self.set_auth_bearer(builder);

        builder
            .send()
            .await
            .context("failed to make delete request")?
            .to_json()
            .await
    }
}

struct LoggingMiddleware;

#[async_trait::async_trait]
impl Middleware for LoggingMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        debug!("Request: {} {}", req.method(), req.url());
        let res = next.run(req, extensions).await;
        match res {
            Ok(ref res) => {
                debug!("Response: {}", res.status());
            }
            Err(ref e) => {
                debug!("Response error: {}", e);
            }
        }
        res
    }
}
