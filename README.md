# Yappr app

The Yappr app shows the updates and questions from your machines in one feed, in a browser.
It pairs with the [yappr](https://github.com/fractalops/yappr) command line on each machine.

The app runs in the page. It connects to your machines with iroh. A browser cannot open direct connections, so its traffic goes through a relay. The relay passes on encrypted data.
The app saves the entries in the browser's storage on this device.

## Connect a computer

[Install yappr](https://github.com/fractalops/yappr#install) on the computer, then start it with a pairing code:

```sh
yappr start --pair
```

In the app, open **Setup**, then scan the QR code, import an image of it, or paste the `YAPPR:` code.
Select **Request pairing**, then enter `y` on the computer to accept the device.

The computer delivers each new entry to the app while the page is open, also when `yappr start` does not run: `yappr post`, `ask`, and `approve` deliver by themselves.
Questions and approvals show their choices in the feed. Select one, and use **Undo** within five seconds if needed.
**Open** shows the questions that wait for an answer.

Keep the page open to receive entries. A closed page gets the missed entries when it opens again and the computer runs `yappr start`.
Only one tab can use the app at a time.

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
YAPPR_BIN=/path/to/yappr pnpm --dir app test:e2e
```

The browser tests pair a page with the CLI and use the public relays, so they need internet access.

## Source layout

| Path | Purpose |
|---|---|
| `app/` | Svelte page: the feed, the setup, and the QR reader |
| `browser/` | The reader in WebAssembly: reading, deliveries, pairing, answers, and browser storage |
| `serve/` | Static file server for development and tests |
| `scripts/build.sh` | Builds the reader and the page into `pages/` |
