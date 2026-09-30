#!/bin/sh
# Seeds the server's data files into the volume on first start (later edits
# are kept), then runs the server as the echelon user. FE_PUBLIC_ADDRESS and
# FE_LISTEN are read by the server itself.
set -e
umask 077
if [ "$(id -u)" -eq 0 ]; then
  # A volume from an image that ran as root: hand it over (only what isn't
  # the server's already).
  find . -xdev ! -user echelon -exec chown -h echelon:echelon {} + 2>/dev/null || true
  exec setpriv --reuid=echelon --regid=echelon --init-groups \
    --inh-caps=-all,+net_bind_service --ambient-caps=-all,+net_bind_service --bounding-set=-all,+net_bind_service \
    "$0" "$@"
fi
mkdir -p data
for f in /app/data/*; do
  [ -e "data/$(basename "$f")" ] || cp "$f" data/
done
exec /app/dedicated_server "$@"
