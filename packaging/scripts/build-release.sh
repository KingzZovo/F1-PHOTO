#!/usr/bin/env bash
# Build a Linux release tarball:
#   ./packaging/scripts/build-release.sh [--target x86_64-unknown-linux-gnu]
#
# Output: dist/f1photo-${VERSION}-linux-x86_64.tar.gz
# Layout (inside tarball):
#   payload/
#     f1photo                     # statically-linked-ish binary (rust-embed has the SPA)
#     migrations/                 # sqlx migrations (rust-embed could also pick these up later)
#     models/                     # ONNX INT8 models (face_detect.onnx etc.)
#     runtime/libonnxruntime.so*  # ORT 1.18 dylib
#     bundled-pg/                 # portable PG 16 + pgvector tree
#   packaging/                    # systemd unit + install.sh + env.example
#   docs/                         # operator runbook
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

VERSION=$(grep '^version' server/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
TARGET=${TARGET:-x86_64-unknown-linux-gnu}

export PATH=/root/.cargo/bin:$PATH

echo "[1/4] building Vue 3 web/dist"
pushd web >/dev/null
npm install --no-audit --no-fund
npx vite build
popd >/dev/null

echo "[2/4] cargo build --release --target $TARGET"
pushd server >/dev/null
touch src/api/*.rs
cargo build --release --target "$TARGET" --bin f1photo
popd >/dev/null

DIST=$ROOT/dist/f1photo-$VERSION-linux
rm -rf "$DIST"
mkdir -p "$DIST/payload" "$DIST/packaging" "$DIST/docs"

echo "[3/4] assembling payload at $DIST"
cp "server/target/$TARGET/release/f1photo" "$DIST/payload/"
cp -R server/migrations "$DIST/payload/"
mkdir -p "$DIST/payload/models" "$DIST/payload/runtime" "$DIST/payload/bundled-pg" "$DIST/payload/data" "$DIST/payload/logs"
if [[ -d models ]]; then cp -R models/. "$DIST/payload/models/"; fi
if [[ -d runtime ]]; then cp -R runtime/. "$DIST/payload/runtime/"; fi
if [[ -d bundled-pg ]]; then cp -R bundled-pg/. "$DIST/payload/bundled-pg/"; fi
cp -R packaging/linux "$DIST/packaging/"
cp -R packaging/scripts "$DIST/packaging/"
if [[ -d docs ]]; then cp -R docs/. "$DIST/docs/"; fi

echo "[4/4] tarballing"
tar -C "$ROOT/dist" -czf "$ROOT/dist/f1photo-$VERSION-linux-x86_64.tar.gz" \
    "$(basename "$DIST")"
echo "OK -> dist/f1photo-$VERSION-linux-x86_64.tar.gz"
