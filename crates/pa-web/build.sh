#!/bin/sh
# Build the browser UI into <out dir>: index.html, the WebAssembly module with its JS loader, and
# the place names the server reads from the web folder (geo/cities.bin).
# usage: crates/pa-web/build.sh <out dir>
# Needs: the wasm32-unknown-unknown target; wasm-bindgen-cli in the version of Cargo.lock (installed
# when missing); gzip. wasm-opt (binaryen) and brotli are used when they are there.
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
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    "$out/peekaround_bg.wasm" -o "$out/peekaround_bg.wasm.opt"
  mv "$out/peekaround_bg.wasm.opt" "$out/peekaround_bg.wasm"
fi
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
