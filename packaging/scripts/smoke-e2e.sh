#!/usr/bin/env bash
#
# End-to-end HTTP smoke test for f1-photo.
#
# Spins up bundled PostgreSQL + the f1photo binary, bootstraps an admin user
# via the `bootstrap-admin` CLI subcommand, then exercises every API surface
# via curl with strict HTTP status code assertions.
#
# Failures dump the server log and exit 1.
#
# Tunable via env:
#   F1P_BIN     path to f1photo binary (default: dist/f1photo-0.1.0-linux/payload/f1photo,
#               falling back to server/target/release/f1photo, then debug)
#   ORT_DYLIB   path to libonnxruntime.so (default: /home/f1u/work/runtime/libonnxruntime.so)
#   MODELS_DIR  path to ONNX models directory (default: ./models)
#   PG_PORT     bundled PG port (default 55444)
#   APP_PORT    f1photo bind port (default 18799)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

F1P_BIN="${F1P_BIN:-}"
if [[ -z "$F1P_BIN" ]]; then
    if [[ -x "$ROOT/dist/f1photo-0.1.0-linux/payload/f1photo" ]]; then
        F1P_BIN="$ROOT/dist/f1photo-0.1.0-linux/payload/f1photo"
    elif [[ -x "$ROOT/server/target/release/f1photo" ]]; then
        F1P_BIN="$ROOT/server/target/release/f1photo"
    elif [[ -x "$ROOT/server/target/debug/f1photo" ]]; then
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

WORK_DIR="$(mktemp -d)"
PGDATA="$WORK_DIR/pgdata"
UPLOADS="$WORK_DIR/uploads"
LOG="$WORK_DIR/f1p.log"
ADMIN_USER="smoke_admin"
ADMIN_PWD="smoke-admin-pwd-12345"
BASE="http://127.0.0.1:$APP_PORT"

# bundled_pg.rs uses hardcoded DEFAULT_USER='f1photo' / DEFAULT_DB='f1photo_prod'
# (no env override). The bundled cluster + database are created automatically
# by maybe_start() on first run of any subcommand (incl. bootstrap-admin).

