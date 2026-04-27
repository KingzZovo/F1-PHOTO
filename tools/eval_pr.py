#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Milestone #2c — face recognition Precision/Recall baseline driver.

What this does
--------------
1. Reads the fixture MANIFEST (tests/fixtures/face/baseline/MANIFEST.json) to
   know enrolled identities, distractor identities, and per-photo metadata.
2. Logs in to the running server with the smoke admin credentials.
3. Creates a project + work-order, registers each enrolled identity as a
   `person`, and uploads the seed photo for each enrolled identity with
   `owner_type=person` so the worker writes a gallery row.
4. Once the queue drains, uploads every query photo (enrolled and distractor)
   as `owner_type=wo_raw`, letting the worker run SCRFD + ArcFace + recall.
5. Once the queue drains again, queries the `detections` table directly via
   psql to pull `(target_type, match_status, matched_owner_id, matched_score)`
   for every uploaded photo, and joins back to the manifest to compute:
     - overall and per-bucket (western / eastern) precision / recall / F1
     - face-detection rate per bucket (“did SCRFD even produce a face for this
       upscaled Asian crop?”)
     - threshold sweep: replay `recall::Hit::bucket(t)` over
       `(low_lower, match_lower)` grids to see how P/R move.

6. Writes a JSON report to `--report-path` (default `/tmp/pr-baseline.json`)
   and prints a human-readable summary.

Design notes
------------
- This script is intentionally network-only against the local f1photo server
  (so it runs the *real* SCRFD + ArcFace pipeline, not a stub). The companion
  `packaging/scripts/recognition-pr-baseline.sh` boots the server.
- We talk to PostgreSQL directly with the `psql` shipped under
  `bundled-pg/bin/psql` to avoid a python `psycopg2` runtime dep.
- We do NOT modify Thresholds::DEFAULT here. The sweep tells us whether the
  defaults are sensible; any change happens in a follow-up edit to
  `server/src/inference/recall.rs`.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# --------------------------------------------------------------------------
# Tiny HTTP client (stdlib only — keep deps minimal)
# --------------------------------------------------------------------------

class HttpError(Exception):
    def __init__(self, status: int, body: str, url: str):
        super().__init__(f"HTTP {status} from {url}: {body[:300]}")
        self.status = status
        self.body = body
        self.url = url


def _request(method: str, url: str, *, headers=None, data: Optional[bytes] = None,
             timeout: int = 60) -> Tuple[int, bytes, Dict[str, str]]:
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read(), dict(resp.headers)
    except urllib.error.HTTPError as e:
        body = e.read()
        raise HttpError(e.code, body.decode("utf-8", "replace"), url) from None


def http_post_json(url: str, token: Optional[str], payload: Dict[str, Any]) -> Dict[str, Any]:
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    body = json.dumps(payload).encode("utf-8")
    status, raw, _ = _request("POST", url, headers=headers, data=body)
    if status >= 300:
        raise HttpError(status, raw.decode("utf-8", "replace"), url)
    return json.loads(raw) if raw else {}


def http_get_json(url: str, token: Optional[str]) -> Any:
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    status, raw, _ = _request("GET", url, headers=headers)
    if status >= 300:
        raise HttpError(status, raw.decode("utf-8", "replace"), url)
    return json.loads(raw) if raw else None


def http_post_multipart(url: str, token: str, fields: Dict[str, str], file_path: Path,
                         file_field: str = "file") -> Dict[str, Any]:
    """Build a multipart/form-data request without `requests`."""
    boundary = "----f1c2c" + os.urandom(8).hex()
    parts = []
    for k, v in fields.items():
        parts.append(f"--{boundary}\r\n".encode())
        parts.append(f'Content-Disposition: form-data; name="{k}"\r\n\r\n'.encode())
        parts.append(str(v).encode("utf-8"))
        parts.append(b"\r\n")
    parts.append(f"--{boundary}\r\n".encode())
    parts.append((f'Content-Disposition: form-data; name="{file_field}"; '
                  f'filename="{file_path.name}"\r\n').encode())
    parts.append(b"Content-Type: image/jpeg\r\n\r\n")
    parts.append(file_path.read_bytes())
    parts.append(b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    body = b"".join(parts)
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": f"multipart/form-data; boundary={boundary}",
        "Content-Length": str(len(body)),
    }
    status, raw, _ = _request("POST", url, headers=headers, data=body, timeout=120)
    if status >= 300:
        raise HttpError(status, raw.decode("utf-8", "replace"), url)
    return json.loads(raw) if raw else {}


# --------------------------------------------------------------------------
# psql helpers (text protocol, ON_ERROR_STOP)
# --------------------------------------------------------------------------

