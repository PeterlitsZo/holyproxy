# holyproxy

A minimal public GitHub Release asset proxy built with Rust and workers-rs.
Supports GET, HEAD, range requests, and streaming downloads with GitHub
redirects.

## Usage

Prefix a Release asset URL with your proxy origin:

```text
https://proxy.example/https://github.com/OWNER/REPO/releases/download/TAG/FILE
https://proxy.example/github.com/OWNER/REPO/releases/latest/download/FILE
```

## Development

`crates/core` defines HTTP traits and proxy logic; `crates/cloudflare-workers`
implements the adapter. Requires Rust with edition 2024 support, Node.js, and npm.

```sh
rustup target add wasm32-unknown-unknown
cargo install worker-build --version 0.8.7 --locked
npm ci
npm run dev
```

Run `cargo test -p holyproxy-core` to test routing and `npm run check:deploy` to
validate the build. To deploy, run `npx wrangler login`, then `npm run deploy`.
