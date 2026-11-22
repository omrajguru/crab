# CRAB

**Cross-model Relay for Agentic Building**

Sometimes the model I want to ask about a piece of code lives in a different app from the one where I am working. The agent in my editor has the project and knows what needs changing. The other model can do the work, provided I give it the relevant files and bring its answer back. Most of the inconvenience is in carrying the specification and the files between the two.

I have been working on CRAB to handle that part. It gives both agents a shared mailbox through the Model Context Protocol (MCP), a way for an app to expose tools to a model. The agent with the project puts a task in the mailbox. I send one short message to the model I want to do the work. It picks up the task and returns complete files, and the first agent can review them in the project where they belong.

[![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)](https://deploy.workers.cloudflare.com/?url=https://github.com/omrajguru/crab)

Use the button to deploy a copy to your Cloudflare account, or follow the [command-line deployment](#deploy-from-your-terminal) from a local checkout.

[First task](#a-first-task) · [Local setup](#run-crab-locally) · [Cloudflare setup](#deploy-to-cloudflare) · [Client setup](#connect-your-clients) · [Tools](#tool-reference) · [Files](#sending-files) · [Troubleshooting](#when-something-goes-wrong)

## A first task

Suppose I want to change how a CSV importer handles a byte-order mark at the beginning of a file. The agent in my editor can identify the parser, the tests, and the public interface that other code depends on. A second model can work on that fairly independently. It needs those two files and a precise description of the change. Giving it access to the rest of the repository would add little.

1. The **orchestrator**, the agent with the project, calls `submit_task`. It attaches the parser and its tests, specifies that the public interface must stay the same, and asks for a regression test covering a file with the mark and a file without it. CRAB returns a task ID.
2. I send the **worker**, the model in the other app, this message: “Check CRAB for work. Claim the next task and return the complete changed files.”
3. The worker calls `get_next_task`. It receives the specification, the files, and a claim number. When it has finished, it calls `submit_result` with the task ID, the claim number, and the replacement files.
4. The orchestrator calls `get_result` with the task ID and `wait_seconds: 25`. It can start waiting as soon as it submits the task. Once the files arrive, it reviews the diff, applies the approved changes, and runs the importer tests in the project.

The person who deploys CRAB and sends that check message is the **operator**. One person can operate the whole relay. Each model stays in its original app, with that app’s permissions and approval prompts. CRAB stores the handoff. The worker still needs the operator’s message to start, and the project environment remains responsible for reviewing and testing the result.

## Run CRAB locally

From the directory containing `Cargo.toml` and `wrangler.toml`, use a current stable Rust toolchain installed through rustup and Node.js 24 with npm. The server compiles to WebAssembly. Wrangler runs that build in Cloudflare’s local Worker runtime.

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked worker-build --version 0.8.7
cargo test --locked
```

Make sure your shell can find `worker-build`. Cargo usually installs it under `~/.cargo/bin`, or `%USERPROFILE%\.cargo\bin` on Windows.

### Create the two keys

Copy [the environment example](.dev.vars.example) to `.dev.vars`. Use `cp .dev.vars.example .dev.vars` in a Unix shell or `Copy-Item .dev.vars.example .dev.vars` in PowerShell. Generate a key with the command below. Run it twice and save one value as `ORCH_KEY` and the other as `WORK_KEY`.

```sh
node -e "console.log(require('node:crypto').randomBytes(32).toString('hex'))"
```

Each run produces 64 hexadecimal characters from 32 random bytes. Keep the values distinct. Only the orchestrator needs `ORCH_KEY`. A worker receives `WORK_KEY`. The example values are deliberately rejected, and Git ignores `.dev.vars`.

The example sets `LEASE_SECONDS=1` for the expiry test. **Remove that line for ordinary development and production.** A real coding task needs the default 15-minute lease.

```sh
npx wrangler@4.166.0 dev --local
```

Open `http://127.0.0.1:8787/health`. It should return `ok`. That confirms the HTTP process is running. The [acceptance suite](#verify-your-installation) also exercises authentication, storage, and delivery. A hosted web app needs a reachable HTTPS deployment to contact your relay.

## Deploy to Cloudflare

You need a Cloudflare account with permission to deploy Workers and create a SQLite-backed Durable Object. A Durable Object is where CRAB keeps its queue between requests. SQLite-backed objects are available on the Workers Free plan. Actual usage depends on the number of calls, stored tasks, and time spent waiting for results. [Cloudflare Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)

### Deploy with the button

The button at the top opens Cloudflare’s deployment form for this repository. Choose your account and the name for your copy. Cloudflare reads the `RELAY` binding and the `v1` migration from [wrangler.toml](wrangler.toml).

1. Generate two fresh production keys. Enter them as the Worker secrets `ORCH_KEY` and `WORK_KEY`. If Cloudflare imports `LEASE_SECONDS` from the example, omit it or set it to `900`.
2. Set the **build command** to `rustup target add wasm32-unknown-unknown && cargo install --locked worker-build --version 0.8.7` in a build environment with Rust and Cargo available. This installs the packager that the repository’s Wrangler build command invokes.
3. Set the **deploy command** to `npx wrangler@4.166.0 deploy`. Wrangler runs the repository’s WebAssembly build and applies the Durable Object migration.
4. Copy the resulting HTTPS URL. Check `/health`, then verify `/status` with the orchestrator credential before connecting a client.

Cloudflare’s button creates a copy of the source in the deploying user’s Git account and deploys the Worker to that user’s Cloudflare account. [Deploy button documentation](https://developers.cloudflare.com/workers/platform/deploy-buttons/)

### Deploy from your terminal

Complete the local tool installation first. Run the following commands from the repository root. If your account already has a Worker called `crab`, choose an unused `name` in `wrangler.toml` before the first deployment.

```sh
npx wrangler@4.166.0 login
npx wrangler@4.166.0 whoami
npx wrangler@4.166.0 deploy
npx wrangler@4.166.0 secret put ORCH_KEY
npx wrangler@4.166.0 secret put WORK_KEY
```

Enter each production key at its prompt. Protected routes reject requests until their credential is configured. Local `.dev.vars` values serve local development. The `secret put` commands configure the deployed Worker. A `workers.dev` address is sufficient.

On macOS or Linux, with `CRAB_URL` set to the deployed base URL and `ORCH_KEY` available in your shell, check both routes. On PowerShell use `curl.exe` and the corresponding `$env:CRAB_URL` and `$env:ORCH_KEY` variables.

```sh
curl --fail "$CRAB_URL/health"
curl --fail -H "Authorization: Bearer $ORCH_KEY" "$CRAB_URL/status"
```

A new relay should report zero tasks. For subsequent releases, rerun `cargo test --locked` and `npx wrangler@4.166.0 deploy`. Keep the Durable Object binding, class, and migration history when updating an existing mailbox.

## Connect your clients

Add CRAB as a **remote MCP server using Streamable HTTP**. Choose the endpoint by the job the agent will do. An IDE can serve either role. A web app processing a task only needs the worker endpoint.

| Role | URL when the client supports headers | Header value |
| --- | --- | --- |
| Orchestrator | `https://<your-worker>/orch/mcp` | `Authorization: Bearer <ORCH_KEY>` |
| Worker | `https://<your-worker>/work/mcp` | `Authorization: Bearer <WORK_KEY>` |

For a connector that only accepts a URL, use `https://<your-worker>/orch/<ORCH_KEY>/mcp` or `https://<your-worker>/work/<WORK_KEY>/mcp`. Select the client’s no-sign-in option if it asks about OAuth. The entire URL is then a credential. Prefer headers when available because URLs can appear in histories, logs, screenshots, and shared configuration.

For Claude, ChatGPT, or Gemini, use the remote connector configuration available in your account, enable it in the conversation, and keep the app’s tool approvals in place. For Cursor, a user-level MCP configuration for a URL-only connection looks like this. Replace both placeholders locally and keep credentials out of project-owned settings.

```json
{
  "mcpServers": {
    "crab-worker": {
      "url": "https://<your-worker>/work/<WORK_KEY>/mcp"
    }
  }
}
```

Ask the client which CRAB tools it can see. A worker should see exactly `get_next_task` and `submit_result`. An orchestrator should see the four tools in the next section. This checks the credential, transport, and choice of role before you send any project files.

### Standing instructions

Give the orchestrator a short rule it can reuse across tasks.

> Submit a self-contained CRAB task with the goal, target paths, relevant files, interfaces to preserve, and acceptance checks. Send only material approved for the receiving provider. Keep the task ID. Ask me to send the worker its check message, then call get_result with wait_seconds 25 while the task is pending or claimed. On failure or cancellation, stop and report the status. On completion, review the returned paths and changes against the specification before applying them and running the project’s checks.

Give a named worker its label once in its standing instructions. For example, “Your CRAB target is `gemini`. Pass it to `get_next_task` whenever I ask you to check CRAB. Echo the returned `claim_number` in `submit_result` and send complete files.” After that, “Check CRAB for work” is enough. With one worker, leaving the target blank on both sides is simpler.

## Tool reference

| Tool | Arguments | What comes back |
| --- | --- | --- |
| `submit_task` | Required `title`, `requirements`. Optional `context`, `target`, `files`, `request_id`. | The task ID and status. New tasks start as `pending`. |
| `get_result` | Required `task_id`. Optional `wait_seconds`, default `0`, maximum `25`. | Status, code, files, summary, expired attempt count, task ID, and a `next_action` hint. |
| `list_tasks` | Optional `status` and `limit`. The default limit is 20 and the maximum is 100. | Recent tasks, newest first, with ID, title, status, and age in seconds. |
| `cancel_task` | Required `task_id`. | The task ID and `cancelled` status for an open task. |
| `get_next_task` | Optional `target`. | The oldest eligible pending task, its files, claim number, and return instructions, or the text `The queue is empty.` |
| `submit_result` | Required `task_id` and at least one file or nonblank `code`. Optional `claim_number` and `summary`. | The task ID and `done` status. The orchestrator collects the actual content. |

### Sending files

Pass the same attachment format to `submit_task` and `submit_result`. Task attachments arrive under `get_next_task.files`. Returned attachments arrive under `get_result.files`. For example, these are valid arguments for a task submission.

```json
{
  "title": "Add coverage for the greeting fixture",
  "requirements": "Write a test that reads fixtures/greeting.txt as UTF-8 and checks its final newline. Return the complete test file.",
  "request_id": "greeting-fixture-test-1",
  "files": [
    {"path": "fixtures/greeting.txt", "encoding": "text", "content": "hello 世界\n"}
  ]
}
```

Use `text` for UTF-8 and `base64` for binary. For example, `{"path":"fixtures/sample.bin","encoding":"base64","content":"AP8B"}` carries the three bytes `00 ff 01`. An empty file has an empty content string. Paths must be portable, relative to the project, and unique within a transfer. Use forward slashes. A file and a directory cannot occupy the same path.

CRAB computes SHA-256 over the decoded bytes and returns the digest with each file. If you supply an independently computed `sha256`, CRAB also checks it on receipt. That distinction matters when checking a large transfer. A digest calculated after a client has already shortened the content describes the shortened file. Compare against the original when byte-for-byte fidelity matters.

The legacy `code` field accepts several complete files in one string with a path header above each file. Structured attachments are preferable when filenames, empty files, exact newlines, or binary content matter.

### Claims, retries, and retention

A claim reserves the oldest matching task for 15 minutes. An untargeted task is eligible for any worker. A targeted task is eligible for a worker that supplies the matching label, compared without regard to ASCII case. Labels help distribute work. All workers on the same deployment share a role credential.

An expired claim returns to `pending`. After the third expiry the task becomes `failed`. Passing `claim_number` lets a worker finish slightly late while its task is still pending, but rejects its result once another worker has claimed it. Older clients that omit the number retain first-result-wins behavior. A completed, failed, or cancelled task refuses further results.

If the orchestrator loses the response to `submit_task`, it can retry with the same `request_id` and the same task details. CRAB returns the original task while it is retained. Reusing the identifier for different work produces an error. A lost connection during `get_result` only requires another call with the same task ID.

CRAB retains completed, failed, and cancelled tasks for seven days after their last change. Pending tasks remain available. All retained tasks count toward capacity, including cancelled tasks. Collect files you want to keep before retention expires.

## Operating the relay

A Rust Worker authenticates the incoming HTTP request and forwards it to one Durable Object called `Relay`. Its persistent index holds task metadata and counters. The larger task and result bodies live under separate storage keys. A claim changes the shared queue before the specification goes back to its worker, so two workers checking at the same time can claim different tasks.

When the orchestrator chooses to wait, the relay registers a waiter for that task. Finishing the task wakes the waiting request after the result is stored. An alarm handles lease expiry and retention. You can close the original chat and collect a retained result from another orchestrator session using the task ID.

### Limits

| Resource | Limit |
| --- | --- |
| Title, target, summary | 120, 40, and 5,000 characters respectively |
| Requirements, context, legacy code | 60,000, 120,000, and 200,000 characters respectively |
| Attachments per direction | 16 files and 200,000 decoded bytes combined |
| File path and submission request ID | 240 bytes per path and 80 bytes per request ID |
| Entire HTTP request | 1,000,000 bytes, including JSON and base64 overhead |
| Queue capacity | 500 retained tasks |
| Requests | 120 per minute per role, shared across clients using that role |
| Waiting | Up to 25 seconds per call and 16 concurrent collectors per task |
| JSON-RPC batch | Up to 4 messages. Send long waits as individual requests. |

The rate window is held in memory and resets if the object restarts. Connector limits may be lower than server limits. To measure one client, send fixtures of 1,024, 16,384, 65,536, and 200,000 bytes, ask it to return them unchanged, and compare lengths and hashes outside the model. Include Unicode, binary data, and a final marker. Record the client version, plan, and largest size that survives repeated transfers.

### Endpoints and protocol

`GET /health` is public and returns `ok`. `GET /status` requires the orchestrator bearer key. URL-only clients can use `GET /status/<ORCH_KEY>`. Status reports retained counts, the age of the oldest pending task in seconds, and persistent counters for creation, completion, expiry, and failure. An expiry counts a lease, so one task can contribute several expiries.

Both MCP endpoints accept JSON-RPC over HTTP POST. Tool discovery uses `tools/list`; calls use `tools/call`. Results carry MCP text content, and structured tool output is JSON inside that text. A tool-level error can therefore arrive with HTTP 200 and `isError: true`. Protocol errors appear in the JSON-RPC `error` object. Notifications receive HTTP 202 with an empty body.

Initialization supports `2025-06-18`, `2025-03-26`, and `2024-11-05`, with `2025-06-18` as the fallback. The transport uses JSON responses without session IDs. Select Streamable HTTP in clients that offer both HTTP and legacy SSE. GET on an authenticated MCP path returns 405.

### Credentials and project files

Deploy a separate relay for each trust boundary. Anyone holding the worker credential can claim eligible tasks and read the attached source. Anyone holding the orchestrator credential can submit, inspect, collect, or cancel work. CRAB does not provide per-person accounts or file access rules.

Only send files the receiving provider is allowed to process. Review returned code before executing it. When writing an attachment to disk, resolve its path and any existing symlinks against the intended project directory. A valid filename and a matching digest establish where the file says it belongs and which bytes arrived. They say nothing about what the code will do.

Application logs record tool metadata. Cloudflare invocation URL logging is disabled in this project because a URL may contain a role key. Other clients and proxies have their own logs. To rotate a credential, run the corresponding `wrangler secret put` command again, update each connector using that role, and check its tool list. Keep the other role on its own key.

## Verify your installation

Run the acceptance suite against an **isolated local queue**. It creates tasks, submits results, cancels work, and deliberately reaches the worker rate limit. Use disposable keys and set `LEASE_SECONDS=1` in the test `.dev.vars`. Start Wrangler with a fresh storage directory for each run.

```sh
npx wrangler@4.166.0 dev --local --persist-to .wrangler/crab-test-01
```

In another terminal, from the same directory, run the suite with Node.js 24.

```sh
node --env-file=.dev.vars -e "process.env.TEST_LEASE='1'; import('./scripts/smoke.mjs')"
```

The 30 native tests cover the protocol, model, roles, and file validation. The local suite exercises all six tools, simultaneous claims, long waits, expired claims, size and rate limits, and file integrity through the server ceiling. MCP Inspector has also exercised both role lists and all six tools locally. Stored files and counters survived a local restart. Production p99 latency, free-tier consumption under your workload, and reliable file sizes in each real client remain deployment measurements. [Acceptance suite](scripts/smoke.mjs) · [CI workflow](.github/workflows/relay.yml)

## When something goes wrong

| Symptom | What to check |
| --- | --- |
| Health works but the connector gets 404 | Check the role, path, and credential. Wrong keys and unknown routes deliberately share a response. Verify that the keys are distinct and that production secrets were set on the deployed Worker. |
| The browser reports 403 | Check the actual `Origin` header. If the origin should have access, add its exact origin to the comma-separated `ALLOWED_ORIGINS` Worker variable. Server-to-server requests without an Origin do not need an entry. |
| Opening the connector URL in a browser gives 405 | The browser used GET. Use `/health` for a browser check and let the MCP client POST to its endpoint. |
| The worker sees an empty queue | Check `list_tasks` for a pending task. Confirm that the worker is connected to the same deployment and supplied its assigned target. Another worker may already hold the claim. |
| The task keeps returning to pending | Check the lease setting. A production deployment with the test value of one second will repeatedly expire ordinary work. |
| A file is refused or appears incomplete | Check the decoded byte total, serialized request size, path, encoding, and original digest. Split the work into smaller tasks if the client truncates it before submission. |
| A request returns 429 or a wait expires | For 429, let the role’s minute window reset. For an ordinary wait timeout, read the returned status and `next_action`. Continue waiting only while the task is pending or claimed. |
| The build cannot find `worker-build` | Install version 0.8.7 in the environment actually running Wrangler, add Cargo’s bin directory to PATH, and confirm the WebAssembly target is installed there. |

## Source guide

The [request router](src/router.rs) and [key checks](src/auth.rs) define the HTTP boundary. The [MCP layer](src/mcp.rs) and [tool schemas](src/tools.rs) describe what clients see. The [task model](src/model.rs) defines state transitions and limits, the [relay](src/relay.rs) owns storage and delivery, and the [file module](src/files.rs) validates attachments. The [design decisions](DECISIONS.md) explain the reasoning behind the mailbox, the two roles, and the lease model.

The crate declares the MIT license in [Cargo.toml](Cargo.toml).
