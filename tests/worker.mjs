import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import {
  Miniflare, convertV4MiniflareOptions, Log, LogLevel,
} from "miniflare";

const base = new URL("../crates/cloudflare-workers/", import.meta.url);
const schema = await readFile(new URL("migrations/0001_storage.sql", base), "utf8");
const increment = await readFile(new URL("sql/increment_usage.sql", base), "utf8");
const statements = schema.split(";").map((s) => s.trim()).filter(Boolean);
const errors = [];
const outgoing = [];
const mf = new Miniflare(convertV4MiniflareOptions({
  modules: [
    { type: "ESModule", path: fileURLToPath(new URL("build/index.js", base)) },
    { type: "CompiledWasm", path: fileURLToPath(new URL("build/index_bg.wasm", base)) },
  ],
  compatibilityDate: "2026-10-19",
  d1Databases: { HOLYPROXY: "local-test-database" },
  cf: false,
  log: new Log(LogLevel.NONE),
  handleStructuredLogs: (entry) => errors.push(JSON.stringify(entry)),
  outboundService: async (request) => {
    outgoing.push(request);
    assert.equal(request.headers.get("X-Holyproxy-Key"), null);
    const url = new URL(request.url);
    assert.equal(url.searchParams.has("x_holyproxy_key"), false);
    if (url.hostname === "github.com") {
      return new Response(null, {
        status: 302,
        headers: { Location: `https://release-assets.githubusercontent.com${url.pathname}` },
      });
    }
    const status = Number(url.pathname.split("/").at(-1));
    return new Response(status === 304 || request.method === "HEAD" ? null : "download", { status });
  },
}));

async function eventually(check) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.fail("Background audit did not reach the expected state");
}

try {
  const db = await mf.getD1Database("HOLYPROXY");
  await db.batch(statements.map((sql) => db.prepare(sql)));
  const addKey = async (key, username) => {
    const hash = createHash("sha256").update(key).digest("hex");
    await db.prepare("INSERT INTO api_keys (key_hash, username) VALUES (?1, ?2)")
      .bind(hash, username).run();
  };
  await addKey("alice-key", "alice");
  await addKey("alice-second-key", "alice");
  await addKey("bob-key", "bob");
  await addKey("quoted-key", "小明'\"\n");

  async function request(key, status = 200, method = "GET", query = false) {
    const url = `https://proxy.example/github.com/a/b/releases/download/v1/${status}`;
    const response = await mf.dispatchFetch(
      query ? `${url}?x_holyproxy_key=${key}` : url,
      { method, headers: key && !query ? { "X-Holyproxy-Key": key } : {} },
    );
    const actual = response.status;
    await response.text();
    return actual;
  }
  async function counts(username, type = "user") {
    return db.prepare(`SELECT COALESCE(SUM(requests), 0) AS requests,
      COALESCE(SUM(successes), 0) AS successes,
      COALESCE(SUM(failures), 0) AS failures FROM usage_buckets
      WHERE principal_type = ?1 AND username = ?2`).bind(type, username).first();
  }

  // Concurrent requests and two keys for one identity must not lose increments.
  const statuses = await Promise.all(Array.from({ length: 24 }, (_, i) =>
    request(i % 2 ? "alice-key" : "alice-second-key", i % 3 ? 206 : 404)));
  assert.equal(statuses.filter((s) => s === 206).length, 16);
  await eventually(async () => (await counts("alice")).requests === 24);
  assert.deepEqual(await counts("alice"), { requests: 24, successes: 16, failures: 8 });
  assert.equal(outgoing.length, 48); // Each request followed one redirect.

  assert.equal(await request("bob-key", 304), 304);
  assert.equal(await request("bob-key", 200, "HEAD"), 200);
  assert.equal(await request("bob-key", 500), 500);
  assert.equal(await request("bob-key", 200, "POST"), 405);
  assert.equal(await request("quoted-key", 200, "GET", true), 200);
  const beforeAnonymous = outgoing.length;
  assert.equal(await request(null), 403);
  assert.equal(await request("unknown-key"), 403);
  assert.equal(outgoing.length, beforeAnonymous);
  await eventually(async () => (await counts("", "anonymous")).requests === 2);
  await eventually(async () => (await counts("bob")).requests === 4);
  await eventually(async () => (await counts("小明'\"\n")).requests === 1);
  assert.deepEqual(await counts("bob"), { requests: 4, successes: 2, failures: 2 });
  assert.deepEqual(await counts("", "anonymous"), { requests: 2, successes: 0, failures: 2 });
  const rows = await db.prepare("SELECT * FROM usage_buckets").all();
  for (const row of rows.results) {
    assert.equal(row.bucket_start % 900, 0);
    assert.equal(row.requests, row.successes + row.failures);
  }

  // Exercise the same SQL across historical buckets and principal categories.
  await Promise.all([
    db.prepare(increment).bind(900, "user", "alice", 1, 0).run(),
    db.prepare(increment).bind(1800, "user", "alice", 0, 1).run(),
    db.prepare(increment).bind(900, "user", "anonymous", 1, 0).run(),
    db.prepare(increment).bind(900, "anonymous", "", 0, 1).run(),
  ]);
  const historical = await db.prepare(
    "SELECT * FROM usage_buckets WHERE bucket_start < 2700",
  ).all();
  assert.equal(historical.results.length, 4);
  assert(historical.results.every((row) => row.requests === 1));

  // Failed audit persistence must not replace a successful download response.
  await db.prepare("DROP TABLE usage_buckets").run();
  assert.equal(await request("alice-key"), 200);
  await eventually(() => errors.some((entry) => entry.includes("Usage audit write failed")));
  assert(!errors.some((entry) => entry.includes("alice-key")));

  // Unavailable authentication storage still fails closed.
  await db.prepare(statements[1]).run();
  await db.prepare("DROP TABLE api_keys").run();
  assert.equal(await request("alice-key"), 503);
  await eventually(async () => (await counts("", "anonymous")).requests === 1);
  assert.deepEqual(await counts("", "anonymous"), { requests: 1, successes: 0, failures: 1 });
  console.log("Workers/D1 integration passed: identity, concurrency, buckets, outcomes, background failures.");
} finally {
  await mf.dispose();
}
