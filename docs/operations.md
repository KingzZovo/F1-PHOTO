# F1-Photo Operations Runbook

This runbook covers day-2 operations for an offline F1-Photo deployment:
install, upgrade, backup/restore, secret rotation, model upgrade, and
incident response. It targets the release tarball produced by
`packaging/scripts/build-release.sh` (Linux) or `build-release.ps1`
(Windows).

For architecture and API references see:

- `docs/architecture.md`
- `docs/api.md`
- `docs/data_model.md`
- `docs/deployment.md` (high-level deployment narrative)
- `docs/recognition_pipeline.md`
- `docs/training.md`

---

## 1. Topology and runtime layout

```
/opt/f1photo/                          # PREFIX (immutable per release)
  f1photo                              # rust binary (SPA embedded via include_dir)
  migrations/                          # sqlx migrations applied on each boot
  models/                              # ONNX INT8 models (face_detect.onnx ...)
  runtime/libonnxruntime.so.1.18.0     # ORT dylib pointed to by ORT_DYLIB_PATH
  bundled-pg/{bin,lib,share}/...       # portable PostgreSQL 16 + pgvector
  data/                                # photo blob storage (yyyy/mm/dd/sha256.jpg)

/opt/f1photo/bundled-pg-data/          # PG cluster (mutable, NOT in PREFIX)
/etc/f1photo/env                       # systemd EnvironmentFile (chmod 600)
/var/log/f1photo/                      # journald supplements + worker logs
```

- `/opt/f1photo` is overwritten on upgrade; never put runtime state here.
- `/opt/f1photo/bundled-pg-data` is the PG cluster. Treat it like a database.
- `/opt/f1photo/data` holds uploaded photos, addressed by SHA-256.

---

## 2. First-time install (Linux)

```bash
# As root on the target host:
tar xzf f1photo-0.1.0-linux-x86_64.tar.gz
cd f1photo-0.1.0-linux
sudo ./packaging/linux/install.sh
sudo cp packaging/linux/env.example /etc/f1photo/env
sudo $EDITOR /etc/f1photo/env          # set F1P_JWT_SECRET, bundled PG password, etc.
sudo systemctl enable --now f1photo
sudo journalctl -u f1photo -f          # watch boot

# Bootstrap the first admin (interactive prompt for password).
sudo -u f1photo /opt/f1photo/f1photo bootstrap-admin --username admin
```

After `systemctl start`, the binary will:

1. spin up bundled PG on `127.0.0.1:5544` if `F1P_USE_BUNDLED_PG=1` (initdb on first run);
2. apply `migrations/` (idempotent);
3. load ORT models from `models/` (continues with `inference_ready=false` if absent);
4. start the recognition worker;
5. listen on `F1P_BIND` (default `0.0.0.0:8080`).

Verify: `curl -fsS http://127.0.0.1:8080/healthz` should return
`{"status":"ok","version":"..."}`.

---

## 3. First-time install (Windows)

```powershell
# As Administrator from inside the unzipped release folder:
.\packaging\windows\install.cmd
# Edits %ProgramFiles%\F1Photo\env.cmd, then:
net start F1Photo
```

NSSM logs go to `%ProgramFiles%\F1Photo\logs\f1photo.{out,err}.log`.
Use `nssm edit F1Photo` to change service parameters (e.g. CPU affinity).

---

## 4. Upgrade (Linux)

```bash
systemctl stop f1photo
cd /tmp && tar xzf f1photo-NEW.tar.gz
sudo ./packaging/linux/install.sh    # rsync --delete preserves /etc + bundled-pg-data
systemctl start f1photo
journalctl -u f1photo --since '5 minutes ago' | grep -E 'migrations|listening|panic'
```

The install script does **not** touch:

- `/etc/f1photo/env` (env file is preserved as long as it exists)
- `/opt/f1photo/bundled-pg-data` (excluded from rsync)
- `/opt/f1photo/data` (photos)

Migrations run on next boot. If a migration fails, the service refuses to
start and logs the SQL error — do **not** delete the migration row from
`_sqlx_migrations` to force-skip; instead, restore the pre-upgrade backup.

### Rollback

1. `systemctl stop f1photo`
2. Re-extract the previous release tarball over `/opt/f1photo`.
3. Restore `bundled-pg-data` from the latest backup (see §5) if the new
   release applied a destructive migration.
4. `systemctl start f1photo`.

---

## 5. Backup and restore

### What to back up

