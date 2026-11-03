# Contributing

Build and test the Yappr browser app locally.

## Build

Requirements: Rust through rustup, Node.js 24 or newer, and pnpm 10.
On macOS, install LLVM with `brew install llvm` for the WebAssembly C dependencies.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
pnpm --dir app install --frozen-lockfile
./scripts/build.sh
cargo run -p yappr-serve -- --static pages --listen 127.0.0.1:8088
```

Then open `http://127.0.0.1:8088`. Set `YAPPR_BASE` to build for a site under a path, such as `/yappr.run`.

## Tests

```sh
cargo test -p yappr-browser --target "$(rustc -vV | sed -n 's/host: //p')"
pnpm --dir app check
pnpm --dir app test
YAPPR_BIN=/path/to/yappr pnpm --dir app test:e2e
```

The browser tests pair a page with the CLI and use the public [relays](https://docs.iroh.computer/concepts/relays), so they need internet access.

## Source layout

| Path | Purpose |
|---|---|
| `app/` | Svelte page: the feed, the setup, and the QR reader |
| `browser/` | The reader in WebAssembly: reading, deliveries, pairing, answers, and browser storage |
| `serve/` | Static file server for development and tests |
| `scripts/build.sh` | Builds the reader and the page into `pages/` |
