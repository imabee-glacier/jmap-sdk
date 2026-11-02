use anyhow::{Context, Result, bail};
use jmap_sdk::{
    accounts::{Accounts, Gateway},
    transport::definitions,
};
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        if std::env::args().nth(1).as_deref() == Some("serve") {
            eprintln!("{error:#}");
        } else {
            println!("{}", json!({"ok":false,"error":format!("{error:#}")}));
        }
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!(
                "jmap-sdk — Linux JMAP tools for agents\n\n  tools                       Print tool schemas (no credentials needed)\n  call TOOL [JSON | -]         Execute one tool; '-' reads JSON from stdin\n  serve                       Run MCP over stdio\n\nMuse runs these commands for the user. No browser or local web service.\nAccounts: import_accounts reads an account file, encrypts it, then deletes the source.\nUse list_accounts and an account argument on mail calls. Storage: JMAP_SDK_HOME.\nEnvironment credentials remain supported for legacy setups. Default send mode: manual."
            );
            return Ok(());
        }
        Some("tools") => {
            println!("{}", json!({"tools":definitions()}));
            return Ok(());
        }
        Some("call" | "serve") => (),
        _ => bail!("Unknown command; use help"),
    }
    let service = Gateway {
        accounts: Accounts::from_env(),
    };
    if args[0] == "serve" {
        // All operational output is carried as MCP responses; stdout is protocol-only.
        let server = service.serve(rmcp::transport::stdio()).await?;
        server.waiting().await?;
    } else {
        let name = args.get(1).context("Missing tool name")?;
        let input = match args.get(2).map(String::as_str) {
            None => json!({}),
            Some("-") => {
                let mut bytes = Vec::new();
                tokio::io::stdin()
                    .take(2 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                anyhow::ensure!(bytes.len() <= 2 * 1024 * 1024, "Input exceeds 2 MiB");
                serde_json::from_slice(&bytes)?
            }
            Some(value) => serde_json::from_str::<Value>(value)?,
        };
        let result = service.execute(name, input).await?;
        let failed = jmap_sdk::service::has_partial_failure(&result);
        println!("{}", json!({"ok":!failed,"result":result}));
        if failed {
            std::process::exit(1);
        }
    }
    Ok(())
}