| Path                               | Contents                          | Frequency  |
|------------------------------------|-----------------------------------|------------|
| `/opt/f1photo/bundled-pg-data`     | PG cluster (or use `pg_dump`)     | daily      |
| `/opt/f1photo/data`                | Photo blobs (sha256-addressed)    | weekly     |
| `/etc/f1photo/env`                 | secrets, env vars                 | on change  |
| `/opt/f1photo/models`              | ONNX models (versioned in git LFS)| on upgrade |

### Logical backup (recommended)

```bash
# Run as the f1photo user:
sudo -u f1photo /opt/f1photo/bundled-pg/bin/pg_dump \
    -h 127.0.0.1 -p 5544 -U f1photo -d f1photo_prod -Fc \
    -f /var/backups/f1photo-$(date +%F).dump
```

Retain 14 daily dumps. Logical dumps are version-portable and survive PG
minor upgrades.

### Physical backup (faster, version-locked)

```bash
systemctl stop f1photo
tar czf /var/backups/f1photo-pgdata-$(date +%F).tgz \
    -C /opt/f1photo bundled-pg-data
systemctl start f1photo
```

### Photo blob backup

```bash
rsync -a --delete /opt/f1photo/data/ backup-host:/srv/f1photo-photos/
```

Photos are content-addressed, so rsync incrementals are tiny.

### Restore

Logical:

```bash
systemctl stop f1photo
sudo -u f1photo /opt/f1photo/bundled-pg/bin/dropdb -h 127.0.0.1 -p 5544 -U f1photo f1photo_prod
sudo -u f1photo /opt/f1photo/bundled-pg/bin/createdb -h 127.0.0.1 -p 5544 -U f1photo f1photo_prod
sudo -u f1photo /opt/f1photo/bundled-pg/bin/pg_restore \
    -h 127.0.0.1 -p 5544 -U f1photo -d f1photo_prod /var/backups/f1photo-DATE.dump
systemctl start f1photo
```

Physical:

```bash
systemctl stop f1photo
rm -rf /opt/f1photo/bundled-pg-data
tar xzf /var/backups/f1photo-pgdata-DATE.tgz -C /opt/f1photo
chown -R f1photo:f1photo /opt/f1photo/bundled-pg-data
systemctl start f1photo
```

---

## 6. Secret rotation

### `F1P_JWT_SECRET`

Rotating the JWT secret invalidates **every** active session and stored
refresh token. Schedule a maintenance window.

```bash
NEW=$(head -c 32 /dev/urandom | base64 | tr -dc A-Za-z0-9 | head -c 32)
sudo sed -i "s/^F1P_JWT_SECRET=.*/F1P_JWT_SECRET=$NEW/" /etc/f1photo/env
sudo systemctl restart f1photo
```

All users will be bounced to the login screen; no client-side action needed.

### Bundled PG password

```bash
sudo -u f1photo /opt/f1photo/bundled-pg/bin/psql -h 127.0.0.1 -p 5544 \
    -U f1photo -d f1photo_prod \
    -c "ALTER ROLE f1photo WITH PASSWORD 'new-strong-password';"
sudo sed -i "s/^F1P_BUNDLED_PG_PASSWORD=.*/F1P_BUNDLED_PG_PASSWORD=new-strong-password/" /etc/f1photo/env
sudo systemctl restart f1photo
```

If the bundled PG password is lost: stop the service, run `postgres` with
`pg_hba.conf` set to `trust`, `ALTER ROLE`, restore `pg_hba.conf`.

### Admin user password reset

```bash
sudo -u f1photo /opt/f1photo/f1photo reset-password --username admin
# Prompts for the new password, hashes with argon2id, writes to users table.
```

---

## 7. ONNX model upgrade

Models live under `/opt/f1photo/models/`. The registry expects:

- `face_detect.onnx`     (required)
- `face_embed.onnx`      (required, 512-d output)
- `object_detect.onnx`   (required)
- `generic_embed.onnx`   (required, 512-d output)
- `text_embed.onnx`      (optional)

Upgrade procedure:

```bash
systemctl stop f1photo
cp NEW_face_embed.onnx /opt/f1photo/models/face_embed.onnx
chown f1photo:f1photo /opt/f1photo/models/face_embed.onnx
systemctl start f1photo
sudo -u f1photo curl -fsS \
    -H "Authorization: Bearer $ADMIN_JWT" \
    http://127.0.0.1:8080/api/admin/models | jq
```

