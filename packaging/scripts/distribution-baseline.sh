#!/usr/bin/env bash
#
# Milestone #2 — real-dataset distribution baseline orchestrator.
#
# Boots the same bundled PG + serve sequence as recognition-pr-baseline.sh,
# then drives `tools/eval_distribution.py` against a glob of input photos
# (default: the existing #2c face fixture, which doubles as the face slice of
# the #2 distribution baseline). No persons/tools/devices are seeded; this is
# pure post-upload distribution capture.
#
# Run as f1u from the repo root (via sudo -u f1u bash -lc):
#   cd /root/F1-photo && bash packaging/scripts/distribution-baseline.sh
#
# Outputs:
#   $REPORT_PATH  : structured report (JSON, default /tmp/distribution-baseline.json)
#   stdout        : human-readable summary
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

F1P_BIN="${F1P_BIN:-}"
if [ -z "$F1P_BIN" ]; then
    if [ -x "$ROOT/dist/f1photo-0.1.0-linux/payload/f1photo" ]; then
        F1P_BIN="$ROOT/dist/f1photo-0.1.0-linux/payload/f1photo"
    elif [ -x "$ROOT/server/target/release/f1photo" ]; then
        F1P_BIN="$ROOT/server/target/release/f1photo"
    elif [ -x "$ROOT/server/target/debug/f1photo" ]; then
        F1P_BIN="$ROOT/server/target/debug/f1photo"
    else
        echo "✗ cannot locate f1photo binary (set F1P_BIN)" >&2
        exit 1
    fi
fi

ORT_DYLIB="${ORT_DYLIB:-/home/f1u/work/runtime/libonnxruntime.so}"
MODELS_DIR="${MODELS_DIR:-$ROOT/models}"
PG_PORT="${PG_PORT:-55444}"
APP_PORT="${APP_PORT:-18799}"
ADMIN_USER="${ADMIN_USER:-smoke_admin}"
ADMIN_PWD="${ADMIN_PWD:-smoke-admin-pwd-12345}"
REPORT_PATH="${REPORT_PATH:-/tmp/distribution-baseline.json}"
PYTHON="${PYTHON:-/usr/bin/python3}"
PHOTOS_GLOB="${PHOTOS_GLOB:-tests/fixtures/face/baseline/**/*.jpg}"

WORK_DIR="$(mktemp -d)"
PGDATA="$WORK_DIR/pgdata"
UPLOADS="$WORK_DIR/uploads"
LOG="$WORK_DIR/server.log"
BASE="http://127.0.0.1:$APP_PORT"

cleanup() {
    rc=$?
    set +e
    if [ -n "${PID:-}" ] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null
        for _ in $(seq 1 5); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 1
        done
        kill -9 "$PID" 2>/dev/null || true
    fi
    for pid in $(pgrep -f "bundled-pg/bin/postgres -D $PGDATA " 2>/dev/null); do
        kill -9 $pid 2>/dev/null || true
    done
    if [ -n "${WORK_DIR:-}" ] && [ -d "$WORK_DIR" ]; then
        if [ "${KEEP_WORK_DIR:-0}" = "1" ]; then
            echo "→ keeping work dir: $WORK_DIR"
        else
            rm -rf "$WORK_DIR"
        fi
    fi
    exit $rc
}
trap cleanup EXIT INT TERM

echo "→ binary:      $F1P_BIN"
echo "→ ORT lib:     $ORT_DYLIB"
echo "→ models:      $MODELS_DIR"
echo "→ pg port:     $PG_PORT"
echo "→ app port:    $APP_PORT"
echo "→ work dir:    $WORK_DIR"
echo "→ photos glob: $PHOTOS_GLOB"
echo "→ report:      $REPORT_PATH"

# Sanity
[ -f "$F1P_BIN" ]                  || { echo "✗ binary missing: $F1P_BIN" >&2; exit 1; }
[ -d "$MODELS_DIR" ]               || { echo "✗ models dir missing: $MODELS_DIR" >&2; exit 1; }
for m in face_detect face_embed object_detect generic_embed; do
    [ -f "$MODELS_DIR/$m.onnx" ] || { echo "✗ missing model $MODELS_DIR/$m.onnx" >&2; exit 1; }
done
[ -f "$ORT_DYLIB" ]                || { echo "✗ ORT dylib missing: $ORT_DYLIB" >&2; exit 1; }
[ -d "$ROOT/bundled-pg/bin" ]      || { echo "✗ bundled-pg/bin missing" >&2; exit 1; }

mkdir -p "$UPLOADS"

# Defensive: kill any orphan bundled postgres on our port.
for pid in $(pgrep -f "bundled-pg/bin/postgres -D .* -p $PG_PORT " 2>/dev/null); do
    kill -9 $pid 2>/dev/null || true
done
sleep 1

common_env() {
    cat <<EOF
F1P_USE_BUNDLED_PG=1
F1P_BUNDLED_PG_PORT=$PG_PORT
F1P_BUNDLED_PG_DIR=$ROOT/bundled-pg/bin
F1P_BUNDLED_PG_DATA=$PGDATA
F1P_BUNDLED_PG_PASSWORD=smokepwd
F1P_JWT_SECRET=smoke-jwt-secret-aaaaaaaaaaaaaaaa
F1P_BIND=127.0.0.1:$APP_PORT
F1P_DATA_DIR=$UPLOADS
F1P_MODELS_DIR=$MODELS_DIR
ORT_DYLIB_PATH=$ORT_DYLIB
RUST_LOG=info,sqlx=warn
EOF
}

echo "▶ bootstrap-admin (also boots bundled PG + runs migrations)"
env $(common_env | xargs) "$F1P_BIN" bootstrap-admin \
    --username "$ADMIN_USER" --password "$ADMIN_PWD" --full-name 'Smoke Admin' \
    > "$WORK_DIR/bootstrap.log" 2>&1 || {
    echo "✗ bootstrap-admin failed; log:" >&2
    cat "$WORK_DIR/bootstrap.log" >&2
    exit 1
}
echo "✓ admin bootstrapped"

echo "▶ starting server"
env $(common_env | xargs) "$F1P_BIN" serve > "$LOG" 2>&1 &
PID=$!

for i in $(seq 1 30); do
    if curl -sS -o /dev/null "$BASE/healthz" 2>/dev/null; then
        echo "✓ server up (PID $PID) after ${i}s"
        break
    fi
    sleep 1
    if ! kill -0 "$PID" 2>/dev/null; then
        echo "✗ server died during startup" >&2
        tail -100 "$LOG" >&2
        exit 1
    fi
done
if ! curl -sS -o /dev/null "$BASE/healthz" 2>/dev/null; then
    echo "✗ server did not become healthy within 30s" >&2
    tail -100 "$LOG" >&2
    exit 1
fi

echo "▶ running tools/eval_distribution.py"
"$PYTHON" "$ROOT/tools/eval_distribution.py" \
    --base-url "$BASE" \
    --admin-user "$ADMIN_USER" --admin-pwd "$ADMIN_PWD" \
    --photos-glob "$PHOTOS_GLOB" \
    --report-path "$REPORT_PATH" \
    --psql-bin "$ROOT/bundled-pg/bin/psql" \
    --pg-host 127.0.0.1 --pg-port "$PG_PORT" \
    --pg-user f1photo --pg-db f1photo_prod \
    --pg-pwd smokepwd
RC=$?
echo "▶ eval_distribution.py exit=$RC report=$REPORT_PATH"
exit $RC
