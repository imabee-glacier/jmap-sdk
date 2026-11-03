use jmap_sdk::{
    jmap::pointer,
    mail::*,
    store::{Store, hash},
};
use serde_json::{Value, json};
fn address(s: &str) -> Address {
    Address {
        email: s.into(),
        name: None,
    }
}
fn draft() -> Draft {
    serde_json::from_value(json!({"identity_id":"identity","to":[{"email":"friend@example.com"}],"cc":[],"bcc":[{"email":"private@example.com"}],"subject":"Hi","text":"Hello","html":"<p>Hello<img src=\"cid:logo\"></p>","attachments":[{"blob_id":"img","name":"logo.png","content_type":"image/png","cid":"logo"},{"blob_id":"pdf","name":"résumé.pdf","content_type":"application/pdf"}]})).expect("draft")
}
fn reply(all: bool) -> Reply {
    Reply {
        email_id: "e".into(),
        identity_id: "i".into(),
        text: "Reply".into(),
        html: None,
        reply_all: all,
        quote: false,
        attachments: vec![],
    }
}
#[test]
fn reply_all_honors_reply_to_excludes_self_bcc_and_duplicates() {
    let source = json!({"from":[{"email":"sender@example.com"}],"replyTo":[{"email":"reply@example.com"}],"to":[{"email":"me@example.com"},{"email":"other@example.com"},{"email":"reply@example.com"}],"cc":[{"email":"other@example.com"},{"email":"cc@example.com"},{"email":"alias@example.com"}],"bcc":[{"email":"hidden@example.com"}],"subject":"RE: Existing","messageId":["latest@example.com"],"references":["root@example.com"]});
    let (draft, headers) = reply_draft(
        &source,
        reply(true),
        &[address("me@example.com"), address("alias@example.com")],
    )
    .expect("reply");
    assert_eq!(
        draft.to,
        vec![address("reply@example.com"), address("other@example.com")]
    );
    assert_eq!(draft.cc, vec![address("cc@example.com")]);
    assert!(draft.bcc.is_empty());
    assert_eq!(draft.subject, "RE: Existing");
    assert_eq!(headers["inReplyTo"], json!(["latest@example.com"]));
    assert_eq!(
        headers["references"],
        json!(["root@example.com", "latest@example.com"])
    );
}
#[test]
fn reply_single_uses_from_and_does_not_invent_message_ids() {
    let (draft,headers)=reply_draft(&json!({"from":[{"email":"sender@example.com"}],"to":[{"email":"other@example.com"}],"subject":"Question"}),reply(false),&[]).expect("reply");
    assert_eq!(draft.to, vec![address("sender@example.com")]);
    assert!(draft.cc.is_empty());
    assert_eq!(draft.subject, "Re: Question");
    assert!(headers.get("inReplyTo").is_none());
}
#[test]
fn mime_tree_preserves_alternative_inline_and_regular_attachments() {
    let email =
        build_email(&draft(), address("me@example.com"), "drafts", json!({})).expect("email");
    let body = &email["bodyStructure"];
    assert_eq!(body["type"], "multipart/mixed");
    assert_eq!(body["subParts"][0]["type"], "multipart/related");
    assert_eq!(
        body["subParts"][0]["subParts"][0]["type"],
        "multipart/alternative"
    );
    assert_eq!(body["subParts"][0]["subParts"][1]["cid"], "logo");
    assert_eq!(body["subParts"][1]["blobId"], "pdf");
    assert_eq!(email["bcc"][0]["email"], "private@example.com");
    assert_eq!(email["bodyValues"]["text"]["value"], "Hello");
}
#[test]
fn reject_header_injection_and_duplicate_inline_cids() {
    let mut d = draft();
    d.subject = "Hi\r\nBcc: victim@example.com".into();
    assert!(build_email(&d, address("me@example.com"), "d", json!({})).is_err());
    let mut d = draft();
    d.attachments.push(d.attachments[0].clone());
    assert!(build_email(&d, address("me@example.com"), "d", json!({})).is_err());
    assert!(address("bad@example.com\r\n").validate().is_err());
}
#[test]
fn truncated_or_missing_body_cannot_be_previewed() {
    assert!(
        preview(
            &json!({"bodyValues":{"text":{"value":"partial","isTruncated":true}}}),
            "i"
        )
        .is_err()
    );
    assert!(
        preview(
            &json!({"bodyValues":{},"textBody":[{"partId":"missing"}]}),
            "i"
        )
        .is_err()
    );
}
fn snap() -> Value {
    json!({"id":"email","identityId":"identity","subject":"hello","to":["friend@example.com"]})
}
#[test]
fn manual_requires_confirmation_and_matching_snapshot() {
    let db = Store::memory().expect("db");
    let intent = db.prepare("a", snap()).expect("prepare");
    let id = intent["intent_id"].as_str().expect("id");
    let digest = intent["preview_hash"].as_str().expect("hash");
    assert!(db.claim("a", id, false, Some(digest)).is_err());
    assert!(db.claim("a", id, true, Some("wrong")).is_err());
    assert!(db.claim("a", id, true, Some(digest)).expect("claim"));
    assert!(!db.claim("a", id, true, Some(digest)).expect("duplicate"));
}
#[test]
fn repeated_prepare_is_idempotent_and_scoped() {
    let db = Store::memory().expect("db");
    let a = db.prepare("a", snap()).expect("a");
    let b = db.prepare("a", snap()).expect("b");
    assert_eq!(a["intent_id"], b["intent_id"]);
    assert!(db.get("b", a["intent_id"].as_str().expect("id")).is_err());
    let mut changed = snap();
    changed["subject"] = json!("changed");
    assert!(db.prepare("a", changed).is_err());
}
#[test]
fn cancel_prevents_send_and_autonomy_is_account_scoped() {
    let db = Store::memory().expect("db");
    db.set_mode("a", Mode::Autonomous).expect("mode");
    assert_eq!(db.mode("b").expect("mode"), Mode::Manual);
    let a = db.prepare("a", snap()).expect("prepare");
    let id = a["intent_id"].as_str().expect("id");
    db.cancel("a", id).expect("cancel");
    assert!(!db.claim("a", id, false, None).expect("claim"));
}
#[test]
fn send_claim_survives_restart_and_uncertainty_is_terminal() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("state.sqlite");
    let id;
    {
        let db = Store::open(&path).expect("db");
        db.set_mode("a", Mode::Autonomous).expect("mode");
        let intent = db.prepare("a", snap()).expect("prepare");
        id = intent["intent_id"].as_str().expect("id").to_owned();
        assert!(db.claim("a", &id, false, None).expect("claim"));
    }
    let db = Store::open(&path).expect("reopen");
    assert!(!db.claim("a", &id, false, None).expect("claim"));
    db.finish("a", &id, "uncertain", json!({})).expect("finish");
    assert!(!db.claim("a", &id, false, None).expect("claim"));
    assert_eq!(
        db.prepare("a", snap()).expect("reprepare")["status"],
        "uncertain"
    );
}
#[test]
fn separate_process_connections_cannot_claim_same_send() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("state.sqlite");
    let a = Store::open(&path).expect("a");
    let b = Store::open(&path).expect("b");
    a.set_mode("a", Mode::Autonomous).expect("mode");
    let intent = a.prepare("a", snap()).expect("prepare");
    let id = intent["intent_id"].as_str().expect("id");
    assert!(a.claim("a", id, false, None).expect("a claim"));
    assert!(!b.claim("a", id, false, None).expect("b claim"));
}
#[test]
fn expiration_blocks_claim() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("state.sqlite");
    let db = Store::open(&path).expect("db");
    db.set_mode("a", Mode::Autonomous).expect("mode");
    let intent = db.prepare("a", snap()).expect("prepare");
    rusqlite::Connection::open(&path)
        .expect("raw")
        .execute("UPDATE intents SET expires=0", [])
        .expect("expire");
    assert!(
        db.claim("a", intent["intent_id"].as_str().expect("id"), false, None)
            .is_err()
    );
}
#[test]
fn hashes_include_bcc_and_patch_paths_escape_ids() {
    let a = json!({"bcc":["one@example.com"]});
    let b = json!({"bcc":["two@example.com"]});
    assert_ne!(hash(&a), hash(&b));
    assert_eq!(pointer("box/a~b"), "box~1a~0b");
}
#[test]
fn schemas_deserialize_and_send_is_sensitive() {
    for definition in jmap_sdk::transport::definitions() {
        let _: rmcp::model::Tool = serde_json::from_value(definition.clone()).expect("MCP tool");
        if definition["name"] == "send_draft" {
            assert_eq!(definition["annotations"]["readOnlyHint"], false);
            assert_eq!(definition["annotations"]["destructiveHint"], true);
        }
    }
}
#[test]
fn cancelled_draft_can_be_prepared_again_with_fresh_approval() {
    let db = Store::memory().expect("db");
    let first = db.prepare("a", snap()).expect("prepare");
    let id = first["intent_id"].as_str().expect("id");
    db.cancel("a", id).expect("cancel");
    let again = db.prepare("a", snap()).expect("re-prepare");
    assert_eq!(again["status"], "prepared");
    let digest = again["preview_hash"].as_str().expect("hash");
    assert!(
        db.claim(
            "a",
            again["intent_id"].as_str().expect("id"),
            true,
            Some(digest)
        )
        .expect("claim")
    );
    // Once submitting, the same draft can never be prepared into a second send.
    let after = db.prepare("a", snap()).expect("prepare after claim");
    assert_eq!(after["status"], "submitting");
}
#[test]
fn forward_quote_is_human_readable() {
    let source = json!({"from":[{"email":"ann@example.com","name":"Ann"}],"to":[{"email":"me@example.com","name":null}],"cc":[],"subject":"Plans","sentAt":"2026-11-01T10:00:00Z","textBody":[{"partId":"1"}],"bodyValues":{"1":{"value":"See you"}}});
    let quote = forward_quote(&source);
    assert!(quote.has_text);
    assert!(quote.text.contains("From: Ann <ann@example.com>\nDate: 2026-11-01T10:00:00Z\nSubject: Plans\nTo: me@example.com\n\nSee you"));
    assert!(!quote.text.contains("Cc:") && !quote.text.contains('"') && !quote.text.contains('['));
    let html_only =
        json!({"from":[{"email":"ann@example.com"}],"subject":"x","textBody":[],"bodyValues":{}});
    assert!(!forward_quote(&html_only).has_text);
}
fn quoted_source() -> Value {
    json!({"from":[{"email":"ann@example.com","name":"Ann"}],"to":[{"email":"me@example.com"}],"subject":"Plans","sentAt":"2026-11-01T10:00:00Z","messageId":["m@example.com"],"textBody":[{"partId":"1","type":"text/plain"}],"bodyValues":{"1":{"value":"See you <soon> & bring tea\r\n\r\nAnn"}}})
}
fn quoting(all: bool) -> Reply {
    Reply {
        quote: true,
        ..reply(all)
    }
}
#[test]
fn reply_quotes_the_original_with_attribution_and_keeps_threading() {
    let (draft, headers) = reply_draft(&quoted_source(), quoting(false), &[]).expect("reply");
    assert_eq!(
        draft.text,
        "Reply\n\nOn 2026-11-01T10:00:00Z, Ann <ann@example.com> wrote:\n> See you <soon> & bring tea\n>\n> Ann"
    );
    assert!(draft.html.is_none());
    assert_eq!(headers["inReplyTo"], json!(["m@example.com"]));
}
#[test]
fn html_reply_gets_an_escaped_blockquote() {
    let mut input = quoting(false);
    input.html = Some("<p>Reply</p>".into());
    let (draft, _) = reply_draft(&quoted_source(), input, &[]).expect("reply");
    let html = draft.html.expect("html");
    assert!(html.starts_with("<p>Reply</p>"));
    assert!(html.contains("<blockquote"));
    assert!(html.contains("See you &lt;soon&gt; &amp; bring tea<br><br>Ann"));
    assert!(!html.contains("<soon>"));
}
#[test]
fn html_only_original_is_quoted_as_text_without_markup_or_scripts() {
    let source = json!({"from":[{"email":"ann@example.com"}],"subject":"x","textBody":[{"partId":"1","type":"text/html"}],"bodyValues":{"1":{"value":"<style>p{color:red}</style><p>Hello&nbsp;<b>there</b></p><script>alert(1)</script><div>Bye &amp; thanks</div>"}}});
    let (draft, _) = reply_draft(&source, quoting(false), &[]).expect("reply");
    assert!(
        draft.text.ends_with("> Hello there\n>\n> Bye & thanks"),
        "{}",
        draft.text
    );
    assert!(
        !draft.text.contains("alert") && !draft.text.contains("color") && !draft.text.contains('<')
    );
}
#[test]
fn quoting_refuses_a_truncated_original_but_quote_false_still_works() {
    let source = json!({"from":[{"email":"ann@example.com"}],"subject":"x","textBody":[{"partId":"1","type":"text/plain"}],"bodyValues":{"1":{"value":"part","isTruncated":true}}});
    assert!(reply_draft(&source, quoting(false), &[]).is_err());
    let (draft, _) = reply_draft(&source, reply(false), &[]).expect("no quote");
    assert_eq!(draft.text, "Reply");
}
#[test]
fn empty_original_adds_no_quote_block() {
    let source =
        json!({"from":[{"email":"ann@example.com"}],"subject":"x","textBody":[],"bodyValues":{}});
    let (draft, _) = reply_draft(&source, quoting(false), &[]).expect("reply");
    assert_eq!(draft.text, "Reply");
}
#[test]
fn reply_quotes_by_default_and_schema_says_so() {
    let input: Reply = serde_json::from_value(json!({"email_id":"e","identity_id":"i","text":"t"}))
        .expect("reply");
    assert!(input.quote);
}
