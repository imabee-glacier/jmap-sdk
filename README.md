# JMAP SDK

A Linux-only Rust JMAP email tool, replacing `imabee101/stalwart-jmap-mcp`. Meta Muse is the primary target; other agents can use the same tools over stdio MCP.

One binary. Muse runs it for the user. No browser, pages, localhost service, Docker runtime requirement, or CI.

Status: tested against a live Stalwart server; **not yet validated inside Muse** (native approval cards are unverified). See [validation](docs/validation.md) and [research and approval limitations](docs/research.md).

## User workflow

Ask Muse to set up JMAP SDK and follow [the agent guide](docs/muse.md). The user never runs commands.

For credentials, prepare a file using [the account template](examples/accounts.template.json), fill it outside the conversation, and make that file available to the agent's filesystem through the available file-transfer mechanism. Tell Muse to import it **without opening or displaying it**. The program reads the file directly, encrypts and verifies its contents, then deletes the local plaintext file. No password is needed in a prompt, command argument, tool result, or secrets-manager integration.

Multiple accounts can be imported together. Every mail tool accepts an `account` name; always specify it when working with more than one account. A default can be selected through `select_account`. Modes, send history, and approval snapshots are scoped to the underlying server/mail account.

The import format is JSON with an `accounts` array. Each account has `name`, `session_url`, and either `username` + `password` or `token`. `account_id` is optional for JMAP credentials exposing several accounts. An optional top-level `default_account` chooses the default. Replacing saved credentials requires `replace_existing: true`.

Deletion covers the local import file only, not copies retained by an upload service, conversation, backups, or the user's device. We cannot guarantee an attachment won't be processed by the chat platform before the binary receives it. File deletion is not secure erasure.

## Sending and approvals

| Mode | Behavior |
| --- | --- |
| `manual` (default) | Prepare a complete preview, obtain user approval, send with the matching preview hash and `confirmed: true`. |
| `autonomous` | Prepare and send without the application's confirmation fields, within the user's instructions and the host's permission policy. |

Muse owns its native approval dialog. The current Meta platform documentation classifies sending email as a sensitive write requiring approval on every use. The CLI cannot force a Muse card, prove that someone clicked one, or disable Sentinel. Manual mode is a workflow check, **not an independent human-authorization boundary**. The editable local application deliberately does not try to defend itself against its own agent.

The practical balance is free reading, sorting, and drafting; review before sending by default; explicit per-account autonomy when the user wants it and the host permits it. Details and source links are in [research](docs/research.md).

## Features

- Multiple saved accounts, encrypted file import, default selection, credential replacement/removal.
- Paginated search; messages, threads, mailbox roles/rights, and sender identities.
- Text and HTML drafts; To/Cc/Bcc and Reply-To; editable replacement drafts.
- Reply and reply-all with Reply-To preference, own-identity exclusion, deduplication, `In-Reply-To` and `References`; mark the source answered after successful submission.
- Forwarding with the original message attached as `.eml`, preserving its attachments and formatting.
- Binary attachment upload/download; Unicode filenames; inline Content-ID images; raw-message download. Files stay in the agent filesystem rather than being base64-expanded into model output.
- Read/unread, stars, folders/labels, archive, trash, junk, and restore to Inbox. Archive creates an Archive mailbox if missing. Folder creation, renaming, and reparenting.
- Full send previews including Bcc, identity, body, attachment metadata, and immutable blob identifiers; 15-minute expiry.
- Durable SQLite submission claims, repeat-call protection, delivery status, cancellation of unsent intents, and a send/mode audit log.

Draft replacement retains the old draft for recovery; trash it after verifying the replacement. Forwarding attaches the complete original by default. Reply text is supplied explicitly, including any desired quoted text/signature. There is no permanent deletion, scheduling, contact/calendar management, or S/MIME/PGP signing/decryption in this version.

## Agent/developer installation

The following commands are for the agent or developer, not the end user. Rust and a C toolchain are needed only to build; SQLite is bundled and TLS uses rustls.

```sh
cargo install --locked --git https://github.com/imabee-glacier/jmap-sdk
jmap-sdk tools
```

Or from a checkout: `cargo build --release --locked && install -Dm755 target/release/jmap-sdk "$HOME/.local/bin/jmap-sdk"`.

Import and inspect accounts:

```sh
jmap-sdk call import_accounts '{"path":"/absolute/path/accounts.json"}'
jmap-sdk call list_accounts '{}'
jmap-sdk call account_status '{"account":"personal"}'
jmap-sdk call list_identities '{"account":"personal"}'
```

Use JSON on stdin (`jmap-sdk call TOOL -`) for message contents, avoiding shell interpolation. `tools` prints the complete JSON schemas without requiring credentials. Calls return JSON with `ok` and `result`; failures return `ok: false` and a nonzero exit status. Per-object JMAP failures preserve successes and failure maps so the agent can handle partial completion.

Other agents can start `jmap-sdk serve` as a stdio MCP server. It uses the official Rust MCP SDK. No network listener is started. Do not assume Muse accepts the same MCP configuration as Muse Code; the primary Muse integration is its agent-executed CLI/custom workflow.

## Storage and credentials

`JMAP_SDK_HOME` selects the state directory, defaulting to `$XDG_STATE_HOME/jmap-sdk` or `$HOME/.local/state/jmap-sdk`.

- `accounts.enc`: authenticated XChaCha20-Poly1305 encryption with a fresh random nonce for each save.
- `vault.key`: generated local encryption key; file mode `0600`, directory `0700`.
- `state.sqlite`: durable send state and previews. Contains private mail data, not account passwords. It is permission-restricted, not encrypted.

Atomic replacement, file locking, fsync, and authenticated read-back precede plaintext deletion. Invalid imports keep the source file. A deletion failure is reported explicitly after a successful save. Keep the key and encrypted account file together when moving the installation; losing the key requires importing credentials again. Encryption protects the credential file in isolation, not against someone with access to both files or the running agent VM.

For legacy setups and test harnesses, the binary also accepts `JMAP_SESSION_URL`, `JMAP_TOKEN` or `JMAP_USERNAME`/`JMAP_PASSWORD`, optional `JMAP_ACCOUNT_ID`, and `JMAP_SDK_DATABASE` when no saved account is selected. Saved-account import is the user-facing setup path.

HTTPS is required for saved accounts. Credentials are sent only to the configured session origin, including discovery redirects and advertised API/upload/download URLs. Fix an incorrect Stalwart public URL rather than silently rewriting it. `JMAP_SDK_ALLOW_HTTP=1` applies only to legacy environment configuration for isolated local tests.

## Recovery and limits

A send is durably claimed before `EmailSubmission/set`. After a timeout or process interruption, `uncertain` or `submitting` means **inspect `send_status`; do not resend**. Even no provider records is not proof of failure. A submitted message is accepted for delivery; recipient delivery status is separate. Keep the database across agent restarts. Deleting it, cloning the draft, or submitting through another client is outside the repeat-call guarantee.

Attachments are limited to 25 MiB each and the server's advertised upload limit. Message bodies are capped at 1 MiB per format. Reads expose truncation; send preview refuses incomplete/undecodable body data. Search and bulk mutations are bounded to 100 items. Conditional state checks prevent silently overwriting concurrent mailbox changes. Tokens are supported, but automatic OAuth authorization/refresh is not implemented.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

No CI configuration is included. Live Muse approval compatibility must be verified separately before claiming native-card support or publishing a fully verified Muse integration.
