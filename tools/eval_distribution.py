#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Milestone #2 — real-dataset distribution baseline driver.

What this does
--------------
Unlike `eval_pr.py` (#2c, which needs a labelled fixture to compute Precision
/ Recall), this script is purely descriptive: feed a directory of real photos
through the live recognition pipeline as `owner_type=wo_raw` and capture the
distributions of:

- per-photo `face_count` / `tool_count` / `device_count` (rows in
  `detections` keyed by `target_type`)
- per-photo `recognition_items_total` and a breakdown by `recognition_items.status`
- per-photo final `photos.status` after the worker drains
- score distributions per detect_target (max / median / quantiles of
  `detections.score` across all rows for the bucket)

No ground-truth identities are required. No persons / tools / devices are
seeded. The script exists to characterize what the real ONNX pipeline
actually emits on a curated real-photo set, which is the input that the
operator needs before calibrating thresholds (#3) or replacing the
domain-detector (#5).

The script reuses the same server-boot orchestration as
`packaging/scripts/recognition-pr-baseline.sh`; the companion shell script is
`packaging/scripts/distribution-baseline.sh`.

Usage
-----
  python3 tools/eval_distribution.py \
      --base-url http://127.0.0.1:18799 \
      --admin-user smoke_admin --admin-pwd smoke-admin-pwd-12345 \
      --photos-glob 'tests/fixtures/face/baseline/**/*.jpg' \
      --report-path /tmp/distribution-baseline.json \
      --psql-bin ./bundled-pg/bin/psql \
      --pg-host 127.0.0.1 --pg-port 55444 \
      --pg-user f1photo --pg-db f1photo_prod --pg-pwd smokepwd

Output JSON shape
-----------------
  {
    "meta": {"photo_count": N, "project_id": "...", "work_order_id": "..."},
    "per_photo": [
      {"path": "...", "status": "unmatched", "face_count": 1, "tool_count": 0,
       "device_count": 0, "recognition_items_total": 1,
       "recognition_items_by_status": {"unmatched": 1},
       "max_face_score": 0.93, "max_tool_score": null, "max_device_score": null},
      ...
    ],
    "distributions": {
      "face_count": {"0": k0, "1": k1, "2": k2, "3+": k3},
      "tool_count": {"0": k0, "1": k1, "2+": k2},
      "device_count": {"0": k0, "1": k1, "2+": k2},
      "recognition_items_total": {"0": k0, "1": k1, "2": k2, "3+": k3},
      "photo_status": {"unmatched": x, "matched": y, "learning": z, ...},
      "recognition_items_status": {"unmatched": x, "matched": y, ...},
      "face_score_quantiles": {"min": ..., "p25": ..., "median": ..., "p75": ..., "max": ...},
      "tool_score_quantiles": {...},
      "device_score_quantiles": {...}
    }
  }
"""
from __future__ import annotations

import argparse
import glob
import json
import mimetypes
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple
from uuid import uuid4


# ============================================================================
# HTTP helpers (stdlib only; copied from eval_pr.py for self-containment)
# ============================================================================

class HttpError(Exception):
    def __init__(self, status: int, body: str):
        super().__init__(f"HTTP {status}: {body[:300]}")
        self.status = status
        self.body = body


def _request(method: str, url: str, *, headers=None, data: Optional[bytes] = None,
             timeout: float = 30.0) -> Tuple[int, bytes]:
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        body = e.read() if hasattr(e, "read") else b""
        raise HttpError(e.code, body.decode("utf-8", "replace")) from None


def http_post_json(url: str, token: Optional[str], payload: Dict[str, Any]) -> Dict[str, Any]:
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    status, body = _request("POST", url, headers=headers,
                            data=json.dumps(payload).encode("utf-8"))
    if status >= 300:
        raise HttpError(status, body.decode("utf-8", "replace"))
    return json.loads(body) if body else {}


def http_get_json(url: str, token: Optional[str]) -> Any:
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    status, body = _request("GET", url, headers=headers)
    if status >= 300:
        raise HttpError(status, body.decode("utf-8", "replace"))
    return json.loads(body) if body else None


def http_post_multipart(url: str, token: str, fields: Dict[str, str], file_path: Path,
                        file_field: str = "file") -> Dict[str, Any]:
    boundary = f"----eval-distribution-{uuid4().hex}"
    parts: List[bytes] = []
    for k, v in fields.items():
        parts.append(
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{k}"\r\n\r\n'
            f"{v}\r\n".encode("utf-8")
        )
    mime, _ = mimetypes.guess_type(str(file_path))
    mime = mime or "application/octet-stream"
    file_bytes = file_path.read_bytes()
    parts.append(
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="{file_field}"; filename="{file_path.name}"\r\n'
        f"Content-Type: {mime}\r\n\r\n".encode("utf-8")
    )
    parts.append(file_bytes)
    parts.append(f"\r\n--{boundary}--\r\n".encode("utf-8"))
    body = b"".join(parts)
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": f"multipart/form-data; boundary={boundary}",
        "Content-Length": str(len(body)),
    }
    status, resp = _request("POST", url, headers=headers, data=body, timeout=120.0)
    if status >= 300:
        raise HttpError(status, resp.decode("utf-8", "replace"))
    return json.loads(resp) if resp else {}


# ============================================================================
# psql helper (talks to bundled PG directly to avoid a psycopg2 runtime dep)
# ============================================================================

class PgConn:
    def __init__(self, *, psql_bin: str, host: str, port: int, user: str, db: str,
                 password: str):
        self.psql_bin = psql_bin
        self.host = host
        self.port = port
        self.user = user
        self.db = db
        self.password = password

    def query(self, sql: str) -> List[Dict[str, Any]]:
        env = os.environ.copy()
        env["PGPASSWORD"] = self.password
        cmd = [
            self.psql_bin,
            "-h", self.host, "-p", str(self.port),
            "-U", self.user, "-d", self.db,
            "-At", "-F", "\t",
            "-c", sql,
        ]
        proc = subprocess.run(cmd, capture_output=True, env=env, text=True)
        if proc.returncode != 0:
            raise RuntimeError(f"psql failed (rc={proc.returncode}): {proc.stderr}")
        rows: List[Dict[str, Any]] = []
        # Header-less -At output: caller is expected to know the column order.
        return [line for line in proc.stdout.split("\n") if line]  # raw lines

    def query_columns(self, sql: str, columns: List[str]) -> List[Dict[str, Any]]:
        raw = self.query(sql)
        out: List[Dict[str, Any]] = []
        for line in raw:
            parts = line.split("\t")
            if len(parts) != len(columns):
                raise RuntimeError(
                    f"column count mismatch: got {len(parts)} parts for {len(columns)} columns; line={line!r}"
                )
            row: Dict[str, Any] = {}
            for c, p in zip(columns, parts):
                row[c] = None if p == "" else p
            out.append(row)
        return out


def wait_queue_drained(base_url: str, token: str, *, timeout_s: int = 240,
                       poll_s: float = 1.0) -> None:
    """Poll /api/admin/queue/stats until drained or timeout.

    The endpoint returns a `QueueStats` JSON: `queue_pending`, `queue_locked`,
    `queue_total`, plus per-status photo counts. We consider the queue drained
    when (`queue_pending` + `queue_locked` + `photo_pending` + `photo_processing`)
    is zero for two consecutive polls (debounce against worker race
    conditions).
    """
    deadline = time.time() + timeout_s
    consecutive_zero = 0
    while time.time() < deadline:
        try:
            data = http_get_json(f"{base_url}/api/admin/queue/stats", token)
        except HttpError as e:
            if e.status in (401, 403):
                raise
            time.sleep(poll_s)
            continue
        in_flight = (
            int(data.get("queue_pending", 0))
            + int(data.get("queue_locked", 0))
            + int(data.get("photo_pending", 0))
            + int(data.get("photo_processing", 0))
        )
        if in_flight == 0:
            consecutive_zero += 1
            if consecutive_zero >= 2:
                return
        else:
            consecutive_zero = 0
        time.sleep(poll_s)
    raise TimeoutError(f"queue did not drain within {timeout_s}s")


# ============================================================================
# Distribution computation
# ============================================================================

def _bucket_count(values: List[int], thresholds: List[int], labels: List[str]) -> Dict[str, int]:
    """Bucket a list of integer counts into named buckets.

    `thresholds` is a list of upper-bounds per bucket label, except the last
    label which captures `>= last_threshold`.
    e.g. _bucket_count([0,1,1,2,3,5], [0,1,2], ['0','1','2','3+'])
         -> {'0': 1, '1': 2, '2': 1, '3+': 2}
    """
    out = {label: 0 for label in labels}
    for v in values:
        placed = False
        for i, t in enumerate(thresholds):
            if v == t:
                out[labels[i]] += 1
                placed = True
                break
        if not placed:
            # Falls into final "N+" bucket
            out[labels[-1]] += 1
    return out


def _quantiles(values: List[float]) -> Dict[str, Optional[float]]:
    if not values:
        return {"min": None, "p25": None, "median": None, "p75": None, "max": None}
    s = sorted(values)
    n = len(s)
    def q(p: float) -> float:
        idx = int(round(p * (n - 1)))
        return s[max(0, min(n - 1, idx))]
    return {
        "min": s[0],
        "p25": q(0.25),
        "median": q(0.50),
        "p75": q(0.75),
        "max": s[-1],
    }


def compute_distributions(per_photo: List[Dict[str, Any]],
                          all_face_scores: List[float],
                          all_tool_scores: List[float],
                          all_device_scores: List[float]) -> Dict[str, Any]:
    face_counts = [p["face_count"] for p in per_photo]
    tool_counts = [p["tool_count"] for p in per_photo]
    device_counts = [p["device_count"] for p in per_photo]
    ri_totals = [p["recognition_items_total"] for p in per_photo]

    photo_status_hist: Dict[str, int] = {}
    for p in per_photo:
        photo_status_hist[p["status"]] = photo_status_hist.get(p["status"], 0) + 1

    ri_status_hist: Dict[str, int] = {}
    for p in per_photo:
        for k, v in (p.get("recognition_items_by_status") or {}).items():
            ri_status_hist[k] = ri_status_hist.get(k, 0) + int(v)

    return {
        "face_count": _bucket_count(face_counts, [0, 1, 2], ["0", "1", "2", "3+"]),
        "tool_count": _bucket_count(tool_counts, [0, 1], ["0", "1", "2+"]),
        "device_count": _bucket_count(device_counts, [0, 1], ["0", "1", "2+"]),
        "recognition_items_total": _bucket_count(ri_totals, [0, 1, 2], ["0", "1", "2", "3+"]),
        "photo_status": photo_status_hist,
        "recognition_items_status": ri_status_hist,
        "face_score_quantiles": _quantiles(all_face_scores),
        "tool_score_quantiles": _quantiles(all_tool_scores),
        "device_score_quantiles": _quantiles(all_device_scores),
    }


# ============================================================================
# Main
# ============================================================================

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--admin-user", required=True)
    ap.add_argument("--admin-pwd", required=True)
    ap.add_argument("--photos-glob", required=True,
                    help="glob pattern for input photos (e.g. tests/fixtures/face/baseline/**/*.jpg)")
    ap.add_argument("--report-path", default="/tmp/distribution-baseline.json")
    ap.add_argument("--psql-bin", required=True)
    ap.add_argument("--pg-host", default="127.0.0.1")
    ap.add_argument("--pg-port", type=int, default=5432)
    ap.add_argument("--pg-user", default="f1photo")
    ap.add_argument("--pg-db", default="f1photo_prod")
    ap.add_argument("--pg-pwd", required=True)
    ap.add_argument("--queue-timeout-s", type=int, default=600)
    args = ap.parse_args()

    # 1. Login
    login_resp = http_post_json(
        f"{args.base_url}/api/auth/login", None,
        {"username": args.admin_user, "password": args.admin_pwd},
    )
    token = login_resp["access_token"]
    print(f"✓ logged in as {args.admin_user}")

    # 2. Find photos
    photos = sorted(Path(p) for p in glob.glob(args.photos_glob, recursive=True)
                    if Path(p).is_file())
    if not photos:
        print(f"✗ no photos matched glob: {args.photos_glob}", file=sys.stderr)
        return 2
    print(f"✓ found {len(photos)} photos via glob")

    # 3. Create project + work order
    project_code = f"DIST-{uuid4().hex[:8].upper()}"
    project = http_post_json(
        f"{args.base_url}/api/projects", token,
        {"code": project_code, "name": f"Distribution Baseline {project_code}"},
    )
    project_id = project["id"]
    print(f"✓ project created: {project_id} (code {project_code})")

    wo_code = f"WO-{uuid4().hex[:6].upper()}"
    wo = http_post_json(
        f"{args.base_url}/api/projects/{project_id}/work_orders", token,
        {"code": wo_code, "name": f"Distribution WO {wo_code}"},
    )
    wo_id = wo["id"]
    print(f"✓ work order created: {wo_id} (code {wo_code})")

    # 4. Upload all photos as wo_raw
    print(f"▶ uploading {len(photos)} photos as wo_raw…")
    t0 = time.time()
    uploaded: List[Tuple[str, str]] = []  # (relpath, photo_id)
    repo_root = Path.cwd()
    for i, ph in enumerate(photos, 1):
        try:
            rel = ph.relative_to(repo_root)
        except ValueError:
            rel = ph
        resp = http_post_multipart(
            f"{args.base_url}/api/projects/{project_id}/photos", token,
            {"work_order_id": wo_id, "owner_type": "wo_raw"},
            ph,
        )
        photo_id = resp.get("id") or resp.get("photo_id")
        uploaded.append((str(rel), photo_id))
        if i % 10 == 0 or i == len(photos):
            print(f"  … {i}/{len(photos)} uploaded")
    print(f"✓ uploaded {len(uploaded)} photos in {int(time.time()-t0)}s")

    # 5. Wait for queue to drain
    print("▶ waiting for queue to drain…")
    wait_queue_drained(args.base_url, token, timeout_s=args.queue_timeout_s)
    print("✓ queue drained")

    # 6. Query distributions via psql
    pg = PgConn(
        psql_bin=args.psql_bin, host=args.pg_host, port=args.pg_port,
        user=args.pg_user, db=args.pg_db, password=args.pg_pwd,
    )

    # Per-photo aggregates (one row per uploaded photo)
    sql_per_photo = f"""
      SELECT
        p.id::text,
        p.path,
        p.status::text,
        COALESCE(p.width, 0),
        COALESCE(p.height, 0),
        COALESCE(p.bytes, 0),
        COALESCE(SUM((d.target_type='face')::int), 0)::int   AS face_count,
        COALESCE(SUM((d.target_type='tool')::int), 0)::int   AS tool_count,
        COALESCE(SUM((d.target_type='device')::int), 0)::int AS device_count,
        COALESCE(MAX(CASE WHEN d.target_type='face'   THEN d.score END), -1)::float AS max_face_score,
        COALESCE(MAX(CASE WHEN d.target_type='tool'   THEN d.score END), -1)::float AS max_tool_score,
        COALESCE(MAX(CASE WHEN d.target_type='device' THEN d.score END), -1)::float AS max_device_score
      FROM photos p
      LEFT JOIN detections d ON d.photo_id = p.id
      WHERE p.project_id = '{project_id}' AND p.owner_type = 'wo_raw'
      GROUP BY p.id
      ORDER BY p.path
    """
    rows = pg.query_columns(sql_per_photo, [
        "photo_id", "path", "status", "width", "height", "bytes",
        "face_count", "tool_count", "device_count",
        "max_face_score", "max_tool_score", "max_device_score",
    ])

    # All detection scores for quantile computation
    sql_scores = f"""
      SELECT d.target_type::text, d.score::float
      FROM detections d
      JOIN photos p ON p.id = d.photo_id
      WHERE p.project_id = '{project_id}' AND p.owner_type = 'wo_raw'
    """
    score_rows = pg.query_columns(sql_scores, ["target_type", "score"])
    all_face_scores = [float(r["score"]) for r in score_rows if r["target_type"] == "face"]
    all_tool_scores = [float(r["score"]) for r in score_rows if r["target_type"] == "tool"]
    all_device_scores = [float(r["score"]) for r in score_rows if r["target_type"] == "device"]

    # Recognition_items totals + status breakdown per photo
    sql_ri = f"""
      SELECT p.id::text, COALESCE(ri.status::text, '__none__'), COUNT(ri.id)::int
      FROM photos p
      LEFT JOIN recognition_items ri ON ri.photo_id = p.id
      WHERE p.project_id = '{project_id}' AND p.owner_type = 'wo_raw'
      GROUP BY p.id, ri.status
      ORDER BY p.id
    """
    ri_rows = pg.query_columns(sql_ri, ["photo_id", "status", "count"])
    ri_by_photo: Dict[str, Dict[str, int]] = {}
    for r in ri_rows:
        if r["status"] == "__none__":
            ri_by_photo.setdefault(r["photo_id"], {})
            continue
        ri_by_photo.setdefault(r["photo_id"], {})[r["status"]] = int(r["count"])

    per_photo: List[Dict[str, Any]] = []
    for r in rows:
        ri_map = ri_by_photo.get(r["photo_id"], {})
        ri_total = sum(ri_map.values())
        per_photo.append({
            "path": r["path"],
            "status": r["status"],
            "width": int(r["width"]),
            "height": int(r["height"]),
            "bytes": int(r["bytes"]),
            "face_count": int(r["face_count"]),
            "tool_count": int(r["tool_count"]),
            "device_count": int(r["device_count"]),
            "max_face_score": float(r["max_face_score"]) if float(r["max_face_score"]) >= 0 else None,
            "max_tool_score": float(r["max_tool_score"]) if float(r["max_tool_score"]) >= 0 else None,
            "max_device_score": float(r["max_device_score"]) if float(r["max_device_score"]) >= 0 else None,
            "recognition_items_total": ri_total,
            "recognition_items_by_status": ri_map,
        })

    distributions = compute_distributions(
        per_photo, all_face_scores, all_tool_scores, all_device_scores,
    )

    report = {
        "meta": {
            "photo_count": len(per_photo),
            "project_id": project_id,
            "work_order_id": wo_id,
            "photos_glob": args.photos_glob,
            "thresholds_default": {"low_lower": 0.50, "match_lower": 0.62, "augment_upper": 0.95},
        },
        "per_photo": per_photo,
        "distributions": distributions,
    }

    Path(args.report_path).write_text(json.dumps(report, indent=2, sort_keys=True))
    print(f"✓ report written to {args.report_path}")

    # ---- Human-readable summary ----
    print()
    print("=== DISTRIBUTION BASELINE SUMMARY ===")
    print(f"photos                        : {len(per_photo)}")
    print(f"face_count distribution       : {distributions['face_count']}")
    print(f"tool_count distribution       : {distributions['tool_count']}")
    print(f"device_count distribution     : {distributions['device_count']}")
    print(f"recognition_items_total dist  : {distributions['recognition_items_total']}")
    print(f"photo_status histogram        : {distributions['photo_status']}")
    print(f"recognition_items_status hist : {distributions['recognition_items_status']}")
    print(f"face score quantiles          : {distributions['face_score_quantiles']}")
    print(f"tool score quantiles          : {distributions['tool_score_quantiles']}")
    print(f"device score quantiles        : {distributions['device_score_quantiles']}")
    print("==================================")

    return 0


if __name__ == "__main__":
    sys.exit(main())
