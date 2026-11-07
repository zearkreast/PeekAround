#!/bin/sh
# Linux binary tarball of the desktop app:
#   PeekAround-<version>-linux-x64/ peekaround-desktop, web/geo/cities.bin, LICENSE, licenses/, README.txt
#   → PeekAround-<version>-linux-x64.tar.gz
# usage: packaging/linux-dist.sh <out dir>   (after: cargo build -p pa-desktop --release)
# Runtime needs: libheif1 (HEIC / AVIF), ffmpeg on PATH (video), OpenGL / EGL (or SLINT_BACKEND=winit-software),
# libxkbcommon-x11 for X11; a StatusNotifier host for the tray icon; xdg-desktop-portal for file dialogs.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=${1:?out dir}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
exe="$root/target/release/peekaround-desktop"
name="PeekAround-$version-linux-x64"
dir="$out/$name"
rm -rf "$dir" && mkdir -p "$dir/web/geo" "$dir/licenses"
cp "$exe" "$dir/peekaround-desktop"
cp "$root/packages/web/public/geo/cities.bin" "$dir/web/geo/"
cp "$root/LICENSE" "$dir/LICENSE"
cp "$root"/crates/pa-ui/fonts/OFL-*.txt "$root/crates/pa-ui/ui/icons/LICENSE-lucide.txt" "$dir/licenses/"
cat > "$dir/README.txt" <<TXT
PeekAround $version (Linux x64)

Run ./peekaround-desktop. Data: ~/.config/PeekAround (shared with the 1.x Electron app).
Needs: libheif1 (HEIC/AVIF), ffmpeg on PATH (videos), OpenGL/EGL — or SLINT_BACKEND=winit-software.
TXT
(cd "$out" && rm -f "$name.tar.gz" && tar czf "$name.tar.gz" "$name")
echo "tarball: $out/$name.tar.gz ($(du -h "$out/$name.tar.gz" | cut -f1))"
