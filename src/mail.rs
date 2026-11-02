use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Address {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
}
impl Address {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self
                .email
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || "<>,;\"".contains(c))
                && self.email.matches('@').count() == 1
                && !self.email.starts_with('@')
                && !self.email.ends_with('@'),
            "Use a bare email address and a separate optional display name"
        );
        ensure!(
            self.name
                .as_ref()
                .is_none_or(|s| !s.contains(['\r', '\n', '\0'])),
            "Display name contains a header control character"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub blob_id: String,
    pub name: String,
    pub content_type: String,
    #[serde(default)]
    pub cid: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub identity_id: String,
    #[serde(default)]
    pub to: Vec<Address>,
    #[serde(default)]
    pub cc: Vec<Address>,
    #[serde(default)]
    pub bcc: Vec<Address>,
    #[serde(default)]
    pub reply_to: Option<Vec<Address>>,
    pub subject: String,
    pub text: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub email_id: String,
    pub identity_id: String,
    pub text: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub reply_all: bool,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub email_id: String,
    pub draft: Draft,
    #[serde(default = "yes")]
    pub include_attachments: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmailId {
    pub email_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadId {
    pub thread_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceDraft {
    pub email_id: String,
    pub draft: Draft,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Search {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub mailbox_id: Option<String>,
    #[serde(default)]
    pub unread: Option<bool>,
    #[serde(default)]
    pub has_attachment: Option<bool>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub position: u64,
    #[serde(default = "page_size")]
    pub limit: u64,
}
fn page_size() -> u64 {
    25
}
impl Search {
    pub fn args(&self) -> Result<Value> {
        ensure!((1..=100).contains(&self.limit), "limit must be 1..100");
        let mut filter = json!({});
        for (key, value) in [
            ("text", &self.text),
            ("from", &self.from),
            ("to", &self.to),
            ("subject", &self.subject),
            ("inMailbox", &self.mailbox_id),
            ("before", &self.before),
            ("after", &self.after),
        ] {
            if let Some(value) = value {
                filter[key] = json!(value);
            }
        }
        if let Some(unread) = self.unread {
            filter[if unread { "notKeyword" } else { "hasKeyword" }] = json!("$seen");
        }
        if let Some(has) = self.has_attachment {
            filter["hasAttachment"] = json!(has);
        }
        Ok(
            json!({"filter":filter,"position":self.position,"limit":self.limit,"sort":[{"property":"receivedAt","isAscending":false}],"calculateTotal":true}),
        )
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub path: String,
    pub content_type: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Download {
    pub email_id: String,
    pub blob_id: String,
    pub output_path: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Modify {
    pub email_ids: Vec<String>,
    #[serde(default)]
    pub read: Option<bool>,
    #[serde(default)]
    pub flagged: Option<bool>,
    #[serde(default)]
    pub add_mailbox_ids: Vec<String>,
    #[serde(default)]
    pub remove_mailbox_ids: Vec<String>,
    /// Fail rather than overwrite if the account's Email state changed.
    pub if_in_state: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub email_ids: Vec<String>,
    pub destination_role: String,
    pub if_in_state: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailboxChange {
    #[serde(default)]
    pub mailbox_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub if_in_state: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareSend {
    pub email_id: String,
    pub identity_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub intent_id: String,
    /// Required in manual mode. Must match the preview shown to the user.
    #[serde(default)]
    pub preview_hash: Option<String>,
    /// Host/agent confirmation, NOT cryptographic evidence of a human click.
    #[serde(default)]
    pub confirmed: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntentId {
    pub intent_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub mode: Mode,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Manual,
    Autonomous,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

pub fn addresses(email: &Value, field: &str) -> Vec<Address> {
    serde_json::from_value(email[field].clone()).unwrap_or_default()
}
pub fn reply_draft(source: &Value, input: Reply, own: &[Address]) -> Result<(Draft, Value)> {
    let mut seen: HashSet<String> = own.iter().map(|a| a.email.to_lowercase()).collect();
    let reply_to = addresses(source, "replyTo");
    let primary = if reply_to.is_empty() {
        addresses(source, "from")
    } else {
        reply_to
    };
    let mut to = Vec::new();
    let mut cc = Vec::new();
    for addr in primary {
        if seen.insert(addr.email.to_lowercase()) {
            to.push(addr);
        }
    }
    if input.reply_all {
        for addr in addresses(source, "to") {
            if seen.insert(addr.email.to_lowercase()) {
                to.push(addr);
            }
        }
        for addr in addresses(source, "cc") {
            if seen.insert(addr.email.to_lowercase()) {
                cc.push(addr);
            }
        }
    }
    ensure!(
        !to.is_empty() || !cc.is_empty(),
        "No reply recipients remain after excluding your identities; compose explicitly"
    );
    let subject = source["subject"].as_str().unwrap_or_default();
    let subject = if subject.to_ascii_lowercase().starts_with("re:") {
        subject.to_owned()
    } else {
        format!("Re: {subject}")
    };
    let mut refs: Vec<String> =
        serde_json::from_value(source["references"].clone()).unwrap_or_default();
    if refs.is_empty() {
        refs = serde_json::from_value(source["inReplyTo"].clone()).unwrap_or_default();
    }
    let message_ids: Vec<String> =
        serde_json::from_value(source["messageId"].clone()).unwrap_or_default();
    for id in &message_ids {
        if !refs.contains(id) {
            refs.push(id.clone());
        }
    }
    let mut headers = json!({});
    if !message_ids.is_empty() {
        headers["inReplyTo"] = json!(message_ids);
    }
    if !refs.is_empty() {
        headers["references"] = json!(refs);
    }
    Ok((
        Draft {
            identity_id: input.identity_id,
            to,
            cc,
            bcc: vec![],
            reply_to: None,
            subject,
            text: input.text,
            html: input.html,
            attachments: input.attachments,
        },
        headers,
    ))
}
pub fn build_email(draft: &Draft, from: Address, mailbox: &str, headers: Value) -> Result<Value> {
    from.validate()?;
    for addr in draft
        .to
        .iter()
        .chain(&draft.cc)
        .chain(&draft.bcc)
        .chain(draft.reply_to.iter().flatten())
    {
        addr.validate()?;
    }
    ensure!(
        !draft.subject.contains(['\r', '\n', '\0']),
        "Subject contains a header control character"
    );
    ensure!(
        draft.text.len() <= 1024 * 1024
            && draft.html.as_ref().is_none_or(|s| s.len() <= 1024 * 1024),
        "Body exceeds 1 MiB per format"
    );
    ensure!(draft.attachments.len() <= 100, "Too many attachments");
    let mut body_values = json!({"text":{"value":draft.text}});
    let mut body = json!({"partId":"text","type":"text/plain"});
    if let Some(html) = &draft.html {
        body_values["html"] = json!({"value":html});
        body = json!({"type":"multipart/alternative","subParts":[body,{"partId":"html","type":"text/html"}]});
    }
    let mut inline = Vec::new();
    let mut files = Vec::new();
    let mut cids = HashSet::new();
    for a in &draft.attachments {
        ensure!(
            !a.blob_id.is_empty() && !a.name.is_empty() && !a.name.contains(['\r', '\n', '\0']),
            "Invalid attachment metadata"
        );
        validate_mime(&a.content_type)?;
        let mut part = json!({"blobId":a.blob_id,"name":a.name,"type":a.content_type,"disposition":"attachment"});
        if let Some(cid) = &a.cid {
            ensure!(
                draft.html.is_some()
                    && !cid.is_empty()
                    && !cid
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || "<>".contains(c))
                    && cids.insert(cid),
                "Inline CID requires HTML and a unique bare content ID"
            );
            part["cid"] = json!(cid);
            part["disposition"] = json!("inline");
            inline.push(part);
        } else {
            files.push(part);
        }
    }
    if !inline.is_empty() {
        inline.insert(0, body);
        body = json!({"type":"multipart/related","subParts":inline});
    }
    if !files.is_empty() {
        files.insert(0, body);
        body = json!({"type":"multipart/mixed","subParts":files});
    }
    let mut email = json!({"mailboxIds":{mailbox:true},"keywords":{"$draft":true,"$seen":true},"from":[from],"to":draft.to,"cc":draft.cc,"bcc":draft.bcc,"subject":draft.subject,"messageId":[format!("{}@jmap-sdk.local", uuid::Uuid::new_v4())],"bodyStructure":body,"bodyValues":body_values});
    if let Some(reply_to) = &draft.reply_to {
        email["replyTo"] = json!(reply_to);
    }
    for key in ["inReplyTo", "references"] {
        if let Some(value) = headers.get(key) {
            email[key] = value.clone();
        }
    }
    Ok(email)
}
pub fn validate_mime(mime: &str) -> Result<()> {
    ensure!(
        mime.contains('/')
            && !mime.chars().any(|c| c.is_control() || c.is_whitespace())
            && !mime.starts_with("multipart/"),
        "Use a valid non-multipart MIME type"
    );
    Ok(())
}
pub fn full_body(email: &Value) -> Result<()> {
    let values = email["bodyValues"]
        .as_object()
        .context("Message body values are missing")?;
    ensure!(
        values
            .values()
            .all(|v| v["isTruncated"] != true && v["isEncodingProblem"] != true),
        "Body is truncated or has an encoding problem; cannot prepare a complete send preview"
    );
    for key in ["textBody", "htmlBody"] {
        if let Some(parts) = email[key].as_array() {
            for part in parts {
                ensure!(
                    part["partId"]
                        .as_str()
                        .is_some_and(|id| values.get(id).is_some_and(|v| v["value"].is_string())),
                    "Body part missing from preview"
                );
            }
        }
    }
    Ok(())
}
pub fn preview(email: &Value, identity_id: &str) -> Result<Value> {
    full_body(email)?;
    let mut result = json!({"identityId":identity_id});
    for field in [
        "id",
        "blobId",
        "from",
        "to",
        "cc",
        "bcc",
        "replyTo",
        "subject",
        "messageId",
        "inReplyTo",
        "references",
        "textBody",
        "htmlBody",
        "bodyValues",
        "attachments",
        "bodyStructure",
    ] {
        result[field] = email[field].clone();
    }
    Ok(result)
}

/// Human-readable address list, e.g. `Ann <ann@example.com>, bob@example.com`.
pub fn format_addresses(list: &[Address]) -> String {
    list.iter()
        .map(
            |a| match a.name.as_deref().filter(|n| !n.trim().is_empty()) {
                Some(name) => format!("{name} <{}>", a.email),
                None => a.email.clone(),
            },
        )
        .collect::<Vec<_>>()
        .join(", ")
}
pub struct ForwardQuote {
    pub text: String,
    pub has_text: bool,
}
/// Conventional forwarded-message header block followed by the original plain text.
pub fn forward_quote(source: &Value) -> ForwardQuote {
    let body = source["textBody"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| source["bodyValues"][p["partId"].as_str()?]["value"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut text = String::from("\n\n---------- Forwarded message ----------\n");
    for (label, field) in [("From", "from"), ("To", "to"), ("Cc", "cc")] {
        let list = addresses(source, field);
        if !list.is_empty() {
            text.push_str(&format!("{label}: {}\n", format_addresses(&list)));
        }
        if field == "from"
            && let Some(date) = source["sentAt"]
                .as_str()
                .or_else(|| source["receivedAt"].as_str())
        {
            text.push_str(&format!("Date: {date}\n"));
        }
        if field == "from" {
            text.push_str(&format!(
                "Subject: {}\n",
                source["subject"].as_str().unwrap_or_default()
            ));
        }
    }
    text.push('\n');
    text.push_str(&body);
    ForwardQuote {
        text,
        has_text: !body.is_empty(),
    }
}