After swapping a face/embed model, **all stored embeddings become
incompatible**. Re-run the finetune CLI to recompute identity embeddings:

```bash
sudo -u f1photo /opt/f1photo/f1photo finetune \
    --project-id 00000000-0000-0000-0000-000000000001
```

If you swap an ONNX runtime version (1.18 → 1.19), update
`runtime/libonnxruntime.so.*` and `ORT_DYLIB_PATH` in `/etc/f1photo/env`.

---

## 8. Capacity planning and tuning

Reference hardware: 10C/20T, 24 GB RAM, no GPU.

| Knob                          | Default | Notes                                 |
|-------------------------------|---------|---------------------------------------|
| `F1P_INFERENCE_THREADS`       | 4       | per-Session intra-op threads          |
| Worker concurrency            | 1       | LISTEN/NOTIFY single consumer; raise via systemd templating |
| `F1P_MAX_UPLOAD_MB`           | 20      | per-photo limit; multipart enforced   |
| `F1P_BUNDLED_PG_PORT`         | 5544    | avoid conflict with system PG (5432)  |
| `shared_buffers` (PG)         | 4 GB    | tune in `bundled-pg-data/postgresql.conf` |

Thresholds (recognition_pipeline.md):

- match: 0.62 (cosine)
- low:   0.50
- augment_upper: 0.95 (above this, the embedding is added back to identity samples)

---

## 9. Incident playbook

### `/healthz` is OK but `/readyz` is failing

- `/readyz` returns 503 when ORT models are missing or PG is unreachable.
- Check `journalctl -u f1photo` for `inference_ready=false` and missing model names.
- Confirm `ORT_DYLIB_PATH` is set and the file exists.

### Worker is up but recognition_items stay `pending`

```bash
sudo -u f1photo curl -fsS \
    -H "Authorization: Bearer $ADMIN_JWT" \
    http://127.0.0.1:8080/api/admin/queue/stats | jq
```

Look for `claimed`/`backoff_until`. If the worker keeps backing off, the
likely cause is a model load failure (see above) or a DB row lock.

### Photo upload returns 413

Increase `F1P_MAX_UPLOAD_MB`, restart the service, and re-upload.
Nginx (if fronting) needs `client_max_body_size` raised to match.

### Bundled PG fails to start (port in use)

```bash
ss -tnlp | grep 5544
# Either kill the squatter or change F1P_BUNDLED_PG_PORT in /etc/f1photo/env.
```

### Disk filling up under `/opt/f1photo/data`

Photo blobs are deduplicated by SHA-256. Re-uploads do not duplicate. To
reclaim space, soft-delete photos via the API and run the GC CLI (TBD).
For emergency cleanup, dump SHA-256s of soft-deleted photos and delete
their blobs:

```bash
psql ... -c "COPY (SELECT sha256 FROM photos WHERE deleted_at IS NOT NULL) TO STDOUT" \
    | while read sha; do
        f=/opt/f1photo/data/${sha:0:2}/${sha:2:2}/$sha.jpg
        [[ -f $f ]] && rm -- "$f"
      done
```

### `panic: ONNX Runtime not found`

The binary degrades to `inference_ready=false` and continues serving
uploads + manual workflows. Set `ORT_DYLIB_PATH` and restart.

---

## 10. Observability

- All logs are tracing JSON, written to stdout. journald captures them.
- Notable spans: `bundled_pg.bootstrap`, `migrations.apply`,
  `worker.claim_one`, `worker.run_inference`, `recognition.upsert_item`.
- Add Prometheus scraping later via a sidecar exporter; the binary itself
  exposes no `/metrics` endpoint today.

---

## 11. Useful one-liners

```bash
# Tail recognition queue depth
watch -n5 'sudo -u f1photo /opt/f1photo/bundled-pg/bin/psql -h 127.0.0.1 -p 5544 -U f1photo -d f1photo_prod -c "SELECT status, COUNT(*) FROM recognition_items GROUP BY 1 ORDER BY 1;"'

# Force re-enqueue all unmatched items in a project
psql ... -c "UPDATE photos SET status='pending' WHERE project_id='<uuid>' AND status='unmatched';"
sudo systemctl restart f1photo   # worker picks up via LISTEN/NOTIFY on next NOTIFY

# Show audit log for a project (last 50)
psql ... -c "SELECT created_at, actor, action, target_kind, target_id, summary FROM audit_log WHERE project_id='<uuid>' ORDER BY created_at DESC LIMIT 50;"
```

---

Last updated: with release v0.1.0.
