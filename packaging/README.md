# F1-Photo packaging

This directory holds everything needed to ship the offline release artefacts
for the work-order photo archive system.

## Layout

```
packaging/
  linux/
    f1photo.service     # systemd unit (Type=simple, runs as f1photo:f1photo)
    install.sh          # one-shot installer for the unpacked tarball
    env.example         # template /etc/f1photo/env
  windows/
    install.cmd         # NSSM-based service installer (run as Administrator)
    env.example.cmd     # template environment block
    nssm.exe            # NOT vendored; place an NSSM 2.24+ binary here
  scripts/
    build-release.sh    # Linux: web build + cargo --release + tar.gz
    build-release.ps1   # Windows: web build + cargo --release + zip
```

## Release flow

1. **Build the SPA** (`web/dist`) with Vite — picked up by `rust-embed` at
   compile time of the server crate (see `server/src/static_assets.rs`).
2. **Build the binary**: `cargo build --release --bin f1photo`. The release
   binary embeds the SPA and serves it from any non-`/api` path.
3. **Stage external assets** under the repo root before running the
   `build-release.*` scripts:
   - `models/` — ONNX INT8 models (`face_detect.onnx`,
     `face_recognize.onnx`, …). The runtime is hot-loaded via `ORT_DYLIB_PATH`.
   - `runtime/` — ONNX Runtime 1.18 dynamic library
     (`libonnxruntime.so.1.18.0` or `onnxruntime.dll`).
   - `bundled-pg/` — portable PostgreSQL 16 tree with pgvector. We expect
     `bundled-pg/bin/{initdb,postgres,psql}` (or `.exe` on Windows).
4. **Package**:
   - Linux: `packaging/scripts/build-release.sh` -> `dist/f1photo-<v>-linux-x86_64.tar.gz`
   - Windows: `packaging/scripts/build-release.ps1` -> `dist/f1photo-<v>-windows-x86_64.zip`
5. **Install on target**:
   - Linux: `tar -xzf …`, `sudo packaging/linux/install.sh`, edit `/etc/f1photo/env`,
     `systemctl enable --now f1photo`.
   - Windows: unzip into a working directory, run `packaging\windows\install.cmd`
     as Administrator. NSSM keeps the service running and rotates logs.

## Bundled Postgres bootstrap

When `F1P_USE_BUNDLED_PG=1`, the server (see `server/src/bundled_pg.rs`):

1. `initdb`s `F1P_BUNDLED_PG_DATA` if the cluster does not yet exist.
2. Spawns `postgres` as a child on `127.0.0.1:F1P_BUNDLED_PG_PORT`
   (default `5544`).
3. Writes `postgresql.conf` and `pg_hba.conf` for loopback-only access with
   `shared_preload_libraries = 'vector'` (pgvector).
4. Auto-derives `F1P_DATABASE_URL` if not already set, then `db::migrate`
   runs the embedded sqlx migrations.

When `F1P_USE_BUNDLED_PG` is unset, the server uses whatever
`F1P_DATABASE_URL` you provide (e.g. an external PG cluster).

## Smoke test after install

```bash
curl -fs http://127.0.0.1:8080/healthz
curl -fs http://127.0.0.1:8080/readyz
curl -fs http://127.0.0.1:8080/   # HTML index from the embedded SPA
```

The SPA is served from `/`, the API from `/api/...`, and `/healthz` /
`/readyz` are reserved.
