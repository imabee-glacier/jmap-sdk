use crate::{config::Config, jmap::Jmap, service::Service, store::Store};
use anyhow::{Context, Result, bail, ensure};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Key, KeyInit, Payload},
};
use fs2::FileExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;
const MAGIC: &[u8] = b"JMAP-SDK-VAULT-1\n";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub name: String,
    pub session_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Vault {
    #[serde(default)]
    default_account: Option<String>,
    #[serde(default)]
    accounts: BTreeMap<String, Account>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportFile {
    accounts: Vec<Account>,
    #[serde(default)]
    default_account: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportAccounts {
    pub path: String,
    #[serde(default)]
    pub replace_existing: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountName {
    pub name: String,
}

#[derive(Clone)]
pub struct Accounts {
    pub directory: PathBuf,
}
impl Accounts {
    pub fn from_env() -> Self {
        let directory = std::env::var_os("JMAP_SDK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state"))
                    })
                    .unwrap_or_else(|| PathBuf::from("."));
                base.join("jmap-sdk")
            });
        Self { directory }
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.directory)?;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.directory.join("vault.lock"))?;
        FileExt::lock_exclusive(&file)?;
        Ok(file)
    }
    fn key(&self, create: bool) -> Result<Zeroizing<Vec<u8>>> {
        let path = self.directory.join("vault.key");
        if create && !path.exists() {
            ensure!(
                !self.directory.join("accounts.enc").exists(),
                "Vault key is missing; restore it from backup. Refusing to overwrite encrypted accounts."
            );
            let key = Key::<XChaCha20Poly1305>::generate();
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(&key)?;
            file.sync_all()?;
        }
        let bytes =
            Zeroizing::new(fs::read(&path).context(
                "Vault key unavailable; restore the original key alongside accounts.enc",
            )?);
        ensure!(bytes.len() == 32, "Invalid vault key");
        Ok(bytes)
    }
    fn load(&self) -> Result<Vault> {
        let path = self.directory.join("accounts.enc");
        if !path.exists() {
            return Ok(Vault::default());
        }
        let encrypted = fs::read(path)?;
        ensure!(
            encrypted.starts_with(MAGIC) && encrypted.len() >= MAGIC.len() + 24 + 16,
            "Invalid encrypted vault format"
        );
        let key = self.key(false)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("Invalid vault key"))?;
        let nonce = XNonce::try_from(&encrypted[MAGIC.len()..MAGIC.len() + 24])
            .map_err(|_| anyhow::anyhow!("Invalid vault nonce"))?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    &nonce,
                    Payload {
                        msg: &encrypted[MAGIC.len() + 24..],
                        aad: MAGIC,
                    },
                )
                .map_err(|_| {
                    anyhow::anyhow!("Vault authentication failed; wrong key or corrupted file")
                })?,
        );
        serde_json::from_slice(&plaintext)
            .map_err(|_| anyhow::anyhow!("Invalid decrypted account data"))
    }
    fn save(&self, vault: &Vault) -> Result<()> {
        let key = self.key(true)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("Invalid vault key"))?;
        let nonce = XNonce::generate();
        let plaintext = Zeroizing::new(serde_json::to_vec(vault)?);
        let encrypted = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: MAGIC,
                },
            )
            .map_err(|_| anyhow::anyhow!("Could not encrypt vault"))?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.directory)?;
        tmp.write_all(MAGIC)?;
        tmp.write_all(&nonce)?;
        tmp.write_all(&encrypted)?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.directory.join("accounts.enc"))?;
        File::open(&self.directory)?.sync_all()?;
        let verified = self.load()?;
        ensure!(
            serde_json::to_vec(&verified)? == *plaintext,
            "Saved vault verification failed"
        );
        Ok(())
    }
    pub fn import(&self, args: ImportAccounts) -> Result<Value> {
        let _lock = self.lock()?;
        let source = Path::new(&args.path);
        ensure!(
            fs::symlink_metadata(source)?.file_type().is_file(),
            "Import source must be a regular file, not a symlink"
        );
        let canonical = source.canonicalize()?;
        for reserved in ["accounts.enc", "vault.key", "vault.lock", "state.sqlite"] {
            let file = self.directory.join(reserved);
            ensure!(
                file.canonicalize().ok().as_ref() != Some(&canonical),
                "Cannot import a vault or state file"
            );
        }
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(source)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1024 * 1024, "Account import exceeds 1 MiB");
        // Never attach serde's input-bearing diagnostics: they can expose a password.
        let imported: ImportFile = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("Invalid account file; use the documented JSON format. File preserved.")
        })?;
        ensure!(
            !imported.accounts.is_empty() && imported.accounts.len() <= 100,
            "Import 1..100 accounts at a time"
        );
        let mut vault = self.load()?;
        let mut names = HashSet::new();
        for account in &imported.accounts {
            ensure!(
                !account.name.is_empty()
                    && account.name.len() <= 64
                    && account
                        .name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
                "Account names must use 1..64 letters, digits, underscores or hyphens"
            );
            ensure!(
                names.insert(account.name.clone()),
                "Duplicate account name in import file"
            );
            ensure!(
                args.replace_existing || !vault.accounts.contains_key(&account.name),
                "Account already exists; explicit replace_existing is required"
            );
            self.config(account).map_err(|_|anyhow::anyhow!("Invalid account configuration; use HTTPS, a credential pair or token, and no credentials in URLs. File preserved."))?;
        }
        for account in imported.accounts {
            vault.accounts.insert(account.name.clone(), account);
        }
        if let Some(default) = imported.default_account {
            ensure!(
                vault.accounts.contains_key(&default),
                "Default account does not exist"
            );
            vault.default_account = Some(default);
        }
        if vault.accounts.len() == 1 {
            vault.default_account = vault.accounts.keys().next().cloned();
        }
        self.save(&vault)?;
        // Delete only after the encrypted write, fsync, and successful authenticated re-read.
        let deleted = fs::remove_file(source).is_ok();
        Ok(
            json!({"imported_accounts":names.into_iter().collect::<Vec<_>>(),"plaintext_deleted":deleted,"warning":if deleted { Value::Null } else { json!("Accounts saved, but plaintext file deletion failed. Ask the agent to delete the source file; do not reimport.") }}),
        )
    }
    pub fn list(&self) -> Result<Value> {
        let _lock = self.lock()?;
        let vault = self.load()?;
        Ok(
            json!({"default_account":vault.default_account,"accounts":vault.accounts.values().map(|a|json!({"name":a.name,"session_url":a.session_url,"username":a.username,"account_id":a.account_id,"auth":if a.token.is_some(){"bearer"}else{"password"}})).collect::<Vec<_>>()}),
        )
    }
    pub fn select(&self, name: &str) -> Result<Value> {
        let _lock = self.lock()?;
        let mut vault = self.load()?;
        ensure!(vault.accounts.contains_key(name), "Unknown account");
        vault.default_account = Some(name.to_owned());
        self.save(&vault)?;
        Ok(json!({"default_account":name}))
    }
    pub fn remove(&self, name: &str) -> Result<Value> {
        let _lock = self.lock()?;
        let mut vault = self.load()?;
        ensure!(vault.accounts.remove(name).is_some(), "Unknown account");
        if vault.default_account.as_deref() == Some(name) {
            vault.default_account = None;
        }
        self.save(&vault)?;
        Ok(
            json!({"removed_account":name,"note":"Local credentials removed. This does not revoke the password/token at the provider or delete mail."}),
        )
    }
    fn config(&self, account: &Account) -> Result<Config> {
        let config = Config {
            session_url: account.session_url.parse()?,
            username: account.username.clone(),
            password: account.password.clone(),
            bearer: account.token.clone(),
            account_id: account.account_id.clone(),
            database: self.directory.join("state.sqlite"),
            allow_http: false,
            max_blob_bytes: 25 * 1024 * 1024,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn resolve(&self, name: Option<&str>) -> Result<(String, Config)> {
        let _lock = self.lock()?;
        let vault = self.load()?;
        if vault.accounts.is_empty() && name.is_none() {
            return Ok(("environment".into(), Config::from_env()?));
        }
        let name = name
            .or(vault.default_account.as_deref())
            .context("Choose an account with the account argument or select_account")?;
        let account = vault
            .accounts
            .get(name)
            .context("Unknown account; call list_accounts")?;
        Ok((name.to_owned(), self.config(account)?))
    }
}
#[derive(Clone)]
pub struct Gateway {
    pub accounts: Accounts,
}
impl Gateway {
    pub async fn execute(&self, name: &str, mut input: Value) -> Result<Value> {
        let result=match name {
            "list_accounts" => { let _:crate::mail::Empty=serde_json::from_value(input)?;return self.accounts.list(); }
            "import_accounts" => return self.accounts.import(serde_json::from_value(input).map_err(|_|anyhow::anyhow!("Supply only path and optional replace_existing; never put credentials in tool arguments"))?),
            "select_account" => { let args:AccountName=serde_json::from_value(input)?;return self.accounts.select(&args.name); }
            "remove_account" => { let args:AccountName=serde_json::from_value(input)?;return self.accounts.remove(&args.name); }
            _ => {
                let object=input.as_object_mut().context("Tool arguments must be a JSON object")?;
                let name=match object.remove("account") { Some(Value::String(name)) => Some(name),None=>None,_=>bail!("account must be a name string") };
                self.accounts.resolve(name.as_deref())?
            }
        };
        let (account, config) = result;
        let store = Arc::new(Store::open(&config.database)?);
        let service = Service {
            jmap: Jmap::new(config)?,
            store,
        };
        let mut result = service.execute(name, input).await?;
        result["account"] = json!(account);
        Ok(result)
    }
}
