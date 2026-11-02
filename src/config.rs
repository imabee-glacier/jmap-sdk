use anyhow::{Context, Result, ensure};
use std::{env, path::PathBuf};
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub session_url: Url,
    pub username: Option<String>,
    pub password: Option<String>,
    pub bearer: Option<String>,
    pub account_id: Option<String>,
    pub database: PathBuf,
    pub allow_http: bool,
    pub max_blob_bytes: usize,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let session_url = Url::parse(
            &env::var("JMAP_SESSION_URL")
                .context("Set JMAP_SESSION_URL using the agent's configuration")?,
        )?;
        let config = Self {
            session_url,
            username: env::var("JMAP_USERNAME").ok(),
            password: env::var("JMAP_PASSWORD").ok(),
            bearer: env::var("JMAP_TOKEN").ok(),
            account_id: env::var("JMAP_ACCOUNT_ID").ok(),
            database: env::var_os("GLACIER_DATABASE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let base = env::var_os("XDG_STATE_HOME")
                        .map(PathBuf::from)
                        .or_else(|| {
                            env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state"))
                        })
                        .unwrap_or_else(|| PathBuf::from("."));
                    base.join("glacier-mail/state.sqlite")
                }),
            allow_http: env::var("GLACIER_ALLOW_HTTP").as_deref() == Ok("1"),
            max_blob_bytes: 25 * 1024 * 1024,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.session_url.username().is_empty() && self.session_url.password().is_none(),
            "Do not put credentials in URLs"
        );
        ensure!(
            self.session_url.scheme() == "https"
                || (self.allow_http && self.session_url.scheme() == "http"),
            "JMAP requires HTTPS (GLACIER_ALLOW_HTTP=1 is for local tests only)"
        );
        ensure!(
            self.bearer.as_ref().is_some_and(|s| !s.is_empty())
                || (self.username.as_ref().is_some_and(|s| !s.is_empty())
                    && self.password.as_ref().is_some_and(|s| !s.is_empty())),
            "Set JMAP_TOKEN or JMAP_USERNAME and JMAP_PASSWORD in the credential environment"
        );
        Ok(())
    }
}
