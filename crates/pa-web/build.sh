#!/bin/sh
# Build the browser UI into <out dir>: index.html, the WebAssembly module with its JS loader, and
# the place names the server reads from the web folder (geo/cities.bin).
# usage: crates/pa-web/build.sh <out dir>
# Needs: the wasm32-unknown-unknown target; wasm-bindgen-cli in the version of Cargo.lock (installed
# when missing); gzip, and brotli when it is there.
# No wasm-opt pass: it makes the module smaller (35 → 32 MB) but its compressed copy bigger.
set -e
out=${1:?usage: build.sh <out dir>}
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
want=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print; exit }' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }')
if [ "$have" != "$want" ]; then
  cargo install wasm-bindgen-cli --version "$want" --locked
fi
cargo build -p pa-web --release --locked --target wasm32-unknown-unknown
rm -rf "$out"
mkdir -p "$out/geo"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name peekaround \
  target/wasm32-unknown-unknown/release/pa_web.wasm
# compressed copies: the server sends them to the browsers that accept them
for f in "$out/peekaround_bg.wasm" "$out/peekaround.js"; do
  gzip -9 -k "$f"
  if command -v brotli >/dev/null 2>&1; then
    brotli -q 9 -k "$f"
  fi
done
cp crates/pa-web/web/* brand/logo.svg "$out/"
cp assets/geo/cities.bin "$out/geo/"
ls -la "$out"
