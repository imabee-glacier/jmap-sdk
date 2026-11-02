# Validation

What has been checked, and what has not. Last run: 2026-11-02, Linux x86_64, Rust stable.

## Automated (repeatable, no network)

`cargo test --locked --all-targets` runs 30 tests:

- **core** (15): reply/reply-all recipient rules (Reply-To preference, own-identity and duplicate exclusion, no Bcc leakage), threading headers, MIME tree for text+HTML, inline CID images and regular attachments, header-injection rejection, truncated-body refusal, manual-mode confirmation and preview-hash matching, idempotent and account-scoped prepare, cancel and re-prepare, durable claims across restarts and separate connections, preview expiry, readable forward header block, MCP schema validity, and `send_draft` annotated as destructive.
- **protocol** (9, against an in-process fake JMAP server): one submission per intent under duplicate and concurrent calls, upstream failure after submission recorded as `uncertain` and never retried, changed recipient or wrong identity blocks a send, autonomous mode skips only the application confirmation, compose never submits, search pagination, attachment byte round trip without overwriting files, refusal to send credentials to a foreign advertised origin, and archive preserving labels with stale state rejected.
- **accounts** (6): multi-account import, encryption, authenticated read-back, and plaintext deletion; invalid imports redacted and the source kept; wrong key or tampering fails closed; replacement requires `replace_existing`; duplicate names do not partially import; account tools work without credentials or network.

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean.

## Live Stalwart (manual, disposable)

Run against a throwaway `stalwartlabs/stalwart:v1` container with two fixture accounts on an isolated Docker network, using the CLI exactly as an agent would (`jmap-sdk call TOOL -`). The container and fixture data were removed afterwards. The test used plain HTTP via the legacy environment path (`JMAP_SDK_ALLOW_HTTP=1`); saved accounts require HTTPS and were covered by the automated tests.

Passed:

1. Upload a binary file and create a draft with text, HTML, a Unicode attachment filename (`résumé.bin`), an inline CID image and a Bcc recipient.
2. Download the raw message; the MIME parses, the attachment bytes match the original, `Content-ID: <logo>` is present, and the UTF-8 body is intact.
3. `prepare_send`, then `send_draft` with confirmation: submitted, with `delivered: yes` in the delivery status. A repeated `send_draft` returns the existing result without a second submission.
4. Second account sends to the first; the first account cannot see the second's send intent (account isolation).
5. Reply-all: `In-Reply-To` and `References` are correct, the reply is in the same thread, and the recipient is correct. After sending, the original is marked `$answered`.
6. Forward: original attached as `message/rfc822`, and the quoted header block is readable plain text.
7. `send_draft` without confirmation is refused in manual mode. Cancel, prepare again, and send the same draft.
8. Read/star, create a folder, add a label, archive (the Archive mailbox was **auto-created** because Stalwart had none, and the label was kept), move to trash, restore to inbox, and reject a stale `if_in_state`.
9. Audit log records the send and mode events.

MCP stdio: `initialize` reports `jmap-sdk`, `tools/list` returns 25 tools, and `tools/call` works.

## Not verified

- **Meta Muse end to end.** Not exercised inside Muse mobile or web. It is unknown whether Muse shows a native approval card for this CLI or MCP integration. Follow the acceptance exercise in [research.md](research.md#required-muse-acceptance-exercise) before claiming native-card support.
- Providers other than Stalwart (Fastmail and others). The client follows RFC 8621 but has only been run against Stalwart.
- Bearer-token accounts against a live server (they are covered only by unit tests). OAuth refresh is not implemented.
- Attachments near the 25 MiB limit on a live server.
