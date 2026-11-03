# holyproxy

A Rust proxy for public GitHub Release assets and Git HTTPS pulls, with API keys
and usage counters. Supports streaming, download ranges and redirects, and Git
Smart HTTP v0/v2 clone, fetch and pull.

## Usage

Open `/` for instructions and a download form, or set `HOLYPROXY_KEY` and run:

```sh
asset='github.com/OWNER/REPO/releases/download/TAG/FILE'
curl -H "X-Holyproxy-Key: $HOLYPROXY_KEY" "https://proxy.example/$asset"
curl "https://proxy.example/$asset?x_holyproxy_key=$HOLYPROXY_KEY"
curl -u "x_holyproxy_key:$HOLYPROXY_KEY" "https://proxy.example/$asset"
curl "https://x_holyproxy_key:$HOLYPROXY_KEY@proxy.example/$asset"
```

Credentials must agree; duplicates/invalid keys return 403, auth errors 503.
Credentials are stripped upstream, including on redirects. Prefer headers to avoid
keys in URL logs. Basic Auth uses the fixed username `x_holyproxy_key` and your API
key as the password; usage still belongs to the user associated with that key.
All supplied Basic/header/query keys must agree. Malformed Basic credentials are
rejected even when another key is valid. Other Authorization schemes are ignored.

Use HTTPS: Basic Auth encodes credentials; it does not encrypt them. URL userinfo
works with clients such as curl that proactively convert it to Basic Auth; browser
navigation and frontend fetch are not supported ways to use this URL format.
Release downloads do not issue a login challenge. Git endpoints return a 401 Basic
challenge when credentials are missing, so standard Git clients authenticate automatically.
URLs and commands containing keys may appear in
history; do not share them. The public homepage does not validate credentials.

## Local client

Install with `cargo install --path crates/client`, then run:

```sh
holyproxy-client --remote https://proxy.example --key "$HOLYPROXY_KEY" \
  --listen 127.0.0.1:10000
curl http://127.0.0.1:10000/https://github.com/OWNER/REPO/releases/download/TAG/FILE
git clone http://127.0.0.1:10000/https://github.com/OWNER/REPO.git
```

`--remote` and `--key` are required; `--listen` defaults to `127.0.0.1:10042`.
The remote URL accepts HTTP(S) and an optional path prefix, without credentials,
query or fragment. The client appends the incoming path and query and sets
`X-Holyproxy-Key`, replacing any supplied header of that name. Other authentication
headers and query parameters pass through unchanged, so do not supply conflicting keys.

Methods, request bodies, response statuses, headers and bodies are forwarded with
streaming, without automatic redirects, decompression or retries. Host and hop-by-hop
headers are handled for the new connection. Remote connection failures return 502.
The client has no GitHub route or authentication policy; supported targets and
methods remain controlled by the remote service. It is an HTTP URL-prefix relay,
not a CONNECT/HTTPS_PROXY server, and does not tunnel protocol upgrades.
Anyone who can reach the listener can use its configured key; the default binds
only to loopback. Stop with Ctrl-C (active transfers are interrupted).

## Git HTTPS

Use the repository's current canonical URL ending in `.git`. No extra Git
authentication configuration is needed:

```sh
git clone "https://x_holyproxy_key:$HOLYPROXY_KEY@proxy.example/https://github.com/OWNER/REPO.git"
cd REPO
git fetch
git pull --ff-only
```

The fixed username is `x_holyproxy_key`; Git saves the remote URL, including its
password, in `.git/config`. Do not share that file or commands containing keys.
Git responds to the proxy’s 401 challenge using these URL credentials. Invalid or
conflicting credentials still return 403. A header alternative is also available:

```sh
git -c "http.https://proxy.example/.extraHeader=X-Holyproxy-Key: $HOLYPROXY_KEY" \
  clone https://proxy.example/github.com/OWNER/REPO.git
```

Repeat that `-c` option for fetch/pull, or set the same domain-scoped `http` option
in the local repository config (which stores the key). Both inner URL forms,
`/https://github.com/...` and `/github.com/...`, are supported. Credentials belong
to the proxy; upstream GitHub URLs must not contain userinfo. Keys and Authorization
are removed before every upstream request.

