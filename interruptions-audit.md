# CRAB v0.1 release audit

> This is the original pre-fix audit. The README describes the current implementation and local verification.

Reviewed the full supplied checkout on November 22, 2026. **Hold deployment.** The mailbox, role-specific tools and pure state machine exist. The Worker fails to compile, the deployment configuration risks recording credentials, and runtime acceptance remains open.

## Build evidence

Tests ran against a temporary copy of the supplied source. A clean Cargo resolution selected worker 0.6.7. The supplied tree has no Cargo.lock. The seven Rust files total 1,445 lines including tests and blank lines.

| Check | Outcome |
| --- | --- |
| Native cargo test | Compile error E0277 at src/tools.rs:208. The test calls unwrap_err on a result whose success type lacks Debug. |
| Isolated native retest | Changing only the assertion to inspect the error allowed all 28 existing tests to pass. |
| Release cargo check for wasm32-unknown-unknown | worker-macros cannot import syn items gated behind its full feature. |
| Isolated WASM diagnostic | Enabling syn 2's full feature for build dependencies exposed additional errors. The impl attribute at src/relay.rs:43 is rejected, and lines 328 and 342 match Option where Storage::get returns a concrete value. The resolved SDK reports an absent key as an error. Distinguish a fresh index from a storage failure and a missing task body. |
| Local Worker, Inspector, production and three real clients | No runnable Worker from the supplied source. No runtime acceptance or latency result established. |

## Release findings

| Severity | Area and location | Impact and correction |
| --- | --- | --- |
| Blocker | Build, Cargo.toml and src/relay.rs:43 | Resolve the macro and storage API errors. Lock dependencies and the worker-build tool. Require native tests, the release WASM build and a local first-task round trip in CI. |
| High | Credentials, wrangler.toml:17 | Default Cloudflare fetch invocation logs include request URLs. Both role paths and the status path contain reusable credentials. Disable invocation logs and verify redaction across log destinations using disposable keys before using real ones. Prefer credential headers where supported. Rotate any keys used with URL logging enabled. |
| High | Leases, src/relay.rs:311 and src/model.rs:259 | Claims, cancellations and completions can repeatedly move the only alarm to a fresh tick while any claim exists. Expired work can stay claimed beyond 15 minutes under ordinary traffic. Preserve the earliest deadline and reconcile overdue claims before selecting work. Verify expiration while other tasks change. |
| High | Input and resource limits, src/router.rs:46 and src/relay.rs:65 | Authenticated bodies are fully buffered before the 1 MB check. Status skips the role limiter. Batches consume one request credit, accumulate replies in memory and can execute several waits sequentially. Stream-count actual bytes, bound batch size, response bytes and total wait time, and apply the documented role budget to status. |
| High | Persistence and recovery, src/relay.rs:117,216,293 | Body, index and alarm operations have separate awaited failure points. Cleanup removes IDs before deleting bodies, leaving no indexed cleanup path if deletion fails. A claim can be stored before alarm scheduling fails. Make related changes atomic where supported and make cleanup and alarm repair retryable. Test recovery across restart. |
| Medium | IDs and retries, src/relay.rs:24,199 | IDs have no collision check. RNG failure substitutes the clock, allowing duplicate IDs and replacement of an existing body. Reject RNG failure, verify uniqueness before writing and consider submission idempotency for clients that retry after losing a response. |
| Medium | Role boundary, src/auth.rs:3 and src/router.rs:33 | Sixteen configured bytes pass the length check. Identical keys would give both roles the same credential. Provision independently generated 32-byte or longer secrets, reject equal or example values, and validate the configured encoding. The documented generator itself uses sufficient random bytes. |
| Medium | Protocol and method handling, src/router.rs:29 and src/mcp.rs:42 | Origin and MCP-Protocol-Version headers are unchecked, IDs can be malformed, and malformed initialization parameters can negotiate successfully. Health and status accept other methods. Validate the negotiated transport requirements and envelope, add negative tests, and distinguish malformed parameters from tool-domain errors. Echoing the 2024 version alone does not implement its legacy SSE transport. |
| Medium | Capacity and recovery prompts, src/relay.rs:201 and README.md | All retained tasks count toward 500. Cancelling a task leaves it in the index for seven days despite the error advising cancellation to make room. Document retained capacity or separate active and retained quotas. Both orchestrator prompts say to wait until done even after failure or cancellation. Stop on every terminal status. |
| Medium | Multiple workers, src/model.rs:182 | Under the PRD's late-result rule, an old worker may finish a reassigned task before the new worker. Shared labels provide routing only. Document first-result-wins for trusted workers or revise the contract to return a claim generation and reject superseded claims. |
| Medium | Monitoring, src/model.rs:328 and src/relay.rs:171 | Status counts only retained tasks. Expiration history and lifetime created, completed and failed counters are missing. Tool logs omit task ID and input size. Persist counters with transitions and emit structured, allowlisted metadata. |
| Medium | Smoke acceptance, scripts/smoke.sh:4 | The script prints expected results, accepts HTTP errors and can finish successfully with an empty parsed ID. It claims arbitrary open work and may disturb an existing queue. Use an isolated namespace, parse JSON, assert status and content, and cover simultaneous claims, overlapping waits, cancellation, role checks both ways, size and rate limits, three expiries and restart persistence. |
| Low | Targeted tasks, README.md worker prompt | The standard check message omits the worker label, leaving targeted work pending. Configure the label once per worker or keep tasks untargeted by default. |