@dataclass
class PgConn:
    psql_bin: str
    host: str
    port: int
    user: str
    db: str
    password: str

    def query(self, sql: str) -> List[List[str]]:
        cmd = [self.psql_bin, "-h", self.host, "-p", str(self.port), "-U", self.user,
               "-d", self.db, "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-c", sql]
        env = dict(os.environ)
        env["PGPASSWORD"] = self.password
        out = subprocess.check_output(cmd, env=env, text=True)
        rows = []
        for line in out.splitlines():
            if line == "":
                continue
            rows.append(line.split("\t"))
        return rows


# --------------------------------------------------------------------------
# Queue drain
# --------------------------------------------------------------------------

def wait_queue_drained(base_url: str, token: str, *, timeout_s: int = 240,
                       poll_s: float = 1.5) -> Dict[str, Any]:
    deadline = time.time() + timeout_s
    last = None
    while time.time() < deadline:
        last = http_get_json(f"{base_url}/api/admin/queue/stats", token)
        pending = int(last.get("queue_pending", 0))
        locked = int(last.get("queue_locked", 0))
        processing = int(last.get("photo_processing", 0))
        if pending == 0 and locked == 0 and processing == 0:
            return last
        time.sleep(poll_s)
    raise TimeoutError(f"queue did not drain within {timeout_s}s; last stats={last}")


# --------------------------------------------------------------------------
# Threshold sweep — mirrors recall::Hit::bucket
# --------------------------------------------------------------------------

def bucket(score: Optional[float], low_lower: float, match_lower: float) -> str:
    if score is None:
        return "unmatched"
    if score >= match_lower:
        return "matched"
    if score >= low_lower:
        return "learning"
    return "unmatched"