Public repositories, full and shallow clones, and protocol v0/v2 are supported.
Private repositories, push, Git LFS, SSH, `git://`, and automatic submodule URL
rewriting are not supported. A renamed repository's old URL may fail: discovery
GET redirects stay on GitHub, but POST redirects return 502 without replaying the
body. Only upload-pack POSTs are allowed, with Git's request media type (otherwise
415); wrong methods return 405 and unsupported targets return 400.

Each discovery or upload-pack HTTP request is counted separately. The initial
unauthenticated 401 challenge counts as an anonymous failure. Counters describe
HTTP status outcomes, not completed clones; a later stream interruption may not be
reflected in them. Git request and response bodies are streamed without pack buffering.

## Keys and D1

Run `python3 scripts/generate_api_key.py` and enter a username. It prints a
base64url username/UUID v4 key, its SHA-256 hash, and username JSON.
From `crates/cloudflare-workers`, initialize local storage:

```sh
npx wrangler d1 migrations apply HOLYPROXY --local
```

Run SQL with `npx wrangler d1 execute HOLYPROXY --local --command "<SQL>"`:

```sql
INSERT INTO api_keys (key_hash, username) VALUES ('<printed hash>', 'alice');
DELETE FROM api_keys WHERE key_hash = '<printed hash>';
SELECT * FROM usage_buckets ORDER BY bucket_start DESC;
```

## Recent request samples

Each usage bucket keeps one latest request sample: `last_method`, `last_path`,
and `last_seen_at` (Unix milliseconds at Worker entry). `bucket_start` remains
Unix seconds. Samples become visible after processing and background persistence;
they are neither random samples nor a complete access log. Older background writes
still increment counts but cannot replace a newer sample. Equal timestamps keep
the first written sample; cross-instance clock skew prevents strict global ordering.

Only the parsed incoming path is stored, without decoding percent escapes. Query,
fragment, URL userinfo, headers (including Authorization), IP and body are excluded.
Methods must be valid ASCII HTTP tokens of at most 32 characters, otherwise NULL.
Paths are capped at 1024 bytes without cutting UTF-8 characters or percent escapes;
unreadable URLs produce NULL paths. Paths can still contain secrets placed there
by clients: this is not arbitrary secret detection. Treat paths as untrusted text
and escape them if rendering HTML.

Existing rows have NULL sample fields until another request updates that bucket;
historical samples cannot be reconstructed. Anonymous failures include invalid
credentials and normal Git 401 probes, so a last sample does not explain every
failure in the bucket. The public homepage is not counted. Background writes are
best effort; failures can lose both counts and samples without failing downloads.

```sql
SELECT datetime(bucket_start, 'unixepoch') AS bucket_utc,
       requests, successes, failures, last_method, last_path,
       datetime(last_seen_at / 1000.0, 'unixepoch') AS last_seen_utc
FROM usage_buckets
WHERE principal_type = 'anonymous'
ORDER BY bucket_start DESC
LIMIT 50;
```

## Deployment

From `crates/cloudflare-workers`, copy the generic config to a private file:

```sh
cp wrangler.jsonc wrangler.private.jsonc
npx wrangler d1 create holyproxy --binding HOLYPROXY \
  --config wrangler.private.jsonc --update-config
npx wrangler d1 migrations apply HOLYPROXY --remote \
  --config wrangler.private.jsonc
```

Apply migrations before deploying the updated Worker, including on existing
installations. Migration `0002_usage_last_request.sql` adds nullable columns and
preserves counters; the old Worker remains compatible. Deploying new code before
the migration causes audit writes to fail. To roll back, restore the previous
Worker version and retain the added columns and data.

Keep real IDs in the ignored private config. Import keys before deploying.
Old KV `1` needs a username. Use `--remote --config wrangler.private.jsonc`
for remote SQL. At repo root: `npx wrangler login`, then `npm run deploy`.
Audit uses UTC 15-minute buckets: requests, successes (2xx/3xx), failures.
Unknown identities share an anonymous bucket. Query in your timezone; background
writes may lose counts. Downloads continue; access logging is disabled.

## Development

Setup (Rust 2024, Node.js/npm): `rustup target add wasm32-unknown-unknown`,
`cargo install worker-build --version 0.8.7 --locked`, then `npm ci`.
At repo root: `npm run dev`, `cargo test --workspace`, `npm run check:deploy`.
The deployment check builds the Worker and performs a dry run without publishing.
