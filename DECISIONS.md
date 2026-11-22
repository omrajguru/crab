# Decisions

Choices made while writing the first version, kept short so each can be revisited.

1. **Hand written MCP layer.** The `rmcp` crate is built around a native async runtime, which fits poorly with the wasm32 target. CRAB needs initialize, ping, tools/list, tools/call, and notifications, which is a small amount of code.
2. **Two endpoints, two keys.** `/orch/<key>/mcp` and `/work/<key>/mcp` expose different tool lists, so a worker model sees only worker tools. Independent URL-safe keys must have at least 43 characters. Header-based credentials are also supported.
3. **Key value storage with an index.** One `index` key holds lifetime counters and light task records (status, timestamps, title, target), and each task body lives under `b:<id>`. Listing, claiming, expiry, and purging read only the index, so large code bodies stay out of those paths. The SQL API can replace this later behind the same helper functions.
4. **Empty string stands for an absent target.** It keeps the model types simple and avoids optional fields in stored JSON.
5. **Target matching.** An untargeted task suits any worker. A targeted task suits only a worker that passes the same label.
6. **Late results with a claim number are rejected after reassignment. Legacy late results are accepted** while a task is still pending or claimed, since the worker may finish after its lease lapsed. A done, failed, or cancelled task refuses new results with an explanatory message.
7. **Empty queue is a normal reply,** rather than a tool error, because a worker that checks early is an expected case.
8. **One alarm cadence.** The alarm fires every minute (or every `LEASE_SECONDS` when that is shorter) while a claim exists, and once at the retention deadline when only finished tasks remain.
9. **Waiters live in memory.** If the Durable Object restarts, a waiting `get_result` call ends and the orchestrator calls it again. Stored state is unaffected.
10. **Rate windows live in memory** and reset on restart, which is acceptable for a single operator.
11. **Responses are single JSON bodies.** Server sent events are skipped, and `GET` returns 405. Session ids are omitted, which the specification permits for stateless servers.
12. **JSON-RPC batches are accepted** for older clients, though single messages are the primary path.