The product is a single-operator text protocol. Billing, visual states, and screen accessibility have no first-party flows in this checkout. Client approvals, permissions, data sharing and readable recovery prompts remain relevant. The server contains no model API or code execution path. The orchestrator prompt calls for review and keeping secrets out. Add worker guidance to treat task context as data and restrict returned paths and edits to the approved project scope during review.

## Acceptance and release order

1. Restore both builds, lock the build inputs and remove secret URLs from telemetry. Verify a fresh deployment can create its first task.
2. Fix lease scheduling, storage recovery and input bounds. Assert all six tools and both roles in the local Worker. The native tests demonstrate selection and role membership only. They cannot establish atomic claims under concurrent requests.
3. On a disposable deployment, exercise each tool in Inspector. Record timing for an already waiting collector, end-to-end server time across the four calls, and per-call p99 excluding waits. The thresholds are 100 ms, 2 s and 300 ms respectively. Include cold and warm requests and realistic task sizes. Record request, storage and duration usage before claiming free-tier fit.
4. Complete a round with an actual orchestrator, Cursor and a web connector. Record plan, connector configuration, confirmations, empty-queue behavior and returned file integrity. Use disposable credentials and synthetic code.

## Product decisions

- Start with Claude web for the web trial. Its setup explicitly accepts paths and a no-sign-in configuration. Cursor documents Streamable HTTP. ChatGPT supports MCP writes on eligible setups and may require confirmation. Count extra confirmations separately from typed prompts. Exact CRAB compatibility remains unverified for all three.
- Establish a per-client result limit by returning known files with an end marker and independently checking byte length and SHA-256 after collection. Exercise both tool arguments and tool results, repeat at increasing sizes, include Unicode, and publish the highest consistently successful size. The 200,000-character CRAB ceiling is not a measured client limit.
- Keep optional target routing in v1. It is implemented and covered by a pure unit test. Leave it blank for one worker and document labels before the second worker joins.
- Keep prompts user-initiated through official connectors and preserve each provider's approvals. Consumer terms restrict automation, extraction or resale in different ways. A supported connector alone does not settle whether recurring output collection is allowed under a particular subscription. Verify the intended use under the actual plan before relying on it. Transfer only code authorized for the receiving provider. Nothing in this audit establishes an account violation.

## Sources

- [Cloudflare invocation logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/) and [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- [MCP transport requirements](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Claude web connectors](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp), [Cursor MCP](https://cursor.com/docs/mcp) and [ChatGPT MCP permissions](https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt-beta)
- [Anthropic consumer terms](https://www.anthropic.com/legal/consumer-terms) and [OpenAI terms of use](https://openai.com/policies/terms-of-use/)

## Attribution

This audit was produced by the [`interruptions`](https://github.com/omrajguru05/interruptions) skill, built by **Om Rajguru**.
