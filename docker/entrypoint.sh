#!/bin/sh
set -eu
umask 077
# HOME/profile/cache are ephemeral; the only persistent application state is /data.
mkdir -p "$HOME"
chmod 0700 "$HOME"
exec "$@"
