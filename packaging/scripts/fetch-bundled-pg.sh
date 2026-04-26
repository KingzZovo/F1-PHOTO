#!/usr/bin/env bash
# Fetch a portable PostgreSQL 16 + pgvector binary tree for the current host.
#
# Output: ./bundled-pg/{bin,lib,share}/...
# Usage:
#   ./packaging/scripts/fetch-bundled-pg.sh [--from-system|--from-cache]
#
# Strategies:
#   --from-system : copy /usr/lib/postgresql/16 + pgvector deb files into a
#                   self-contained tree (works on aarch64 + x86_64 Ubuntu hosts
#                   that have apt installed postgresql-16 + postgresql-16-pgvector).
#   --from-cache  : copy a pre-fetched tree from ./bundled-pg-cache/$TARGET/.
#   default       : download EnterpriseDB binaries (linux-x86_64 only).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

TARGET=${TARGET:-}
FROM_SYSTEM=0
FROM_CACHE=0
PG_VERSION=${PG_VERSION:-16.6}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET=$2; shift 2 ;;
        --from-system) FROM_SYSTEM=1; shift ;;
        --from-cache) FROM_CACHE=1; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

ARCH=$(uname -m)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
if [[ -z "$TARGET" ]]; then
    case "$OS-$ARCH" in
        linux-x86_64)  TARGET=linux-x86_64 ;;
        linux-aarch64) TARGET=linux-aarch64 ;;
        *) echo "unsupported host: $OS-$ARCH" >&2; exit 2 ;;
    esac
fi

DEST=$ROOT/bundled-pg
rm -rf "$DEST"
mkdir -p "$DEST/bin" "$DEST/lib" "$DEST/share/extension"

if [[ "$FROM_SYSTEM" == 1 ]]; then
    echo "[+] copying portable tree from /usr/lib/postgresql/16"
    if [[ ! -d /usr/lib/postgresql/16 ]]; then
        echo "  /usr/lib/postgresql/16 not found. Run: apt install postgresql-16" >&2
        exit 1
    fi
    cp -a /usr/lib/postgresql/16/bin/. "$DEST/bin/"
    cp -a /usr/lib/postgresql/16/lib/. "$DEST/lib/"
    cp -a /usr/share/postgresql/16/. "$DEST/share/"

    # Bundle libpq + ICU + zstd + lz4 + krb5 deps so we don't need system
    # libs at runtime on the deployment host.
    for so in $(ldd "$DEST/bin/postgres" | awk '/=>/ {print $3}' | sort -u); do
        case "$so" in
            /lib/*|/lib64/*|/usr/lib/aarch64-linux-gnu/libc.so*|\
            /usr/lib/x86_64-linux-gnu/libc.so*) ;;
            ""|"not") ;;
            *)
                if [[ -f "$so" ]]; then
                    cp -L "$so" "$DEST/lib/" 2>/dev/null || true
                fi
                ;;
        esac
    done
    if command -v patchelf >/dev/null 2>&1; then
        for b in postgres initdb pg_ctl psql pg_dump pg_restore createdb dropdb; do
            if [[ -f "$DEST/bin/$b" ]]; then
                patchelf --set-rpath '$ORIGIN/../lib' "$DEST/bin/$b" || true
            fi
        done
    else
        echo "  patchelf missing - bundled binaries will fall back to system libs" >&2
    fi

    # pgvector: prefer the apt-installed copy (postgresql-16-pgvector) since it
    # was already copied via /usr/lib/postgresql/16/lib + /usr/share/postgresql/16.
    if [[ -f "$DEST/lib/vector.so" ]]; then
        echo "  pgvector $(ls $DEST/share/extension/vector--*.sql 2>/dev/null | tail -1)"
    else
        echo "  pgvector NOT found. Run: apt install postgresql-16-pgvector" >&2
        exit 1
    fi

elif [[ "$FROM_CACHE" == 1 ]]; then
    CACHE=$ROOT/bundled-pg-cache/$TARGET
    if [[ ! -d "$CACHE" ]]; then
        echo "  $CACHE missing - populate it with a pre-fetched tree." >&2
        exit 1
    fi
    cp -a "$CACHE/." "$DEST/"

else
    case "$TARGET" in
        linux-x86_64)
            URL="https://get.enterprisedb.com/postgresql/postgresql-${PG_VERSION}-1-linux-x64-binaries.tar.gz"
            echo "[+] downloading $URL"
            curl -fsSL "$URL" -o /tmp/pg.tgz
            tar -xzf /tmp/pg.tgz -C "$DEST" --strip-components=1
            ;;
        linux-aarch64)
            echo "  EnterpriseDB does not publish aarch64 portable binaries." >&2
            echo "  Re-run with --from-system on a Debian/Ubuntu aarch64 host." >&2
            exit 2
            ;;
        *) echo "unsupported target: $TARGET" >&2; exit 2 ;;
    esac
fi

echo "[+] sanity check"
"$DEST/bin/postgres" --version
if [[ -f "$DEST/lib/vector.so" ]]; then echo "  pgvector .so present"; fi
ls "$DEST/share/extension/vector.control" >/dev/null && echo "  pgvector control present"
echo "OK -> $DEST"
