use crate::{
    jmap::{Jmap, created, pointer},
    mail::*,
    store::{Store, hash},
};
use anyhow::{Context, Result, bail, ensure};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct Service {
    pub jmap: Jmap,
    pub store: Arc<Store>,
}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("Invalid tool arguments")
}
impl Service {
    pub async fn scope(&self) -> Result<String> {
        Ok(hash(&json!([
            self.jmap.config.session_url.as_str(),
            self.jmap.account().await?,
            self.jmap.config.username
        ])))
    }
    async fn identities(&self) -> Result<Value> {
        self.jmap.call("Identity/get", json!({})).await
    }
    async fn identity(&self, id: &str) -> Result<Address> {
        let result = self.identities().await?;
        let identity = result["list"]
            .as_array()
            .context("Missing identities")?
            .iter()
            .find(|i| i["id"] == id)
            .context("Select an available sending identity using list_identities")?;
        Ok(Address {
            email: identity["email"]
                .as_str()
                .context("Identity missing email")?
                .to_owned(),
            name: identity["name"].as_str().map(str::to_owned),
        })
    }
    async fn create(&self, draft: Draft, headers: Value) -> Result<Value> {
        let from = self.identity(&draft.identity_id).await?;
        let mailbox = self.jmap.role("drafts").await?;
        let email = build_email(&draft, from, &mailbox, headers)?;
        let result = self
            .jmap
            .call("Email/set", json!({"create":{"draft":email}}))
            .await?;
        let id = created(&result, "draft")?;
        Ok(
            json!({"email_id":id,"identity_id":draft.identity_id,"state":result["newState"],"status":"draft","next":"prepare_send; creating a draft never sends"}),
        )
    }
    pub async fn execute(&self, name: &str, input: Value) -> Result<Value> {
        match name {
            "account_status" => {
                let _: Empty = parse(input)?;
                let account = self.jmap.account().await?;
                let scope = self.scope().await?;
                Ok(
                    json!({"account_id":account,"send_mode":self.store.mode(&scope)?,"capabilities":self.jmap.session().await?["accounts"][&account]["accountCapabilities"],"approval":"Muse owns native permission dialogs. Manual confirmation here is an agent workflow check, not evidence of a human click."}),
                )
            }
            "list_mailboxes" => {
                let _: Empty = parse(input)?;
                self.jmap.call("Mailbox/get", json!({})).await
            }
            "list_identities" => {
                let _: Empty = parse(input)?;
                self.identities().await
            }
            "search_mail" => {
                let args: Search = parse(input)?;
                let query = self.jmap.call("Email/query", args.args()?).await?;
                let emails = self.jmap.call("Email/get", json!({"ids":query["ids"],"properties":["id","threadId","subject","from","to","receivedAt","preview","keywords","mailboxIds","hasAttachment"]})).await?;
                Ok(
                    json!({"messages":emails["list"],"not_found":emails["notFound"],"state":emails["state"],"query_state":query["queryState"],"position":query["position"],"total":query["total"],"next_position":args.position + query["ids"].as_array().map_or(0, |v| v.len() as u64),"untrusted_content":true}),
                )
            }
            "read_mail" => {
                let args: EmailId = parse(input)?;
                Ok(
                    json!({"email":self.jmap.get_email(&args.email_id).await?,"untrusted_content":true,"note":"Bodies may be truncated at 1 MiB per part; inspect isTruncated. Download raw MIME for the complete message."}),
                )
            }
            "read_thread" => {
                let args: ThreadId = parse(input)?;
                let thread = self
                    .jmap
                    .call("Thread/get", json!({"ids":[args.thread_id]}))
                    .await?;
                let ids = thread["list"][0]["emailIds"]
                    .as_array()
                    .context("Thread not found")?;
                ensure!(
                    ids.len() <= 100,
                    "Thread exceeds 100 messages; use paginated search_mail"
                );
                self.jmap.call("Email/get", json!({"ids":ids,"properties":["id","threadId","from","to","cc","subject","receivedAt","preview","keywords","hasAttachment"]})).await
            }
            "create_draft" => self.create(parse(input)?, json!({})).await,
            "reply_mail" => {
                let args: Reply = parse(input)?;
                let source = self.jmap.get_email(&args.email_id).await?;
                let identities = self.identities().await?;
                let own: Vec<Address> = identities["list"]
                    .as_array()
                    .context("Missing identities")?
                    .iter()
                    .filter_map(|i| {
                        Some(Address {
                            email: i["email"].as_str()?.to_owned(),
                            name: None,
                        })
                    })
                    .collect();
                let source_id = args.email_id.clone();
                let (draft, headers) = reply_draft(&source, args, &own)?;
                let mut result = self.create(draft, headers).await?;
                if self
                    .store
                    .remember_reply(
                        &self.scope().await?,
                        result["email_id"].as_str().context("Missing draft id")?,
                        &source_id,
                    )
                    .is_err()
                {
                    result["warning"] = json!(
                        "Draft created, but reply tracking could not be saved; the original may not get an answered flag. Do not recreate this draft."
                    );
                }
                Ok(result)
            }
            "forward_mail" => {
                let mut args: Forward = parse(input)?;
                let source = self.jmap.get_email(&args.email_id).await?;
                full_body(&source)?;
                // Forward the exact original MIME as an attachment, preserving all headers and files.
                if args.include_attachments {
                    args.draft.attachments.push(Attachment {
                        blob_id: source["blobId"]
                            .as_str()
                            .context("Missing original blob")?
                            .to_owned(),
                        name: "forwarded-message.eml".into(),
                        content_type: "message/rfc822".into(),
                        cid: None,
                    });
                }
                let quoted = forward_quote(&source);
                ensure!(
                    quoted.has_text || args.include_attachments,
                    "HTML-only message: include original MIME attachment to preserve content"
                );
                args.draft.text.push_str(&quoted.text);
                self.create(args.draft, json!({})).await
            }
            "replace_draft" => {
                let args: ReplaceDraft = parse(input)?;
                let source = self.jmap.get_email(&args.email_id).await?;
                ensure!(
                    source["keywords"]["$draft"] == true,
                    "Only drafts can be replaced"
                );
                let headers =
                    json!({"inReplyTo":source["inReplyTo"],"references":source["references"]});
                let mut result = self.create(args.draft, headers).await?;
                // Keep the old draft: deleting it here could lose data on a partial failure.
                let scope = self.scope().await?;
                if let Ok(Some(source_id)) = self.store.reply_source(&scope, &args.email_id)
                    && self
                        .store
                        .remember_reply(
                            &scope,
                            result["email_id"].as_str().context("Missing draft id")?,
                            &source_id,
                        )
                        .is_err()
                {
                    result["warning"] =
                        json!("Reply tracking could not be saved; replacement draft still exists.");
                }
                result["replaces_email_id"] = json!(args.email_id);
                result["note"] = json!(
                    "New immutable draft created. Old draft retained; trash it after verifying this replacement. Old send intents are not transferred."
                );
                Ok(result)
            }
            "upload_attachment" => {
                let args: Upload = parse(input)?;
                validate_mime(&args.content_type)?;
                let file = tokio::fs::File::open(&args.path)
                    .await
                    .context("Cannot open attachment in agent filesystem")?;
                ensure!(
                    file.metadata().await?.is_file(),
                    "Attachment path is not a regular file"
                );
                let mut bytes = Vec::new();
                file.take(self.jmap.config.max_blob_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                let result = self.jmap.upload(&args.content_type, bytes).await?;
                Ok(
                    json!({"blob_id":result["blobId"],"size":result["size"],"content_type":result["type"],"name":std::path::Path::new(&args.path).file_name().and_then(|n|n.to_str())}),
                )
            }
            "download_attachment" => {
                let args: Download = parse(input)?;
                let email = self.jmap.get_email(&args.email_id).await?;
                ensure!(
                    email["blobId"] == args.blob_id
                        || contains_blob(&email["bodyStructure"], &args.blob_id),
                    "Blob is not part of this message"
                );
                let bytes = self.jmap.download(&args.blob_id, "attachment").await?;
                let mut options = tokio::fs::OpenOptions::new();
                options.write(true).create_new(true).mode(0o600);
                let mut file = options
                    .open(&args.output_path)
                    .await
                    .context("Cannot create output file; existing files are never overwritten")?;
                file.write_all(&bytes).await?;
                file.sync_all().await?;
                Ok(json!({"path":args.output_path,"bytes":bytes.len()}))
            }
            "modify_mail" => {
                let args: Modify = parse(input)?;
                validate_ids(&args.email_ids)?;
                ensure!(!args.if_in_state.is_empty(), "if_in_state is required");
                let mut patch = json!({});
                if let Some(read) = args.read {
                    patch["keywords/$seen"] = if read { json!(true) } else { Value::Null };
                }
                if let Some(flagged) = args.flagged {
                    patch["keywords/$flagged"] = if flagged { json!(true) } else { Value::Null };
                }
                for id in args.add_mailbox_ids {
                    patch[format!("mailboxIds/{}", pointer(&id))] = json!(true);
                }
                for id in args.remove_mailbox_ids {
                    patch[format!("mailboxIds/{}", pointer(&id))] = Value::Null;
                }
                ensure!(
                    patch.as_object().is_some_and(|p| !p.is_empty()),
                    "No changes supplied"
                );
                let update: serde_json::Map<_, _> = args
                    .email_ids
                    .into_iter()
                    .map(|id| (id, patch.clone()))
                    .collect();
                self.jmap
                    .call(
                        "Email/set",
                        json!({"ifInState":args.if_in_state,"update":update}),
                    )
                    .await
            }
            "move_mail" => {
                let args: Move = parse(input)?;
                validate_ids(&args.email_ids)?;
                ensure!(
                    ["inbox", "archive", "trash", "junk"].contains(&args.destination_role.as_str()),
                    "destination_role must be inbox, archive, trash, or junk"
                );
                let destination = if args.destination_role == "archive" {
                    self.jmap.archive_mailbox().await?
                } else {
                    self.jmap.role(&args.destination_role).await?
                };
                let current = self
                    .jmap
                    .call(
                        "Email/get",
                        json!({"ids":args.email_ids,"properties":["id","mailboxIds","keywords"]}),
                    )
                    .await?;
                ensure!(
                    current["state"] == args.if_in_state
                        && current["notFound"].as_array().is_none_or(|a| a.is_empty()),
                    "Messages changed or disappeared; refresh before moving"
                );
                let inbox = if args.destination_role == "archive" {
                    Some(self.jmap.role("inbox").await?)
                } else {
                    None
                };
                let mut update = json!({});
                for email in current["list"].as_array().context("Missing emails")? {
                    let id = email["id"].as_str().context("Missing email id")?;
                    if let Some(inbox) = &inbox {
                        update[id] = json!({format!("mailboxIds/{}",pointer(&destination)):true,format!("mailboxIds/{}",pointer(inbox)):null});
                    } else {
                        update[id] = json!({"mailboxIds":{&destination:true}});
                    }
                }
                self.jmap
                    .call(
                        "Email/set",
                        json!({"ifInState":args.if_in_state,"update":update}),
                    )
                    .await
            }
            "save_mailbox" => {
                let args: MailboxChange = parse(input)?;
                ensure!(
                    !args.name.trim().is_empty() && !args.name.contains('\0'),
                    "Mailbox name is empty or invalid"
                );
                if let Some(id) = args.mailbox_id {
                    let state = args
                        .if_in_state
                        .context("Renaming/reparenting requires if_in_state from list_mailboxes")?;
                    self.jmap.call("Mailbox/set", json!({"ifInState":state,"update":{id:{"name":args.name,"parentId":args.parent_id}}})).await
                } else {
                    self.jmap.call("Mailbox/set", json!({"create":{"mailbox":{"name":args.name,"parentId":args.parent_id}}})).await
                }
            }
            "prepare_send" => {
                let args: PrepareSend = parse(input)?;
                let email = self.jmap.get_email(&args.email_id).await?;
                self.validate_send(&email, &args.identity_id).await?;
                let scope = self.scope().await?;
                let mut intent = self
                    .store
                    .prepare(&scope, preview(&email, &args.identity_id)?)?;
                intent["send_mode"] = json!(self.store.mode(&scope)?);
                intent["confirmation_notice"] = json!(
                    "Show From, To, Cc, Bcc, subject, complete body and all attachments to the user. In manual mode obtain approval before send_draft. Muse may independently require its native approval; no card-rendering API is assumed."
                );
                Ok(intent)
            }
            "send_draft" => self.send(parse(input)?).await,
            "send_status" => {
                let args: IntentId = parse(input)?;
                let scope = self.scope().await?;
                let mut intent = self.store.get(&scope, &args.intent_id)?;
                let email = &intent["preview"]["id"];
                let query = self
                    .jmap
                    .call(
                        "EmailSubmission/query",
                        json!({"filter":{"emailIds":[email]},"limit":100}),
                    )
                    .await?;
                let submissions = self
                    .jmap
                    .call("EmailSubmission/get", json!({"ids":query["ids"]}))
                    .await?;
                intent["submissions"] = submissions["list"].clone();
                intent["note"] = json!(
                    "Submitted means accepted for delivery, not delivered. Missing records do not prove a timed-out send failed. Never automatically resubmit uncertain/submitting intents."
                );
                Ok(intent)
            }
            "cancel_send" => {
                let args: IntentId = parse(input)?;
                self.store.cancel(&self.scope().await?, &args.intent_id)
            }
            "set_send_mode" => {
                let args: Policy = parse(input)?;
                self.store.set_mode(&self.scope().await?, args.mode)?;
                Ok(
                    json!({"mode":args.mode,"note":"Changes this connector's workflow only. Muse permission requirements still apply."}),
                )
            }
            "audit_log" => {
                let _: Empty = parse(input)?;
                self.store.events(&self.scope().await?)
            }
            _ => bail!("Unknown tool"),
        }
    }
    async fn validate_send(&self, email: &Value, identity: &str) -> Result<()> {
        ensure!(
            email["keywords"]["$draft"] == true,
            "Message is not a draft"
        );
        let from = self.identity(identity).await?;
        let senders = addresses(email, "from");
        ensure!(
            senders.len() == 1 && senders[0].email.eq_ignore_ascii_case(&from.email),
            "Selected identity does not match draft From"
        );
        let recipients: Vec<_> = ["to", "cc", "bcc"]
            .iter()
            .flat_map(|key| addresses(email, key))
            .collect();
        ensure!(!recipients.is_empty(), "Draft has no recipients");
        for recipient in recipients {
            recipient.validate()?;
        }
        full_body(email)?;
        Ok(())
    }
    async fn send(&self, args: Send) -> Result<Value> {
        let scope = self.scope().await?;
        let intent = self.store.get(&scope, &args.intent_id)?;
        if intent["status"] != "prepared" {
            return Ok(intent);
        }
        let email_id = intent["preview"]["id"]
            .as_str()
            .context("Missing email id")?;
        let identity_id = intent["preview"]["identityId"]
            .as_str()
            .context("Missing identity")?;
        let email = self.jmap.get_email(email_id).await?;
        self.validate_send(&email, identity_id).await?;
        ensure!(
            hash(&preview(&email, identity_id)?)
                == intent["preview_hash"].as_str().unwrap_or_default(),
            "Draft no longer matches approved preview"
        );
        let existing = self
            .jmap
            .call(
                "EmailSubmission/query",
                json!({"filter":{"emailIds":[email_id]},"limit":1}),
            )
            .await?;
        ensure!(
            existing["ids"].as_array().is_some_and(|ids| ids.is_empty()),
            "Draft already has a submission; inspect send_status"
        );
        let drafts = self.jmap.role("drafts").await?;
        let sent = self.jmap.role("sent").await?;
        // Durable claim happens BEFORE the only submission call. No automatic retries.
        if !self.store.claim(
            &scope,
            &args.intent_id,
            args.confirmed,
            args.preview_hash.as_deref(),
        )? {
            return self.store.get(&scope, &args.intent_id);
        }
        let result = self.jmap.call("EmailSubmission/set",json!({"create":{"send":{"identityId":identity_id,"emailId":email_id}},"onSuccessUpdateEmail":{"#send":{"keywords/$draft":null,"keywords/$seen":true,format!("mailboxIds/{}",pointer(&drafts)):null,format!("mailboxIds/{}",pointer(&sent)):true}}})).await;
        match result {
            Ok(result) => {
                if let Some(id) = result["created"]["send"]["id"].as_str() {
                    self.store.finish(&scope,&args.intent_id,"submitted",json!({"submission_id":id,"email_id":email_id,"note":"Accepted for delivery; inspect send_status and mailbox state. Submission success does not guarantee delivery or Sent-folder update success."}))?;
                } else if result["notCreated"]["send"].is_object() {
                    self.store.finish(&scope,&args.intent_id,"rejected",json!({"error":result["notCreated"]["send"]["type"]}))?;
                } else { self.store.finish(&scope,&args.intent_id,"uncertain",json!({"error":"No definitive submission result"}))?; }
            }
            Err(_) => self.store.finish(&scope,&args.intent_id,"uncertain",json!({"error":"Submission outcome unknown; inspect send_status. Do not retry by making another draft."}))?,
        }
        let mut outcome = self.store.get(&scope, &args.intent_id)?;
        if outcome["status"] == "submitted"
            && let Ok(Some(source_id)) = self.store.reply_source(&scope, email_id)
        {
            match self
                .jmap
                .call(
                    "Email/set",
                    json!({"update":{source_id:{"keywords/$answered":true}}}),
                )
                .await
            {
                Ok(result) if !has_partial_failure(&result) => (),
                _ => {
                    outcome["warning"] = json!(
                        "Message submitted, but the original message could not be marked answered. Do not resend."
                    )
                }
            }
        }
        Ok(outcome)
    }
}
fn contains_blob(part: &Value, blob: &str) -> bool {
    part["blobId"] == blob
        || part["subParts"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| contains_blob(p, blob)))
}
fn validate_ids(ids: &[String]) -> Result<()> {
    ensure!(
        !ids.is_empty() && ids.len() <= 100 && ids.iter().all(|id| !id.is_empty()),
        "Provide 1..100 nonempty email ids"
    );
    Ok(())
}

pub fn has_partial_failure(value: &Value) -> bool {
    ["notCreated", "notUpdated", "notDestroyed"]
        .iter()
        .any(|key| value[*key].as_object().is_some_and(|map| !map.is_empty()))
}
