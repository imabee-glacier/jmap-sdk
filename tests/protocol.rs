use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use jmap_sdk::{
    config::Config,
    jmap::{CORE, Jmap, MAIL, SUBMISSION},
    service::Service,
    store::Store,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Fake {
    url: String,
    email: Arc<Mutex<Value>>,
    calls: Arc<Mutex<Vec<Value>>>,
    submissions: Arc<AtomicUsize>,
    fail_submission: bool,
}
struct Fixture {
    service: Service,
    fake: Fake,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn session(State(f): State<Fake>) -> Json<Value> {
    Json(
        json!({"capabilities":{CORE:{"maxSizeUpload":26214400},MAIL:{},SUBMISSION:{}},"accounts":{"a":{"accountCapabilities":{MAIL:{},SUBMISSION:{}}}},"primaryAccounts":{MAIL:"a"},"apiUrl":format!("{}/api",f.url),"uploadUrl":format!("{}/upload/{{accountId}}",f.url),"downloadUrl":format!("{}/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}",f.url)}),
    )
}
async fn api(State(f): State<Fake>, Json(request): Json<Value>) -> axum::response::Response {
    f.calls.lock().expect("calls").push(request.clone());
    let call = &request["methodCalls"][0];
    let method = call[0].as_str().expect("method");
    let args = &call[1];
    assert_eq!(args["accountId"], "a");
    let result = match method {
        "Email/get" => {
            json!({"list":[f.email.lock().expect("email").clone()],"notFound":[],"state":"s1"})
        }
        "Identity/get" => json!({"list":[{"id":"i","email":"me@example.com","name":"Me"}]}),
        "Mailbox/get" => {
            json!({"list":[{"id":"d/a~b","role":"drafts"},{"id":"s","role":"sent"},{"id":"in","role":"inbox"},{"id":"arc","role":"archive"}],"state":"m1"})
        }
        "Email/query" => {
            json!({"ids":["e"],"queryState":"q1","position":args["position"],"total":1})
        }
        "Email/set" => {
            if let Some(draft) = args["create"].get("draft") {
                assert!(draft.get("bodyStructure").is_some());
                let mut email = draft.clone();
                email["id"] = json!("new");
                email["blobId"] = json!("new-blob");
                email["textBody"] = json!([{"partId":"text","type":"text/plain"}]);
                email["htmlBody"] = json!([]);
                email["attachments"] = json!([]);
                *f.email.lock().expect("email") = email;
                json!({"created":{"draft":{"id":"new"}},"newState":"s2"})
            } else {
                json!({"updated":{"e":null},"notUpdated":{},"newState":"s2"})
            }
        }
        "EmailSubmission/query" => {
            json!({"ids":if f.submissions.load(Ordering::SeqCst)>0 { vec!["sub"] } else { vec![] }})
        }
        "EmailSubmission/get" => {
            json!({"list":[{"id":"sub","emailId":"e","deliveryStatus":{"friend@example.com":{"delivered":"unknown"}}}]})
        }
        "EmailSubmission/set" => {
            f.submissions.fetch_add(1, Ordering::SeqCst);
            if f.fail_submission {
                return (StatusCode::BAD_GATEWAY, "upstream closed after acceptance")
                    .into_response();
            }
            assert_eq!(
                args["onSuccessUpdateEmail"]["#send"]["mailboxIds/d~1a~0b"],
                Value::Null
            );
            json!({"created":{"send":{"id":"sub"}}})
        }
        _ => panic!("Unexpected method {method}"),
    };
    Json(json!({"methodResponses":[[method,result,"0"]]})).into_response()
}
async fn fixture(fail_submission: bool) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let fake = Fake {
        url: url.clone(),
        email: Arc::new(Mutex::new(
            json!({"id":"e","blobId":"blob","keywords":{"$draft":true},"mailboxIds":{"d/a~b":true},"from":[{"email":"me@example.com"}],"to":[{"email":"friend@example.com"}],"cc":[],"bcc":[{"email":"hidden@example.com"}],"subject":"hello","textBody":[{"partId":"text","type":"text/plain"}],"htmlBody":[],"bodyValues":{"text":{"value":"hello","isTruncated":false,"isEncodingProblem":false}},"attachments":[{"blobId":"attachment","name":"a.bin"}],"bodyStructure":{"type":"multipart/mixed","subParts":[{"partId":"text","type":"text/plain"},{"blobId":"attachment","name":"a.bin","type":"application/octet-stream"}]}}),
        )),
        calls: Arc::new(Mutex::new(vec![])),
        submissions: Arc::new(AtomicUsize::new(0)),
        fail_submission,
    };
    let app=Router::new().route("/session",get(session)).route("/api",post(api))
        .route("/upload/a",post(|bytes:axum::body::Bytes|async move { Json(json!({"accountId":"a","blobId":"uploaded","type":"application/octet-stream","size":bytes.len()})) }))
        .route("/download/a/{blob}/{name}",get(||async { vec![0u8,255,1,2,3] })).with_state(fake.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    let dir = tempfile::tempdir().expect("temp");
    let config = Config {
        session_url: format!("{url}/session").parse().expect("URL"),
        username: Some("me@example.com".into()),
        password: Some("test-password".into()),
        bearer: None,
        account_id: None,
        database: dir.path().join("state.sqlite"),
        allow_http: true,
        max_blob_bytes: 25 * 1024 * 1024,
    };
    let service = Service {
        store: Arc::new(Store::open(&config.database).expect("store")),
        jmap: Jmap::new(config).expect("jmap"),
    };
    Fixture {
        service,
        fake,
        task,
        _dir: dir,
    }
}
async fn prepare(f: &Fixture) -> Value {
    f.service
        .execute("prepare_send", json!({"email_id":"e","identity_id":"i"}))
        .await
        .expect("prepare")
}
fn send_args(intent: &Value) -> Value {
    json!({"intent_id":intent["intent_id"],"confirmed":true,"preview_hash":intent["preview_hash"]})
}
#[tokio::test]
async fn manual_send_requires_confirmation_and_duplicate_call_does_not_send_twice() {
    let f = fixture(false).await;
    let intent = prepare(&f).await;
    assert_eq!(intent["preview"]["bcc"][0]["email"], "hidden@example.com");
    assert!(
        f.service
            .execute("send_draft", json!({"intent_id":intent["intent_id"]}))
            .await
            .is_err()
    );
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.service
            .execute("send_draft", send_args(&intent))
            .await
            .expect("send")["status"],
        "submitted"
    );
    assert_eq!(
        f.service
            .execute("send_draft", send_args(&intent))
            .await
            .expect("retry")["status"],
        "submitted"
    );
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn upstream_failure_after_submission_is_uncertain_and_never_retried() {
    let f = fixture(true).await;
    let intent = prepare(&f).await;
    assert_eq!(
        f.service
            .execute("send_draft", send_args(&intent))
            .await
            .expect("send")["status"],
        "uncertain"
    );
    f.service
        .execute("send_draft", send_args(&intent))
        .await
        .expect("retry");
    let status = f
        .service
        .execute("send_status", json!({"intent_id":intent["intent_id"]}))
        .await
        .expect("status");
    assert_eq!(status["submissions"][0]["id"], "sub");
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn changed_recipient_or_wrong_identity_blocks_send() {
    let f = fixture(false).await;
    let intent = prepare(&f).await;
    f.fake.email.lock().expect("email")["to"] = json!([{"email":"attacker@example.com"}]);
    assert!(
        f.service
            .execute("send_draft", send_args(&intent))
            .await
            .is_err()
    );
    assert!(
        f.service
            .execute(
                "prepare_send",
                json!({"email_id":"e","identity_id":"wrong"})
            )
            .await
            .is_err()
    );
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn concurrent_sends_have_one_submission() {
    let f = fixture(false).await;
    let intent = prepare(&f).await;
    let (a, b) = tokio::join!(
        f.service.execute("send_draft", send_args(&intent)),
        f.service.execute("send_draft", send_args(&intent))
    );
    assert!(a.is_ok() || b.is_ok());
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn autonomous_mode_skips_only_the_application_confirmation() {
    let f = fixture(false).await;
    f.service
        .execute("set_send_mode", json!({"mode":"autonomous"}))
        .await
        .expect("mode");
    let intent = prepare(&f).await;
    assert_eq!(
        f.service
            .execute("send_draft", json!({"intent_id":intent["intent_id"]}))
            .await
            .expect("send")["status"],
        "submitted"
    );
}
#[tokio::test]
async fn compose_does_not_submit_and_search_has_pagination() {
    let f = fixture(false).await;
    let draft=f.service.execute("create_draft",json!({"identity_id":"i","to":[{"email":"friend@example.com"}],"subject":"Hi","text":"Hello","attachments":[{"blob_id":"uploaded","name":"a.bin","content_type":"application/octet-stream"}]})).await.expect("draft");
    assert_eq!(draft["email_id"], "new");
    assert_eq!(f.fake.submissions.load(Ordering::SeqCst), 0);
    let results = f
        .service
        .execute(
            "search_mail",
            json!({"position":2,"limit":10,"unread":true}),
        )
        .await
        .expect("search");
    assert_eq!(results["next_position"], 3);
    let calls = f.fake.calls.lock().expect("calls");
    let query = calls
        .iter()
        .find(|c| c["methodCalls"][0][0] == "Email/query")
        .expect("query");
    assert_eq!(query["methodCalls"][0][1]["filter"]["notKeyword"], "$seen");
}
#[tokio::test]
async fn attachment_bytes_roundtrip_to_agent_files_without_overwriting() {
    let f = fixture(false).await;
    let input = f._dir.path().join("input.bin");
    let output = f._dir.path().join("output.bin");
    let bytes = vec![0u8, 255, 1, 2, 3];
    tokio::fs::write(&input, &bytes).await.expect("write");
    let uploaded = f
        .service
        .execute(
            "upload_attachment",
            json!({"path":input,"content_type":"application/octet-stream"}),
        )
        .await
        .expect("upload");
    assert_eq!(uploaded["size"], 5);
    let args = json!({"email_id":"e","blob_id":"attachment","output_path":output});
    f.service
        .execute("download_attachment", args.clone())
        .await
        .expect("download");
    assert_eq!(tokio::fs::read(&output).await.expect("read"), bytes);
    assert!(
        f.service
            .execute("download_attachment", args)
            .await
            .is_err()
    );
    assert!(f.service.execute("download_attachment",json!({"email_id":"e","blob_id":"unrelated","output_path":f._dir.path().join("unrelated")})).await.is_err());
}
#[tokio::test]
async fn credentials_cannot_follow_an_advertised_foreign_origin() {
    let f = fixture(false).await;
    assert!(
        f.service
            .jmap
            .trusted_url("https://other.example/jmap")
            .is_err()
    );
    assert!(
        f.service
            .jmap
            .trusted_url("http://user:secret@127.0.0.1/jmap")
            .is_err()
    );
}
#[tokio::test]
async fn archive_preserves_labels_and_stale_state_is_rejected() {
    let f = fixture(false).await;
    assert!(
        f.service
            .execute(
                "move_mail",
                json!({"email_ids":["e"],"destination_role":"archive","if_in_state":"stale"})
            )
            .await
            .is_err()
    );
    f.service
        .execute(
            "move_mail",
            json!({"email_ids":["e"],"destination_role":"archive","if_in_state":"s1"}),
        )
        .await
        .expect("archive");
    let calls = f.fake.calls.lock().expect("calls");
    let update = calls
        .iter()
        .find(|c| c["methodCalls"][0][0] == "Email/set")
        .expect("update");
    assert_eq!(
        update["methodCalls"][0][1]["update"]["e"],
        json!({"mailboxIds/arc":true,"mailboxIds/in":null})
    );
}
#[tokio::test]
async fn unreachable_server_error_names_the_cause_without_leaking_secrets() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("port")
        .local_addr()
        .expect("addr")
        .port();
    let dir = tempfile::tempdir().expect("temp");
    let config = Config {
        session_url: format!("http://127.0.0.1:{port}/session")
            .parse()
            .expect("URL"),
        username: Some("me@example.com".into()),
        password: Some("secret-password".into()),
        bearer: None,
        account_id: None,
        database: dir.path().join("state.sqlite"),
        allow_http: true,
        max_blob_bytes: 25 * 1024 * 1024,
    };
    let error = format!(
        "{:#}",
        Jmap::new(config)
            .expect("jmap")
            .session()
            .await
            .expect_err("closed port")
    );
    assert!(
        error.starts_with("Could not reach JMAP session (connect: "),
        "{error}"
    );
    assert!(
        !error.contains("secret-password") && !error.contains("127.0.0.1"),
        "{error}"
    );
}