# --------------------------------------------------------------------------
# Main eval
# --------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default=os.environ.get("F1C_BASE_URL", "http://127.0.0.1:18799"))
    ap.add_argument("--admin-user", default=os.environ.get("F1C_ADMIN_USER", "smoke_admin"))
    ap.add_argument("--admin-pwd", default=os.environ.get("F1C_ADMIN_PWD", "smoke-admin-pwd-12345"))
    ap.add_argument("--manifest", default="tests/fixtures/face/baseline/MANIFEST.json")
    ap.add_argument("--report-path", default="/tmp/pr-baseline.json")
    ap.add_argument("--psql-bin", default=os.environ.get("F1C_PSQL_BIN", "./bundled-pg/bin/psql"))
    ap.add_argument("--pg-host", default="127.0.0.1")
    ap.add_argument("--pg-port", type=int, default=int(os.environ.get("F1C_PG_PORT", "55444")))
    ap.add_argument("--pg-user", default="f1photo")
    ap.add_argument("--pg-db", default="f1photo_prod")
    ap.add_argument("--pg-pwd", default=os.environ.get("F1P_BUNDLED_PG_PASSWORD", "smokepwd"))
    args = ap.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    manifest_path = (repo_root / args.manifest).resolve()
    if not manifest_path.exists():
        print(f"ERROR: manifest not found at {manifest_path}", file=sys.stderr)
        return 2
    manifest = json.loads(manifest_path.read_text())

    pg = PgConn(
        psql_bin=args.psql_bin if Path(args.psql_bin).is_absolute() else str((repo_root / args.psql_bin).resolve()),
        host=args.pg_host, port=args.pg_port, user=args.pg_user, db=args.pg_db, password=args.pg_pwd,
    )

    base = args.base_url.rstrip("/")
    print(f"[eval_pr] base_url={base}")
    print(f"[eval_pr] manifest={manifest_path}")
    print(f"[eval_pr] psql={pg.psql_bin} pg=postgresql://{pg.user}@{pg.host}:{pg.port}/{pg.db}")

    # 1) Login
    tok = http_post_json(f"{base}/api/auth/login", None,
                         {"username": args.admin_user, "password": args.admin_pwd})
    token = tok["access_token"]
    print("[eval_pr] logged in as", args.admin_user)

    # 2) Project + WO
    pj = http_post_json(f"{base}/api/projects", token, {
        "code": "P-2C-PR",
        "name": "#2c P/R baseline",
        "icon": "🎯",
        "description": "Milestone #2c face recognition Precision/Recall baseline",
    })
    project_id = pj["id"]
    print(f"[eval_pr] created project {project_id}")
    wo = http_post_json(f"{base}/api/projects/{project_id}/work_orders", token, {
        "code": "WO-2C",
        "title": "baseline WO",
    })
    wo_id = wo["id"]
    print(f"[eval_pr] created WO {wo_id}")

    # 3) Persons + seed uploads
    enrolled_roster = manifest["enrolled_roster"]
    files = manifest["files"]
    seed_files = {f["identity_slug"]: f for f in files if f["role"] == "seed" and f["enrolled"]}
    persons: Dict[str, str] = {}  # slug -> person_id
    photo_ids: List[Tuple[str, Dict[str, Any]]] = []  # (photo_id, meta)

    for r in enrolled_roster:
        slug = r["slug"]
        person = http_post_json(f"{base}/api/persons", token, {
            "employee_no": r["employee_no"],
            "name": r["display"],
        })
        persons[slug] = person["id"]
        # Upload seed for this person
        seed_meta = seed_files[slug]
        seed_path = (repo_root / seed_meta["path"]).resolve()
        photo = http_post_multipart(
            f"{base}/api/projects/{project_id}/photos", token,
            fields={"owner_type": "person", "owner_id": person["id"]},
            file_path=seed_path,
        )
        photo_ids.append((photo["id"], {**seed_meta, "phase": "seed", "expected_slug": slug}))
        print(f"[eval_pr] seed uploaded for {slug} ({r['bucket']})")

    # Drain queue (seed pipeline)
    print("[eval_pr] waiting for seed queue to drain...")
    drained = wait_queue_drained(base, token, timeout_s=240)
    print(f"[eval_pr] seed drained: {drained}")

    # Sanity: how many seed detections produced?
    rows = pg.query(
        f"SELECT photo_id, COUNT(*) FROM detections WHERE project_id = '{project_id}'"
        " AND target_type = 'face' GROUP BY photo_id")
    print(f"[eval_pr] seed detections in DB (per photo): {len(rows)} rows")

    # 4) Query uploads (enrolled queries + distractor queries)
    query_files = [f for f in files if f["role"] == "query"]
    for f in query_files:
        path = (repo_root / f["path"]).resolve()
        photo = http_post_multipart(
            f"{base}/api/projects/{project_id}/photos", token,
            fields={"owner_type": "wo_raw", "wo_id": wo_id, "angle": "front"},
            file_path=path,
        )
        expected_slug = f["identity_slug"] if f["enrolled"] else None
        photo_ids.append((photo["id"], {**f, "phase": "query", "expected_slug": expected_slug}))
    print(f"[eval_pr] uploaded {len(query_files)} query photos; waiting for queue...")
    drained = wait_queue_drained(base, token, timeout_s=600)
    print(f"[eval_pr] query drained: {drained}")

    # 5) Pull detections per photo
    photo_id_to_meta = {pid: meta for pid, meta in photo_ids}
    quoted = ",".join("'" + pid.replace("'", "''") + "'" for pid, _ in photo_ids)
    detection_rows = pg.query(
        "SELECT photo_id::text, target_type::text, match_status::text,"
        " COALESCE(matched_owner_id::text, ''),"
        " COALESCE(to_char(matched_score, 'FM999990.000000'), '')"
        f" FROM detections WHERE project_id = '{project_id}' AND photo_id IN ({quoted})"
    )
    # Map person_id -> slug to interpret matched_owner_id
    person_id_to_slug = {pid: slug for slug, pid in persons.items()}

    # Aggregate face detections per photo (we keep all; for this fixture each
    # photo is a single tightly-cropped face so we expect <= 1 face detection).
    by_photo: Dict[str, List[Dict[str, Any]]] = {}
    for row in detection_rows:
        photo_id, target_type, status, owner_id_str, score_str = row
        if target_type != "face":
            continue  # this baseline focuses on face recall
        score = float(score_str) if score_str else None
        owner_slug = person_id_to_slug.get(owner_id_str) if owner_id_str else None
        by_photo.setdefault(photo_id, []).append({
            "match_status": status,
            "matched_owner_id": owner_id_str or None,
            "matched_owner_slug": owner_slug,
            "matched_score": score,
        })

    # 6) Compute P/R at server's actual Thresholds::DEFAULT.
    # NOTE: keep this tuple in sync with server/src/inference/recall.rs
    # `Thresholds::DEFAULT { low_lower, match_lower, augment_upper }`.
    # Updated for milestone #2c-tune: was (0.50, 0.62), now (0.30, 0.40).
    DEFAULT = (0.30, 0.40)  # mirrors Thresholds::DEFAULT { low_lower, match_lower }

    @dataclass
    class PerPhoto:
        photo_id: str
        phase: str
        bucket: str  # western | eastern
        enrolled: bool
        expected_slug: Optional[str]
        face_count: int
        top1_status: Optional[str]
        top1_owner_slug: Optional[str]
        top1_score: Optional[float]

    per_photo: List[PerPhoto] = []
    for pid, meta in photo_ids:
        if meta["phase"] != "query":
            continue
        dets = by_photo.get(pid, [])
        # Pick the highest-score detection as top1 (if any)
        top1 = max(dets, key=lambda d: d["matched_score"] or -1.0) if dets else None
        per_photo.append(PerPhoto(
            photo_id=pid,
            phase=meta["phase"],
            bucket=meta["bucket"],
            enrolled=meta["enrolled"],
            expected_slug=meta["expected_slug"],
            face_count=len(dets),
            top1_status=top1["match_status"] if top1 else None,
            top1_owner_slug=top1["matched_owner_slug"] if top1 else None,
            top1_score=top1["matched_score"] if top1 else None,
        ))

    def compute_pr(samples: List[PerPhoto], low_lower: float, match_lower: float) -> Dict[str, Any]:
        tp = fp = fn = tn = 0
        face_detected = 0
        for s in samples:
            face_detected += int(s.face_count > 0)
            # Replay bucket using top1_score
            decided = bucket(s.top1_score, low_lower, match_lower)
            decided_slug = s.top1_owner_slug if decided == "matched" else None
            if s.enrolled:
                if decided_slug == s.expected_slug:
                    tp += 1
                elif decided == "matched" and decided_slug != s.expected_slug:
                    fp += 1  # matched the wrong identity
                    fn += 1  # also missed the correct one
                else:
                    fn += 1  # learning / unmatched / no face
            else:  # distractor
                if decided == "matched":
                    fp += 1
                else:
                    tn += 1
        precision = tp / (tp + fp) if (tp + fp) > 0 else None
        recall = tp / (tp + fn) if (tp + fn) > 0 else None
        f1 = (2 * precision * recall / (precision + recall)) if precision and recall else None
        return {
            "low_lower": low_lower,
            "match_lower": match_lower,
            "tp": tp, "fp": fp, "fn": fn, "tn": tn,
            "face_detection_rate": face_detected / len(samples) if samples else None,
            "precision": precision, "recall": recall, "f1": f1,
            "n": len(samples),
        }

    overall = compute_pr(per_photo, *DEFAULT)
    western = compute_pr([s for s in per_photo if s.bucket == "western"], *DEFAULT)
    eastern = compute_pr([s for s in per_photo if s.bucket == "eastern"], *DEFAULT)

    # Threshold sweep. Pin low_lower at the new 0.30 floor (matching the
    # post-#2c-tune default) and walk match_lower up. Lower endpoints (0.30,
    # 0.35) are added so the sweep brackets the new default symmetrically.
    sweep_match_lower = [0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.62, 0.65, 0.70, 0.75, 0.80]
    sweep_low_floor = DEFAULT[0]
    sweep = []
    for ml in sweep_match_lower:
        ll = min(sweep_low_floor, ml)  # keep low_lower <= match_lower
        sweep.append(compute_pr(per_photo, ll, ml))

    report = {
        "milestone": "#2c-tune face recognition P/R at retuned Thresholds::DEFAULT",
        "thresholds_default": {"low_lower": DEFAULT[0], "match_lower": DEFAULT[1], "augment_upper": 0.95},
        "manifest_path": str(manifest_path.relative_to(repo_root)),
        "project_id": project_id, "wo_id": wo_id,
        "counts": {
            "enrolled_identities": len(enrolled_roster),
            "distractor_identities": len(manifest["distractor_roster"]),
            "query_photos": len(per_photo),
            "seed_photos": len(enrolled_roster),
        },
        "overall_at_default": overall,
        "per_bucket_at_default": {"western": western, "eastern": eastern},
        "threshold_sweep": sweep,
        "per_photo": [
            {
                "photo_id": s.photo_id, "bucket": s.bucket, "enrolled": s.enrolled,
                "expected_slug": s.expected_slug,
                "face_count": s.face_count,
                "top1_status": s.top1_status,
                "top1_owner_slug": s.top1_owner_slug,
                "top1_score": s.top1_score,
            } for s in per_photo
        ],
    }

    Path(args.report_path).write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(f"[eval_pr] wrote report to {args.report_path}")

    print(f"\n=== SUMMARY (Thresholds::DEFAULT low={DEFAULT[0]:.2f} match={DEFAULT[1]:.2f}) ===")
    for label, blk in ("overall", overall), ("western", western), ("eastern", eastern):
        print(f"  {label:8} n={blk['n']:>2}  P={blk['precision']}  R={blk['recall']}  F1={blk['f1']}"
              f"  TP={blk['tp']} FP={blk['fp']} FN={blk['fn']} TN={blk['tn']}"
              f"  face_det_rate={blk['face_detection_rate']}")
    print(f"\n=== THRESHOLD SWEEP (low_lower fixed at {sweep_low_floor:.2f} floor) ===")
    print("  match_lower  P        R        F1       TP  FP  FN  TN")
    for s in sweep:
        print(f"  {s['match_lower']:.2f}         {s['precision']!s:<8} {s['recall']!s:<8} {s['f1']!s:<8} "
              f"{s['tp']:>2}  {s['fp']:>2}  {s['fn']:>2}  {s['tn']:>2}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
