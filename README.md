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

## Deployment

From `crates/cloudflare-workers`, copy the generic config to a private file:

```sh
cp wrangler.jsonc wrangler.private.jsonc
npx wrangler d1 create holyproxy --binding HOLYPROXY \
  --config wrangler.private.jsonc --update-config
npx wrangler d1 migrations apply HOLYPROXY --remote \
  --config wrangler.private.jsonc
```

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
