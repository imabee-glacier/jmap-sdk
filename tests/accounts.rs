use glacier_mail::accounts::{Accounts, Gateway, ImportAccounts};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
fn write_import(dir: &std::path::Path, accounts: serde_json::Value) -> std::path::PathBuf {
    let path = dir.join("import.json");
    fs::write(&path, json!({"accounts":accounts}).to_string()).expect("file");
    path
}
fn profile(name: &str) -> serde_json::Value {
    json!({"name":name,"session_url":"https://mail.example.test/jmap/session","username":format!("{name}@example.test"),"password":"test-secret-that-must-not-appear"})
}
#[test]
fn imports_multiple_accounts_encrypts_verifies_and_deletes_plaintext() {
    let dir = tempfile::tempdir().expect("temp");
    let accounts = Accounts {
        directory: dir.path().join("vault"),
    };
    let path = write_import(dir.path(), json!([profile("work"), profile("personal")]));
    let result = accounts
        .import(ImportAccounts {
            path: path.to_str().expect("path").into(),
            replace_existing: false,
        })
        .expect("import");
    assert_eq!(result["plaintext_deleted"], true);
    assert!(!path.exists());
    let raw = fs::read(accounts.directory.join("accounts.enc")).expect("ciphertext");
    assert!(!String::from_utf8_lossy(&raw).contains("test-secret"));
    assert!(
        !accounts
            .list()
            .expect("list")
            .to_string()
            .contains("test-secret")
    );
    assert!(accounts.resolve(None).is_err());
    for name in ["work", "personal"] {
        let (selected, config) = accounts.resolve(Some(name)).expect("resolve");
        assert_eq!(selected, name);
        assert_eq!(config.username, Some(format!("{name}@example.test")));
        assert_eq!(
            config.password.as_deref(),
            Some("test-secret-that-must-not-appear")
        );
    }
    accounts.select("work").expect("default");
    assert_eq!(accounts.resolve(None).expect("default").0, "work");
    for name in ["accounts.enc", "vault.key"] {
        assert_eq!(
            fs::metadata(accounts.directory.join(name))
                .expect("mode")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn invalid_import_is_redacted_and_plaintext_is_kept() {
    let dir = tempfile::tempdir().expect("temp");
    let accounts = Accounts {
        directory: dir.path().join("vault"),
    };
    let path = dir.path().join("bad.json");
    fs::write(
        &path,
        r#"{"accounts":[{"password":"do-not-echo-this","unexpected":"another-secret"}]}"#,
    )
    .expect("file");
    let error = accounts
        .import(ImportAccounts {
            path: path.to_str().expect("path").into(),
            replace_existing: false,
        })
        .expect_err("bad");
    assert!(!format!("{error:#}").contains("do-not-echo"));
    assert!(path.exists());
}
#[test]
fn wrong_key_or_tampering_fails_closed() {
    let dir = tempfile::tempdir().expect("temp");
    let accounts = Accounts {
        directory: dir.path().join("vault"),
    };
    let path = write_import(dir.path(), json!([profile("work")]));
    accounts
        .import(ImportAccounts {
            path: path.to_str().expect("path").into(),
            replace_existing: false,
        })
        .expect("import");
    let path = accounts.directory.join("accounts.enc");
    let mut bytes = fs::read(&path).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&path, bytes).expect("tamper");
    assert!(accounts.list().is_err());
    assert!(accounts.resolve(Some("work")).is_err());
}
#[test]
fn replace_requires_explicit_flag_and_keeps_other_accounts() {
    let dir = tempfile::tempdir().expect("temp");
    let accounts = Accounts {
        directory: dir.path().join("vault"),
    };
    let path = write_import(dir.path(), json!([profile("work"), profile("personal")]));
    accounts
        .import(ImportAccounts {
            path: path.to_str().expect("path").into(),
            replace_existing: false,
        })
        .expect("import");
    let path = write_import(dir.path(), json!([profile("work")]));
    assert!(
        accounts
            .import(ImportAccounts {
                path: path.to_str().expect("path").into(),
                replace_existing: false
            })
            .is_err()
    );
    assert!(path.exists());
    accounts
        .import(ImportAccounts {
            path: path.to_str().expect("path").into(),
            replace_existing: true,
        })
        .expect("replace");
    assert_eq!(
        accounts.list().expect("list")["accounts"]
            .as_array()
            .expect("accounts")
            .len(),
        2
    );
    accounts.remove("work").expect("remove");
    assert!(accounts.resolve(Some("work")).is_err());
    assert!(accounts.resolve(Some("personal")).is_ok());
}
#[test]
fn duplicate_names_do_not_partially_import() {
    let dir = tempfile::tempdir().expect("temp");
    let accounts = Accounts {
        directory: dir.path().join("vault"),
    };
    let path = write_import(dir.path(), json!([profile("work"), profile("work")]));
    assert!(
        accounts
            .import(ImportAccounts {
                path: path.to_str().expect("path").into(),
                replace_existing: false
            })
            .is_err()
    );
    assert!(path.exists());
    assert_eq!(accounts.list().expect("list")["accounts"], json!([]));
}
#[tokio::test]
async fn gateway_account_tools_work_without_credentials_or_network() {
    let dir = tempfile::tempdir().expect("temp");
    let gateway = Gateway {
        accounts: Accounts {
            directory: dir.path().join("vault"),
        },
    };
    let path = write_import(dir.path(), json!([profile("work")]));
    let result = gateway
        .execute("import_accounts", json!({"path":path}))
        .await
        .expect("import");
    assert_eq!(result["plaintext_deleted"], true);
    assert_eq!(
        gateway
            .execute("list_accounts", json!({}))
            .await
            .expect("list")["default_account"],
        "work"
    );
    assert!(
        gateway
            .execute("account_status", json!({"account":"unknown"}))
            .await
            .is_err()
    );
}
