#!/usr/bin/env bash
#
# Milestone #2c — face recognition Precision/Recall baseline orchestrator.
#
# Mirrors smoke-e2e.sh's startup sequence (mktemp work dir, F1P_USE_BUNDLED_PG=1
# so the server boots its own bundled PostgreSQL via maybe_start(), bootstrap
# admin via the `bootstrap-admin` CLI subcommand, then `serve` in a subprocess),
# then drives a real recognition pass via tools/eval_pr.py and writes a
# Precision/Recall report.
#
# Run as f1u from the repo root (via sudo -u f1u bash -lc):
#   cd /root/F1-photo && bash packaging/scripts/recognition-pr-baseline.sh
#
# Outputs:
#   $REPORT_PATH  : structured report (JSON, default /tmp/pr-baseline.json)
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
REPORT_PATH="${REPORT_PATH:-/tmp/pr-baseline.json}"
# Default to system python3 so this works as the unprivileged f1u user, which
# cannot read venvs under /root. eval_pr.py uses stdlib only.
PYTHON="${PYTHON:-/usr/bin/python3}"
MANIFEST_REL="${MANIFEST_REL:-tests/fixtures/face/baseline/MANIFEST.json}"

WORK_DIR="$(mktemp -d)"
PGDATA="$WORK_DIR/pgdata"
UPLOADS="$WORK_DIR/uploads"
LOG="$WORK_DIR/server.log"
BASE="http://127.0.0.1:$APP_PORT"

PID=""
cleanup() {
    local rc=$?
    set +e
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null
        wait "$PID" 2>/dev/null
    fi
    if [ $rc -ne 0 ]; then
        echo "✗ pr-baseline FAILED (exit $rc); last 100 lines of $LOG:" >&2
        tail -n 100 "$LOG" 2>/dev/null >&2 || true
        echo "--- bootstrap log ---" >&2
        cat "$WORK_DIR/bootstrap.log" 2>/dev/null >&2 || true
    fi
    rm -rf "$WORK_DIR"
    exit $rc
}
trap cleanup EXIT INT TERM

echo "→ binary:    $F1P_BIN"
echo "→ ORT lib:   $ORT_DYLIB"
echo "→ models:    $MODELS_DIR"
echo "→ pg port:   $PG_PORT"
echo "→ app port:  $APP_PORT"
echo "→ work dir:  $WORK_DIR"
echo "→ manifest:  $MANIFEST_REL"
echo "→ report:    $REPORT_PATH"

# Sanity
[ -f "$F1P_BIN" ]                  || { echo "✗ binary missing: $F1P_BIN" >&2; exit 1; }
[ -d "$MODELS_DIR" ]               || { echo "✗ models dir missing: $MODELS_DIR" >&2; exit 1; }
for m in face_detect face_embed object_detect generic_embed; do
    [ -f "$MODELS_DIR/$m.onnx" ] || { echo "✗ missing model $MODELS_DIR/$m.onnx" >&2; exit 1; }
done
[ -f "$ORT_DYLIB" ]                || { echo "✗ ORT dylib missing: $ORT_DYLIB" >&2; exit 1; }
[ -d "$ROOT/bundled-pg/bin" ]      || { echo "✗ bundled-pg/bin missing" >&2; exit 1; }
[ -f "$ROOT/$MANIFEST_REL" ]       || { echo "✗ fixture manifest missing: $ROOT/$MANIFEST_REL" >&2; exit 1; }

mkdir -p "$UPLOADS"

# Defensive: kill any orphan bundled postgres on our port from a previous run.
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

echo "▶ running tools/eval_pr.py"
"$PYTHON" "$ROOT/tools/eval_pr.py" \
    --base-url "$BASE" \
    --admin-user "$ADMIN_USER" --admin-pwd "$ADMIN_PWD" \
    --manifest "$MANIFEST_REL" \
    --report-path "$REPORT_PATH" \
    --psql-bin "$ROOT/bundled-pg/bin/psql" \
    --pg-host 127.0.0.1 --pg-port "$PG_PORT" \
    --pg-user f1photo --pg-db f1photo_prod \
    --pg-pwd smokepwd
RC=$?
echo "▶ eval_pr.py exit=$RC report=$REPORT_PATH"
exit $RC