PID=""
cleanup() {
    local rc=$?
    if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
    fi
    if [[ $rc -ne 0 ]]; then
        echo "✗ smoke FAILED (exit $rc); last 100 lines of $LOG:" >&2
        tail -n 100 "$LOG" >&2 2>/dev/null || true
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

assert_http() {
    local expected="$1"; shift
    local method="$1"; shift
    local url="$1"; shift
    local body_file="$WORK_DIR/last.body"
    local code
    code="$(curl -sS -o "$body_file" -w '%{http_code}' -X "$method" "$@" "$url")"
    if [[ "$code" != "$expected" ]]; then
        echo "✗ $method $url → expected $expected, got $code" >&2
        echo "   body: $(head -c 400 "$body_file")" >&2
        return 1
    fi
    echo "✓ $method $url → $code"
}

jq_get() {
    python3 -c "import json,sys; print(json.load(open('$WORK_DIR/last.body'))$1)"
}

[[ -f "$F1P_BIN" ]]    || { echo "✗ binary missing: $F1P_BIN" >&2; exit 1; }
[[ -d "$MODELS_DIR" ]] || { echo "✗ models missing: $MODELS_DIR" >&2; exit 1; }
for m in face_detect face_embed object_detect generic_embed; do
    [[ -f "$MODELS_DIR/$m.onnx" ]] || {
        echo "✗ missing model $MODELS_DIR/$m.onnx (run packaging/scripts/make-stub-models.py)" >&2
        exit 1
    }
done
[[ -f "$ORT_DYLIB" ]]      || { echo "✗ ORT dylib missing: $ORT_DYLIB" >&2; exit 1; }
[[ -d "$ROOT/bundled-pg/bin" ]] || { echo "✗ bundled-pg/bin missing (run fetch-bundled-pg.sh)" >&2; exit 1; }

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
    if curl -sS -o /dev/null "http://127.0.0.1:$APP_PORT/healthz"; then
        echo "✓ server up (PID $PID)"
        break
    fi
    sleep 1
    if ! kill -0 "$PID" 2>/dev/null; then
        echo "✗ server died during startup" >&2
        tail -100 "$LOG" >&2
        exit 1
    fi
done
if ! curl -sS -o /dev/null "http://127.0.0.1:$APP_PORT/healthz"; then
    echo "✗ server failed to start within 30s" >&2
    tail -100 "$LOG" >&2
    exit 1
fi

echo "▶ health"
assert_http 200 GET "$BASE/healthz"
assert_http 200 GET "$BASE/readyz"

echo "▶ auth: anonymous"
assert_http 401 GET "$BASE/api/auth/me"
assert_http 401 POST "$BASE/api/auth/login" -H 'content-type: application/json' \
    -d '{"username":"'"$ADMIN_USER"'","password":"WRONG-PASSWORD"}'

echo "▶ auth: login good"
assert_http 200 POST "$BASE/api/auth/login" -H 'content-type: application/json' \
    -d '{"username":"'"$ADMIN_USER"'","password":"'"$ADMIN_PWD"'"}'
TOKEN="$(jq_get "['access_token']")"
[[ -n "$TOKEN" && "$TOKEN" != "None" ]] || { echo "✗ no access_token" >&2; exit 1; }
ROLE="$(jq_get "['user']['role']")"
[[ "$ROLE" == "admin" ]] || { echo "✗ role $ROLE != admin" >&2; exit 1; }
echo "✓ token acquired (role=$ROLE)"

AUTH_H="Authorization: Bearer $TOKEN"

echo "▶ auth: me with token"
assert_http 200 GET "$BASE/api/auth/me" -H "$AUTH_H"

echo "▶ projects: create"
assert_http 201 POST "$BASE/api/projects" -H "$AUTH_H" -H 'content-type: application/json' \
    -d '{"code":"smk-001","name":"Smoke Test Project","icon":"🔥","description":"e2e"}'
PROJECT_ID="$(jq_get "['id']")"
echo "  project_id=$PROJECT_ID"

echo "▶ projects: list contains created"
assert_http 200 GET "$BASE/api/projects" -H "$AUTH_H"
python3 -c "import json; ds=json.load(open('$WORK_DIR/last.body')); items=ds.get('items', ds) if isinstance(ds,dict) else ds; assert any(p.get('id')=='$PROJECT_ID' for p in items), 'project not in list'"
echo "  contains $PROJECT_ID ✓"

echo "▶ projects: my perms"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/me" -H "$AUTH_H"
IS_ADMIN="$(jq_get "['is_admin']")"
[[ "$IS_ADMIN" == "True" ]] || { echo "✗ is_admin=$IS_ADMIN" >&2; exit 1; }

echo "▶ projects: rename PATCH"
assert_http 200 PATCH "$BASE/api/projects/$PROJECT_ID" -H "$AUTH_H" -H 'content-type: application/json' \
    -d '{"name":"Smoke Test Project (renamed)"}'

echo "▶ work_orders: create"
assert_http 201 POST "$BASE/api/projects/$PROJECT_ID/work_orders" -H "$AUTH_H" \
    -H 'content-type: application/json' \
    -d '{"code":"WO-SMK-1","title":"smoke wo"}'
WO_ID="$(jq_get "['id']")"
echo "  wo_id=$WO_ID"

echo "▶ work_orders: list"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/work_orders" -H "$AUTH_H"

echo "▶ photos: multipart upload"
PNG="$WORK_DIR/test.png"
python3 -c "
import struct, zlib, sys
sig=b'\\x89PNG\\r\\n\\x1a\\n'
def chunk(t,d):
    return struct.pack('!I',len(d))+t+d+struct.pack('!I',zlib.crc32(t+d)&0xffffffff)
ihdr=struct.pack('!IIBBBBB',1,1,8,2,0,0,0)
raw=b'\\x00\\xff\\x00\\x00'
idat=zlib.compress(raw)
open(sys.argv[1],'wb').write(sig+chunk(b'IHDR',ihdr)+chunk(b'IDAT',idat)+chunk(b'IEND',b''))
" "$PNG"
assert_http 202 POST "$BASE/api/projects/$PROJECT_ID/photos" -H "$AUTH_H" \
    -F "file=@$PNG;type=image/png" \
    -F "owner_type=wo_raw" \
    -F "wo_id=$WO_ID" \
    -F "angle=front"
PHOTO_ID="$(jq_get "['id']")"
echo "  photo_id=$PHOTO_ID"

echo "▶ photos: list project"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/photos" -H "$AUTH_H"

echo "▶ photos: single resource"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/photos/$PHOTO_ID" -H "$AUTH_H"

echo "▶ recognition: items list"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/recognition_items" -H "$AUTH_H"

echo "▶ recognition: worker drains queue + photo leaves processing"
# Worker should pick up the just-uploaded photo, run the real ONNX pipeline
# end-to-end, and transition the photo out of 'processing' within a few
# seconds. If it doesn't, that's a regression.
drain_ok=0
for i in $(seq 1 30); do
    curl -sS -o "$WORK_DIR/last.body" "$BASE/api/admin/queue/stats" -H "$AUTH_H"
    QP="$(jq_get "['queue_pending']")"
    QL="$(jq_get "['queue_locked']")"
    PP="$(jq_get "['photo_processing']")"
    if [[ "$QP" == "0" && "$QL" == "0" && "$PP" == "0" ]]; then
        drain_ok=1
        break
    fi
    sleep 1
done
if [[ $drain_ok -ne 1 ]]; then
    echo "✗ recognition queue did not drain in 30s (queue_pending=$QP queue_locked=$QL photo_processing=$PP)" >&2
    echo "  last queue stats:" >&2; cat "$WORK_DIR/last.body" >&2 ; echo >&2
    echo "  last 60 lines of server log:" >&2; tail -60 "$LOG" >&2 ; echo >&2
    exit 1
fi
echo "✓ queue drained (queue_pending=0 queue_locked=0 photo_processing=0)"

# Re-fetch the photo and verify it has a terminal status now.
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/photos/$PHOTO_ID" -H "$AUTH_H"
PSTATUS="$(jq_get "['status']")"
case "$PSTATUS" in
    matched|learning|unmatched|failed) echo "✓ photo terminal status: $PSTATUS" ;;
    *) echo "✗ photo still in non-terminal status: $PSTATUS" >&2; exit 1 ;;
