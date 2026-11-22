import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { setTimeout as sleep } from "node:timers/promises";

const base = new URL(process.env.BASE ?? "http://127.0.0.1:8787");
assert(
  ["127.0.0.1", "localhost", "[::1]"].includes(base.hostname),
  "Run against an isolated local relay",
);
const keys = { orch: process.env.ORCH_KEY, work: process.env.WORK_KEY };
assert(keys.orch && keys.work, "Set both local test keys");
const url = (role) => new URL("/" + role + "/" + keys[role] + "/mcp", base);
const post = (role, message) =>
  fetch(url(role), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(message),
  });
async function rpc(role, method, params = {}) {
  const response = await post(role, { jsonrpc: "2.0", id: 1, method, params });
  assert.equal(response.status, 200);
  return response.json();
}
async function tool(role, name, args = {}, error = false) {
  const { result } = await rpc(role, "tools/call", { name, arguments: args });
  assert.ok(result);
  assert.equal(
    result.isError,
    error,
    result.isError ? result.content[0].text : undefined,
  );
  if (error) return;
  const text = result.content[0].text;
  return text === "The queue is empty." ? text : JSON.parse(text);
}
const target = randomUUID();
const task = (extra = {}) =>
  tool("orch", "submit_task", {
    title: "Local acceptance",
    requirements: "Return the supplied files unchanged.",
    target,
    ...extra,
  });
const claim = () => tool("work", "get_next_task", { target });
const result = (id, seconds = 0) =>
  tool("orch", "get_result", { task_id: id, wait_seconds: seconds });
const status = async () =>
  (await fetch(new URL("/status/" + keys.orch, base))).json();
assert.equal((await status()).pending, 0, "Use an empty local namespace");
for (const [role, count] of [
  ["orch", 4],
  ["work", 2],
]) {
  assert.equal(
    (
      await rpc(role, "initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "acceptance", version: "1" },
      })
    ).result.serverInfo.name,
    "crab",
  );
  assert.equal((await rpc(role, "tools/list")).result.tools.length, count);
  assert.equal(
    (await post(role, { jsonrpc: "2.0", method: "notifications/initialized" }))
      .status,
    202,
  );
  assert.equal((await fetch(url(role))).status, 405);
}
for (const [role, name] of [
  ["work", "submit_task"],
  ["orch", "submit_result"],
])
  assert.equal((await rpc(role, "tools/call", { name })).error.code, -32602);
assert.equal((await fetch(new URL("/orch/incorrect/mcp", base))).status, 404);
const files = [
  { path: "example/hello.txt", encoding: "text", content: "hello 世界\n" },
  { path: "example/bytes.bin", encoding: "base64", content: "AP8B" },
  { path: "empty.txt", encoding: "text", content: "" },
];
for (const f of files)
  f.sha256 = createHash("sha256")
    .update(Buffer.from(f.content, f.encoding === "text" ? "utf8" : "base64"))
    .digest("hex");
const request_id = randomUUID();
const { task_id } = await task({ files, request_id });
assert.equal((await task({ files, request_id })).task_id, task_id);
const headerStatus = await fetch(new URL("/status", base), {
  headers: { Authorization: "Bearer " + keys.orch },
});
assert.equal(headerStatus.status, 200);
for (const bad of [
  { ...files[0], path: "../outside" },
  { ...files[1], content: "INVALID" },
  { ...files[0], sha256: "0".repeat(64) },
  { ...files[0], content: "x".repeat(200001) },
])
  await tool(
    "orch",
    "submit_task",
    { title: "x", requirements: "x", files: [bad] },
    true,
  );
const claims = await Promise.all([claim(), claim(), claim()]);
assert.equal(claims.filter((c) => c.task_id === task_id).length, 1);
assert.deepEqual(claims.find((c) => c.task_id === task_id).files, files);
const waiting = result(task_id, 5).then((value) => ({
  value,
  at: performance.now(),
}));
await sleep(100);
const start = performance.now();
await tool("work", "submit_result", { task_id, files, claim_number: 1 });
const submittedAt = performance.now();
const delivery = await waiting;
const collected = delivery.value;
assert.equal(collected.status, "done");
assert.deepEqual(collected.files, files);
console.log(
  "Local submit and waiting collection:",
  Math.round(delivery.at - start),
  "ms; collection after submit response:",
  Math.max(0, Math.round(delivery.at - submittedAt)),
  "ms",
);
for (const size of [1024, 16384, 65536, 200000]) {
  const content = "x".repeat(size - 4) + "END!";
  const sha256 = createHash("sha256").update(content).digest("hex");
  const input = { path: "fixture.txt", encoding: "text", content, sha256 };
  const { task_id: id } = await task({ files: [input] });
  assert.deepEqual((await claim()).files, [input]);
  await tool("work", "submit_result", { task_id: id, files: [input] });
  assert.deepEqual((await result(id)).files, [input]);
}
const { task_id: cancelled } = await task();
await tool("orch", "cancel_task", { task_id: cancelled });
await tool("work", "submit_result", { task_id: cancelled, code: "late" }, true);
assert(
  (await tool("orch", "list_tasks", { status: "cancelled" })).some(
    (t) => t.task_id === cancelled,
  ),
);
await tool(
  "orch",
  "submit_task",
  { title: "x", requirements: "x".repeat(60001) },
  true,
);
assert.equal((await post("orch", "x".repeat(1000000))).status, 413);
if (process.env.TEST_LEASE === "1") {
  const { task_id: id } = await task();
  for (let attempt = 1; attempt <= 3; attempt++) {
    const leased = await claim();
    assert.equal(leased.task_id, id);
    assert.equal(leased.claim_number, attempt);
    if (attempt > 1)
      await tool(
        "work",
        "submit_result",
        { task_id: id, claim_number: attempt - 1, code: "old" },
        true,
      );
    let row;
    for (let i = 0; i < 30; i++) {
      await sleep(200);
      row = await result(id);
      if (row.attempts === attempt) break;
    }
    assert.equal(
      row.status,
      attempt === 3 ? "failed" : "pending",
      "Run with LEASE_SECONDS=1",
    );
  }
}
assert((await status()).counters.completed >= 5);
let limited = false;
for (let i = 0; i <= 120; i++) {
  const r = await post("work", { jsonrpc: "2.0", id: 1, method: "ping" });
  await r.arrayBuffer();
  if (r.status === 429) {
    limited = true;
    break;
  }
}
assert(limited);
console.log("All local relay acceptance checks passed.");
