#!/bin/sh
# regenerates ui/icons.slint from ui/icons/*.svg (run from crates/pa-ui)
cd ui && { echo "// Generated from icons/*.svg (lucide, ISC — see icons/LICENSE-lucide.txt). Colour them with \`colorize\`."; echo "export global Icons {"; for f in icons/*.svg; do n=$(basename $f .svg); echo "    out property <image> $n: @image-url(\"$f\");"; done; echo "    out property <image> logo: @image-url(\"assets/logo.svg\");"; echo "}"; } > icons.slint
