#!/bin/sh
# Seeds the server's data files into the volume on first start (later edits
# are kept), then runs the server. FE_PUBLIC_ADDRESS and FE_LISTEN are read
# by the server itself.
set -e
mkdir -p data
for f in /app/data/*; do
  [ -e "data/$(basename "$f")" ] || cp "$f" data/
done
exec /app/dedicated_server "$@"
