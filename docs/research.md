# Muse integration research

Checked 2026-11-02. Target: Meta Muse mobile and web with its Linux VM, not Muse Code and not ChatGPT Work.

## Recommendation

Keep the mail core independent of the host. Use a Linux Rust executable with structured CLI operations for Muse and stdio MCP for other agents. Users ask the agent to install, import a credential file, choose accounts, fetch attachments, prepare replies, and switch modes. Never ask users to run a command or open a local service.

Use application manual mode by default. Reading, mailbox organization, and drafting can proceed within the user's granted scope. Sending should show the selected account, sender identity, all recipients including Bcc, the complete body, and attachments. The send operation is a separate sensitive action. An optional autonomous mode removes the application confirmation step; host restrictions remain authoritative.

## What Meta establishes

Meta's [connector guidelines](https://platform.muse.ai/docs), sections 3 and 5, classify email sending as a sensitive write. Those writes require approval each time and cannot receive an Always allow grant. The page says portal annotations are coming soon; classification must currently be supplied in tool documentation. This supports a native approval integration for reviewed connectors, not an undocumented CLI switch that forces a card.

Meta's [technical account of Muse safety](https://research.meta.ai/blog/security-and-safety-for-ai-agents-our-approach-with-muse) describes Sentinel as a separate authority for connector calls and outbound network access. Sentinel sends approval requests directly to the client and receives the decision outside the agent conversation. Meta also describes custom integrations using APIs or CLIs.

**Research result:** no public developer contract was found for arbitrary Muse card rendering, a per-call “always show approval” flag for a custom CLI, or a verifiable human-approval token delivered to this program. Native cards and exact approval content remain unverified for this custom integration. Do not equate Muse Code's MCP support with Muse mobile/web compatibility.

The Meta Help Center connector page was referenced during discovery but could not be fetched reliably. Conflicting third-party descriptions of custom MCP setup were not treated as implementation guarantees.

## What the application can guarantee

The implementation validates the selected sending identity, snapshots the draft including Bcc and attachments, checks that the immutable message still matches, requires confirmation fields in manual mode, and records the send claim before making a submission request. A duplicate call with the same intent never deliberately submits again. It refuses to auto-retry an ambiguous outcome.

It cannot prove that `confirmed: true` came from the human rather than the agent. It cannot prevent an agent from editing its own configuration or running a different mail client, and does not attempt to. The user explicitly preferred a simple editable local tool, without a separate host, browser, or approval website.

A chat-rendered preview is useful presentation, not an independent consent mechanism. The native Sentinel prompt, when present, is the host's consent mechanism. If the host doesn't offer a reliable per-send prompt for this workflow, the product must describe manual mode as an agent-mediated approval workflow and must not claim enforced per-send human approval.

## Other agents

The [official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk) supplies stdio protocol handling. Tools expose read-only/destructive/open-world annotations according to their effects. Annotations guide host behavior; they are not authorization.

[MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview) supports HTML resources in compatible hosts. [OpenAI's plugin reference](https://developers.openai.com/plugins/reference) documents UI resource metadata and app-only tool visibility. Neither establishes Meta Muse support. This implementation intentionally ships no HTML resource, iframe, pages, or local web server. Adding those would not solve the target user's current constraints.

## Email implementation decisions

The old [repository](https://github.com/imabee101/stalwart-jmap-mcp) is a small Python stdio implementation: basic search/read/draft/send plus trash/destroy in its source. Its README exposes attachment metadata only. The Rust implementation is independently written and expands the actual email workflow instead of merely translating those tools.

[RFC 8621](https://www.rfc-editor.org/rfc/rfc8621.html) supplies the mail model: immutable message contents, MIME body parts/blobs, reply headers, identities, and submission. Consequently, editing produces a replacement draft; sending uses EmailSubmission; success cleanup removes the draft keyword and moves the message to Sent. Forwarding the original message blob preserves its MIME content. Reply recipient rules and repeat-call protection are application decisions, tested separately.

A deliberately small JMAP HTTP layer makes origin checks, response bounds, object errors, and the no-write-retry policy explicit. The [Stalwart JMAP documentation](https://stalw.art/docs/http/jmap/) explains session discovery and how advertised endpoint URLs derive from the server's public URL. This client rejects unexpected origins rather than forwarding credentials or guessing replacement endpoints.

The [RustCrypto implementation](https://docs.rs/chacha20poly1305/latest/chacha20poly1305/) provides authenticated encryption for the local account vault. The generated key lives in the same VM so the agent can use the accounts without asking the user for a decryption password. This is protection for stored credential bytes, not isolation from the agent.

## Required Muse acceptance exercise

An agent should run this with dedicated test accounts on both Muse mobile and web:

1. Install from the reviewed source/binary through the agent; user runs no commands.
2. Transfer a two-account credential file through the available file mechanism. Import without opening it in the conversation; verify deletion and ensure no secret appears in tool output. Check whether the platform itself extracts uploaded contents.
3. Search both accounts, download an attachment, and present it using Muse's existing artifact mechanism.
4. Prepare a reply containing an attachment. Check how the full preview appears, whether a native approval prompt appears, and what exact details it includes.
5. Deny approval: no send. Approve one send: one delivered message. Change recipients/body/attachments after preview: a fresh preview and approval are necessary.
6. Repeat the send invocation and resume after interruption: no duplicate delivery.
7. Enable application autonomy explicitly for one account. Verify the other stays manual and record whether Sentinel still requires approval.
8. Record results by client/version and connector path. If card support is absent, state that plainly; do not fall back to a localhost browser.

Until this exercise is completed, native Muse cards and end-to-end Muse operation are **not verified**. No connector submission, push, or PR should claim otherwise.
