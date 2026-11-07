#!/bin/sh
# Static libheif for the Windows cross build (x86_64-pc-windows-gnu, mingw-w64 win32 threads):
#   libde265 1.0.16 (HEVC → HEIC), dav1d 1.5.1 (AV1 → AVIF), libheif 1.23.1 (the copy vendored in
#   libheif-sys), all static, into <prefix>. Needs: cmake, ninja, meson, nasm,
#   gcc-mingw-w64-x86-64-win32 + g++-mingw-w64-x86-64-win32, curl.
# usage: packaging/win-libheif.sh <work dir>          → <work dir>/prefix, <work dir>/env.sh
# then:  . <work dir>/env.sh && cargo build -p pa-desktop --release --target x86_64-pc-windows-gnu
# (env.sh points libheif-sys' pkg-config probe at the prefix and links heif, de265, dav1d and
#  libstdc++ statically: the exe needs no extra DLLs)
set -e
work=$(mkdir -p "${1:?work dir}" && cd "$1" && pwd)
P="$work/prefix"
mkdir -p "$work/dl" "$work/src"
cd "$work"
[ -f dl/libde265.tar.gz ] || curl -sSL -o dl/libde265.tar.gz https://github.com/strukturag/libde265/releases/download/v1.0.16/libde265-1.0.16.tar.gz
[ -f dl/dav1d.tar.gz ] || curl -sSL -o dl/dav1d.tar.gz https://code.videolan.org/videolan/dav1d/-/archive/1.5.1/dav1d-1.5.1.tar.gz
[ -d src/libde265-1.0.16 ] || tar xzf dl/libde265.tar.gz -C src
[ -d src/dav1d-1.5.1 ] || tar xzf dl/dav1d.tar.gz -C src
if [ ! -d src/libheif ]; then
  vendored=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/libheif-sys-5.*/vendor/libheif | tail -1)
  cp -r "$vendored" src/libheif
  # the example image I/O library is not needed (and pulls libjpeg / libpng / libtiff)
  sed -i 's/add_subdirectory(heifio)//' src/libheif/CMakeLists.txt
fi

cat > mingw.cmake <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32 $P)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
EOF
cat > mingw-meson.txt <<'EOF'
[binaries]
c = 'x86_64-w64-mingw32-gcc'
cpp = 'x86_64-w64-mingw32-g++'
ar = 'x86_64-w64-mingw32-ar'
strip = 'x86_64-w64-mingw32-strip'
windres = 'x86_64-w64-mingw32-windres'
pkg-config = 'pkg-config'
nasm = 'nasm'

[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF

rm -rf b-de265 b-dav1d b-heif
cmake -S src/libde265-1.0.16 -B b-de265 -G Ninja -DCMAKE_TOOLCHAIN_FILE="$work/mingw.cmake" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$P" -DBUILD_SHARED_LIBS=OFF \
  -DENABLE_SDL=OFF -DENABLE_DECODER=OFF -DENABLE_ENCODER=OFF
ninja -C b-de265 install

meson setup b-dav1d src/dav1d-1.5.1 --cross-file "$work/mingw-meson.txt" --prefix="$P" --libdir=lib \
  --buildtype=release --default-library=static -Denable_tools=false -Denable_tests=false -Denable_examples=false
ninja -C b-dav1d install

# every codec off except the two decoders (built in, no plugins); LIBDE265_STATIC_BUILD: de265.h
# must not declare its functions dllimport
off=""
for c in AOM_DECODER AOM_ENCODER X265 X264 RAV1E SvtEnc KVAZAAR JPEG_DECODER JPEG_ENCODER \
  OpenJPEG_DECODER OpenJPEG_ENCODER OPENJPH_ENCODER FFMPEG_DECODER OpenH264_DECODER UVG266 VVDEC VVENC; do
  off="$off -DWITH_$c=OFF -DWITH_${c}_PLUGIN=OFF"
done
# shellcheck disable=SC2086
PKG_CONFIG_LIBDIR="$P/lib/pkgconfig" PKG_CONFIG_PATH='' cmake -S src/libheif -B b-heif -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE="$work/mingw.cmake" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$P" \
  -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_PREFIX_PATH="$P" -DBUILD_SHARED_LIBS=OFF -DBUILD_TESTING=OFF \
  -DWITH_EXAMPLES=OFF -DWITH_EXAMPLE_HEIF_THUMB=OFF -DWITH_EXAMPLE_HEIF_VIEW=OFF -DWITH_GDK_PIXBUF=OFF \
  -DBUILD_DOCUMENTATION=OFF -DENABLE_PLUGIN_LOADING=OFF -DWITH_LIBSHARPYUV=OFF \
  -DWITH_UNCOMPRESSED_CODEC=OFF -DWITH_HEADER_COMPRESSION=OFF \
  -DWITH_LIBDE265=ON -DWITH_LIBDE265_PLUGIN=OFF -DWITH_DAV1D=ON -DWITH_DAV1D_PLUGIN=OFF $off \
  -DCMAKE_C_FLAGS=-DLIBDE265_STATIC_BUILD -DCMAKE_CXX_FLAGS=-DLIBDE265_STATIC_BUILD
ninja -C b-heif install

gcclib=$(dirname "$(x86_64-w64-mingw32-g++ -print-libgcc-file-name)")
cat > env.sh <<EOF
# source me before: cargo build --target x86_64-pc-windows-gnu (libheif-sys finds the static libheif)
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR_x86_64_pc_windows_gnu=$P/lib/pkgconfig
export PKG_CONFIG_PATH_x86_64_pc_windows_gnu=
export SYSTEM_DEPS_LIBHEIF_LINK=static
export SYSTEM_DEPS_LIBHEIF_LIB="heif de265 dav1d stdc++"
export SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE="$P/lib:$gcclib"
EOF
ls -la "$P/lib"
echo "env: $work/env.sh"
