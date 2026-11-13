#!/bin/sh
# Windows package of the desktop app (cross-built with mingw-w64):
#   PeekAround-<version>-win-x64/ PeekAround.exe, ffmpeg.exe, web/geo/cities.bin, LICENSE.txt, licenses/
#   PeekAround-<version>-win-x64.zip and, when makensis is installed, PeekAround-Setup-<version>.exe
# usage: packaging/win-dist.sh <out dir> [ffmpeg.exe]
#   build first (HEIC / AVIF through a static libheif + libde265 + dav1d, see win-libheif.sh):
#     packaging/win-libheif.sh <work> && . <work>/env.sh
#     cargo build -p pa-desktop --release --target x86_64-pc-windows-gnu
#   (UI fonts are compiled into the exe; their OFL licences go to licenses/)
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
# absolute: makensis resolves relative paths against the .nsi file, not the working directory
out=$(mkdir -p "${1:?out dir}" && cd "$1" && pwd)
ffmpeg=${2:-$root/node_modules/ffmpeg-static/ffmpeg.exe}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
exe="$root/target/x86_64-pc-windows-gnu/release/peekaround-desktop.exe"
name="PeekAround-$version-win-x64"
dir="$out/$name"
rm -rf "$dir" && mkdir -p "$dir/web/geo"
cp "$exe" "$dir/PeekAround.exe"
[ -f "$ffmpeg" ] && cp "$ffmpeg" "$dir/ffmpeg.exe" || echo "warning: no ffmpeg.exe ($ffmpeg)"
cp "$root/assets/geo/cities.bin" "$dir/web/geo/"
cp "$root/LICENSE" "$dir/LICENSE.txt"
mkdir -p "$dir/licenses"
cp "$root"/crates/pa-ui/fonts/OFL-*.txt "$root/crates/pa-ui/ui/icons/LICENSE-lucide.txt" "$dir/licenses/"
(cd "$out" && rm -f "$name.zip" && zip -qr9 "$name.zip" "$name")
echo "zip: $out/$name.zip ($(du -h "$out/$name.zip" | cut -f1))"
if command -v makensis >/dev/null; then
  makensis -V2 -DVERSION="$version" -DICON="$root/crates/pa-desktop/assets/icon.ico" -DSRC="$dir" -DOUT="$out/PeekAround-Setup-$version.exe" "$here/installer.nsi"
  echo "installer: $out/PeekAround-Setup-$version.exe"
fi
