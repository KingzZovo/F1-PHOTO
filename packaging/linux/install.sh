#!/usr/bin/env bash
# Install F1-Photo on a Linux host (one-shot bootstrap).
# Run as root from inside the unpacked release tarball:
#   sudo ./packaging/linux/install.sh
set -euo pipefail

PREFIX=${PREFIX:-/opt/f1photo}
LOGDIR=${LOGDIR:-/var/log/f1photo}
ETCDIR=${ETCDIR:-/etc/f1photo}
USER=${F1PHOTO_USER:-f1photo}
GROUP=${F1PHOTO_GROUP:-f1photo}
SRC=$(cd "$(dirname "$0")/../.." && pwd)

echo "[1/6] creating user/group $USER:$GROUP"
if ! getent group "$GROUP" >/dev/null; then groupadd --system "$GROUP"; fi
if ! id -u "$USER" >/dev/null 2>&1; then
    useradd --system --gid "$GROUP" --home "$PREFIX" --shell /sbin/nologin "$USER"
fi

echo "[2/6] copying payload to $PREFIX"
mkdir -p "$PREFIX" "$LOGDIR" "$ETCDIR"
rsync -a --delete --exclude bundled-pg-data "$SRC/payload/" "$PREFIX/"
chown -R "$USER:$GROUP" "$PREFIX" "$LOGDIR"
chmod 0750 "$PREFIX" "$LOGDIR"

echo "[3/6] writing example env (only if missing)"
if [[ ! -f "$ETCDIR/env" ]]; then
    install -m 600 -o "$USER" -g "$GROUP" "$SRC/packaging/linux/env.example" "$ETCDIR/env"
    echo "  -> edit $ETCDIR/env then re-run"
fi

echo "[4/6] installing systemd unit"
install -m 644 "$SRC/packaging/linux/f1photo.service" /etc/systemd/system/f1photo.service
systemctl daemon-reload

echo "[5/6] running initial bootstrap (creates bundled PG cluster + admin)"
sudo -u "$USER" -E env -i HOME="$PREFIX" PATH=/usr/bin:/bin \
    "$PREFIX/f1photo" --help >/dev/null

echo "[6/6] done. Enable + start with:"
echo "      systemctl enable --now f1photo"
echo "      journalctl -u f1photo -f"
