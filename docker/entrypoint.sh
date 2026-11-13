#!/bin/sh
# Run as PUID:PGID so files written into your mounted folders get the right owner.
set -e
if [ "$(id -u)" = "0" ] && [ "${PUID:-0}" != "0" ]; then
  if ! getent group "$PGID" >/dev/null 2>&1; then groupadd -o -g "$PGID" peek; fi
  if ! getent passwd "$PUID" >/dev/null 2>&1; then useradd -o -u "$PUID" -g "$PGID" -d /data -s /bin/sh peek; fi
  mkdir -p "${PEEK_DATA_DIR:-/data}"
  chown -R "$PUID:$PGID" "${PEEK_DATA_DIR:-/data}"
  exec gosu "$PUID:$PGID" "$@"
fi
exec "$@"