esac

# Assert the worker actually wrote a recognition_items row for this photo.
# This guards against silent regressions where the real ONNX pipeline errors
# out and we fall back to `unmatched` without any per-photo detection trail.
# If the real pipeline errors out, the worker marks the photo `unmatched`
# WITHOUT inserting any detection / recognition_items row, so total=0 here
# → this assertion catches that silent regression. (Operators who opt back
# into the stub-fallback path via F1P_INFERENCE_STUB_FALLBACK=1 at runtime
# see the same shape.)
echo "▶ recognition: at least one recognition_items row exists for the uploaded photo"
assert_http 200 GET "$BASE/api/projects/$PROJECT_ID/recognition_items?photo_id=$PHOTO_ID&page_size=5" -H "$AUTH_H"
RI_TOTAL="$(jq_get "['total']")"
echo "  recognition_items.total=$RI_TOTAL (status=$PSTATUS)"
if [[ -z "$RI_TOTAL" || "$RI_TOTAL" -lt 1 ]]; then
    echo "✗ expected recognition_items.total ≥ 1 for photo $PHOTO_ID, got '$RI_TOTAL'" >&2
    echo "  this means the worker did NOT execute the real inference pipeline end-to-end." >&2
    echo "  last 80 lines of server log:" >&2; tail -80 "$LOG" >&2 ; echo >&2
    exit 1
fi
echo "✓ recognition_items row written by worker (real ONNX pipeline executed)"

# Because at least one recognition_items row was produced AND no fallback WARN
# is allowed below (we tighten the WARN whitelist now that DINOv2 is wired),
# the smoke proves the real pipeline ran for this photo end-to-end.

echo "▶ master data: persons"
assert_http 201 POST "$BASE/api/persons" -H "$AUTH_H" -H 'content-type: application/json' \
    -d '{"employee_no":"E-001","name":"Smoke Person"}'
assert_http 200 GET "$BASE/api/persons" -H "$AUTH_H"

echo "▶ master data: tools"
assert_http 201 POST "$BASE/api/tools" -H "$AUTH_H" -H 'content-type: application/json' \
    -d '{"sn":"T-001","name":"Smoke Tool","kind":"wrench"}'

echo "▶ master data: devices"
assert_http 201 POST "$BASE/api/devices" -H "$AUTH_H" -H 'content-type: application/json' \
    -d '{"sn":"D-001","name":"Smoke Device","kind":"laptop"}'

echo "▶ admin: queue stats"
assert_http 200 GET "$BASE/api/admin/queue/stats" -H "$AUTH_H"

echo "▶ admin: ONNX model registry"
assert_http 200 GET "$BASE/api/admin/models" -H "$AUTH_H"
ORT_OK="$(jq_get "['ort_available']")"
READY="$(jq_get "['ready']")"
echo "  ort_available=$ORT_OK ready=$READY"
[[ "$ORT_OK" == "True" ]] || { echo "✗ ort_available is not true (got $ORT_OK)" >&2; exit 1; }
[[ "$READY" == "True" ]] || { echo "✗ model registry not ready (got $READY)" >&2; exit 1; }

echo "▶ settings: get"
assert_http 200 GET "$BASE/api/settings" -H "$AUTH_H"

echo "▶ auth: logout"
assert_http 204 POST "$BASE/api/auth/logout" -H "$AUTH_H"

echo "▶ server log: ERROR / panic check"
# Step C of the turn-23 ONNX rollout: all four production model slots are
# wired with real ONNX weights, so the real pipeline must run cleanly. We no
# longer whitelist the stub-fallback WARN here; if it appears the smoke
# fails so silent inference regressions surface immediately.
if grep -nE ' ERROR | panicked at ' "$LOG" >&2; then
    echo "✗ server log contains ERROR/panic lines" >&2
    exit 1
fi
UNEXPECTED_WARN=$(grep -E ' WARN ' "$LOG" || true)
if [[ -n "$UNEXPECTED_WARN" ]]; then
    echo "✗ server log contains unexpected WARN lines:" >&2
    printf '%s\n' "$UNEXPECTED_WARN" >&2
    exit 1
fi
echo "✓ no unexpected ERROR/WARN/panic in server log"

echo ""
echo "✓✓✓ SMOKE PASSED"
