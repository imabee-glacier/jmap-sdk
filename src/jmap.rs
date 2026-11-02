use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, RequestBuilder, Response};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::OnceCell;
use url::Url;

pub const CORE: &str = "urn:ietf:params:jmap:core";
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
/// Keep the message stable but say why: connect, TLS, proxy, DNS or timeout.
/// The URL is stripped; credentials only ever travel in headers, never in the error.
fn failure(what: &str, error: reqwest::Error) -> anyhow::Error {
    use std::error::Error;
    let error = error.without_url();
    let mut causes = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        causes.push(cause.to_string());
        source = cause.source();
    }
    causes.dedup();
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else {
        "request"
    };
    anyhow::anyhow!("{what} ({kind}: {})", causes.join(": "))
}
#[derive(Clone)]
pub struct Jmap {
    pub config: Arc<Config>,
    http: Client,
    session: Arc<OnceCell<Value>>,
}
impl Jmap {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config: Arc::new(config),
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(45))
                .connect_timeout(Duration::from_secs(10))
                .build()?,
            session: Arc::new(OnceCell::new()),
        })
    }
    fn auth(&self, request: RequestBuilder) -> RequestBuilder {
        if let Some(token) = &self.config.bearer {
            request.bearer_auth(token)
        } else {
            request.basic_auth(
                self.config.username.as_deref().unwrap_or_default(),
                self.config.password.as_deref(),
            )
        }
    }
    pub fn trusted_url(&self, raw: &str) -> Result<Url> {
        let url = self.config.session_url.join(raw)?;
        ensure!(
            url.origin() == self.config.session_url.origin()
                && url.username().is_empty()
                && url.password().is_none(),
            "JMAP advertised a different origin; fix the server's public URL configuration before transferring credentials"
        );
        Ok(url)
    }
    async fn bytes(&self, mut response: Response, limit: usize) -> Result<Vec<u8>> {
        ensure!(
            response.status().is_success(),
            "JMAP HTTP status {}",
            response.status().as_u16()
        );
        ensure!(
            response.content_length().is_none_or(|n| n <= limit as u64),
            "Response exceeds size limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("JMAP response interrupted")?
        {
            ensure!(
                bytes.len() + chunk.len() <= limit,
                "Response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn json(&self, response: Response) -> Result<Value> {
        serde_json::from_slice(&self.bytes(response, 40 * 1024 * 1024).await?)
            .context("Invalid JMAP JSON response")
    }
    pub async fn session(&self) -> Result<&Value> {
        self.session
            .get_or_try_init(|| async {
                let mut url = self.config.session_url.clone();
                for _ in 0..5 {
                    let response = self
                        .auth(self.http.get(url.clone()))
                        .send()
                        .await
                        .map_err(|e| failure("Could not reach JMAP session", e))?;
                    if response.status().is_redirection() {
                        let location = response
                            .headers()
                            .get("location")
                            .context("Redirect missing Location")?
                            .to_str()?;
                        url = self.trusted_url(url.join(location)?.as_str())?;
                        continue;
                    }
                    let session = self.json(response).await?;
                    ensure!(
                        session["capabilities"][CORE].is_object()
                            && session["capabilities"][MAIL].is_object(),
                        "Server does not advertise JMAP core and mail"
                    );
                    self.trusted_url(session["apiUrl"].as_str().context("Missing apiUrl")?)?;
                    return Ok(session);
                }
                bail!("Too many JMAP discovery redirects")
            })
            .await
    }
    pub async fn account(&self) -> Result<String> {
        let session = self.session().await?;
        let accounts = session["accounts"]
            .as_object()
            .context("Missing accounts")?;
        let id = if let Some(id) = &self.config.account_id {
            id.clone()
        } else if let Some(id) = session["primaryAccounts"][MAIL].as_str() {
            id.to_owned()
        } else {
            let eligible: Vec<_> = accounts
                .iter()
                .filter(|(_, a)| a["accountCapabilities"][MAIL].is_object())
                .collect();
            ensure!(
                eligible.len() == 1,
                "Select JMAP_ACCOUNT_ID; account selection is ambiguous"
            );
            eligible[0].0.clone()
        };
        ensure!(
            accounts
                .get(&id)
                .is_some_and(|a| a["accountCapabilities"][MAIL].is_object()),
            "Account does not support mail"
        );
        Ok(id)
    }
    pub async fn call(&self, method: &str, mut args: Value) -> Result<Value> {
        let session = self.session().await?;
        let account = self.account().await?;
        args["accountId"] = json!(account);
        let mut using = vec![CORE, MAIL];
        if method.starts_with("EmailSubmission/") || method.starts_with("Identity/") {
            ensure!(
                session["capabilities"][SUBMISSION].is_object()
                    && session["accounts"][&account]["accountCapabilities"][SUBMISSION].is_object(),
                "Account does not support submission"
            );
            using.push(SUBMISSION);
        }
        let url = self.trusted_url(session["apiUrl"].as_str().context("Missing apiUrl")?)?;
        let response = self.auth(self.http.post(url)).json(&json!({"using":using,"methodCalls":[[method,args,"0"]]})).send().await.map_err(|e| failure("JMAP request interrupted; a write may have completed, inspect status before retrying", e))?;
        let body = self.json(response).await?;
        let responses = body["methodResponses"]
            .as_array()
            .context("Missing methodResponses")?;
        let result = responses
            .iter()
            .find(|r| r[2] == "0")
            .context("Missing correlated JMAP response")?;
        ensure!(
            result[0] == method,
            "JMAP method failed: {}",
            result[1]["type"].as_str().unwrap_or("invalidResponse")
        );
        Ok(result[1].clone())
    }
    pub async fn get_email(&self, id: &str) -> Result<Value> {
        let response = self.call("Email/get", json!({"ids":[id],"properties":["id","blobId","threadId","mailboxIds","keywords","from","to","cc","bcc","replyTo","subject","messageId","inReplyTo","references","sentAt","receivedAt","textBody","htmlBody","bodyValues","attachments","bodyStructure"],"fetchAllBodyValues":true,"maxBodyValueBytes":1048576})).await?;
        let email = response["list"]
            .as_array()
            .and_then(|l| l.first())
            .context("Email not found")?
            .clone();
        Ok(email)
    }
    pub async fn role(&self, role: &str) -> Result<String> {
        let mailboxes = self.call("Mailbox/get", json!({})).await?;
        mailboxes["list"]
            .as_array()
            .context("Missing mailbox list")?
            .iter()
            .find(|m| m["role"] == role)
            .and_then(|m| m["id"].as_str())
            .map(str::to_owned)
            .with_context(|| format!("Account needs a mailbox with role {role}"))
    }
    pub async fn archive_mailbox(&self) -> Result<String> {
        let result = self.call("Mailbox/get", json!({})).await?;
        if let Some(id) = result["list"]
            .as_array()
            .context("Missing mailboxes")?
            .iter()
            .find(|m| m["role"] == "archive")
            .and_then(|m| m["id"].as_str())
        {
            return Ok(id.to_owned());
        }
        let result = self
            .call(
                "Mailbox/set",
                json!({"create":{"archive":{"name":"Archive","role":"archive"}}}),
            )
            .await?;
        created(&result, "archive")
    }
    pub async fn upload(&self, content_type: &str, bytes: Vec<u8>) -> Result<Value> {
        ensure!(
            bytes.len() <= self.config.max_blob_bytes,
            "Attachment exceeds 25 MiB limit"
        );
        let session = self.session().await?;
        let max = session["capabilities"][CORE]["maxSizeUpload"]
            .as_u64()
            .unwrap_or(u64::MAX);
        ensure!(
            bytes.len() as u64 <= max,
            "Attachment exceeds server upload limit"
        );
        let template = session["uploadUrl"].as_str().context("Missing uploadUrl")?;
        let url =
            self.trusted_url(&template.replace("{accountId}", &encode(&self.account().await?)))?;
        let response = self
            .auth(self.http.post(url))
            .header("Content-Type", content_type)
            .body(bytes)
            .send()
            .await
            .map_err(|e| failure("Upload interrupted", e))?;
        let result = self.json(response).await?;
        ensure!(
            result["accountId"] == self.account().await? && result["blobId"].is_string(),
            "Invalid upload response"
        );
        Ok(result)
    }
    pub async fn download(&self, blob: &str, name: &str) -> Result<Vec<u8>> {
        let session = self.session().await?;
        let template = session["downloadUrl"]
            .as_str()
            .context("Missing downloadUrl")?;
        let url = template
            .replace("{accountId}", &encode(&self.account().await?))
            .replace("{blobId}", &encode(blob))
            .replace("{name}", &encode(name))
            .replace("{type}", "application%2Foctet-stream");
        let response = self
            .auth(self.http.get(self.trusted_url(&url)?))
            .send()
            .await
            .map_err(|e| failure("Download interrupted", e))?;
        self.bytes(response, self.config.max_blob_bytes).await
    }
}
pub fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
pub fn pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
pub fn created(result: &Value, id: &str) -> Result<String> {
    ensure!(
        result["notCreated"][id].is_null(),
        "JMAP object creation failed: {}",
        result["notCreated"][id]["type"]
            .as_str()
            .unwrap_or("unknown")
    );
    result["created"][id]["id"]
        .as_str()
        .map(str::to_owned)
        .context("Creation returned no id")
}
