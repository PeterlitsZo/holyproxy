# holyproxy

A Rust proxy for public GitHub Release assets with API keys and usage counters.
Supports GET, HEAD, ranges, streaming, and GitHub download redirects.

## Usage

Set `HOLYPROXY_KEY` to an allowed key, then prefix the asset URL:

```sh
asset='github.com/OWNER/REPO/releases/download/TAG/FILE'
curl -H "X-Holyproxy-Key: $HOLYPROXY_KEY" "https://proxy.example/$asset"
curl "https://proxy.example/$asset?x_holyproxy_key=$HOLYPROXY_KEY"
```

Credentials must agree; duplicates/invalid keys return 403, auth errors 503.
Credentials are stripped upstream. Prefer headers to avoid keys in URL logs.

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
At repo root: `npm run dev`, `cargo test --workspace`, `npm run test:worker`.
