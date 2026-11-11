#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
# Rustup and Homebrew can select different compilers on macOS.
if [ -z "${RUSTC:-}" ] && command -v rustup >/dev/null; then
  RUSTC=$(rustup which rustc)
  export RUSTC
fi
if [ -z "${CC_wasm32_unknown_unknown:-}" ] && [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
  CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
  export CC_wasm32_unknown_unknown
fi
bindgen=${WASM_BINDGEN:-wasm-bindgen}
if [ "$("$bindgen" --version)" != 'wasm-bindgen 0.2.129' ]; then
  echo 'Install wasm-bindgen-cli 0.2.129, or set WASM_BINDGEN to its executable.' >&2
  exit 1
fi
cargo build --locked -p yappr-browser --target wasm32-unknown-unknown --release
mkdir -p app/static/wasm
"$bindgen" target/wasm32-unknown-unknown/release/yappr_browser.wasm --target web --out-dir app/static/wasm --out-name yappr_browser
cp app/static/wasm/yappr_browser.js app/src/service-worker/reader.generated.js
cp -R app/static/wasm/snippets app/src/service-worker/
pnpm --dir app build
mkdir -p pages
cp -R app/build/. pages/
# GitHub Pages must not process the generated _app directory with Jekyll.
touch pages/.nojekyll
