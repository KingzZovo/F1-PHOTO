#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""
bootstrap_correction_collector.py - F1-photo #5-bootstrap correction-loop
scaffolding.

Pulls unmatched detections from a project (defaults to the wo_raw recognition
path) and emits a flat CSV plus a structured JSON sidecar so a human reviewer
can label them, after which the labelled rows feed the existing
`PATCH /api/projects/<pid>/recognition_items/<id>/correct` endpoint and the
`v_training_corrections` view that the #7a retrain pipeline already consumes.

The persons-bootstrap path (`owner_type=person`) does NOT run YOLOv8, so the
collector is intentionally pointed at the wo_raw / reverse-smoke project where
tool detections actually exist (#2a-real-reverse / #2c-asia evidence: 214
unmatched tool detections at score median ~0.89 / max ~0.95 on the 119-photo
CNNC ID-photo cohort).

This tool is read-only: it talks to bundled-pg via `psql` and writes only the
output files. It does not mutate detections, recognition_items, or any other
table. It is safe to re-run.

Typical usage (CLI defaults match the wo_raw reverse-smoke project):

    python3 tools/bootstrap_correction_collector.py \
        --project-id f54d2a48-537c-4ab3-8f5b-826662465410 \
        --target-type tool \
        --match-status unmatched \
        --out-csv /tmp/corrections-$(date +%Y%m%d).csv \
        --out-json /tmp/corrections-$(date +%Y%m%d).json

Output schema (one row per detection):

    detection_id, photo_id, project_id, project_code, photo_hash,
    photo_path, photo_width, photo_height, target_type, match_status,
    score, class_id, bbox_source, bbox_x1, bbox_y1, bbox_x2, bbox_y2,
    bbox_w_px, bbox_h_px, bbox_area_px, created_at,
    -- reviewer-filled (blank on emit) --
    corrected_label, corrected_owner_type, corrected_owner_id,
    suppress, reviewer, reviewed_at, notes

Author: F1-photo dev relay agent (#5-bootstrap correction-loop scaffolding).
"""
from __future__ import annotations

import argparse
import csv
import dataclasses
import datetime as _dt
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

DEFAULT_PROJECT_ID = "f54d2a48-537c-4ab3-8f5b-826662465410"  # #2a-real-reverse / wo_raw
DEFAULT_PSQL = "/root/F1-photo/bundled-pg/bin/psql"
DEFAULT_PG_HOST = "127.0.0.1"
DEFAULT_PG_PORT = "55444"
DEFAULT_PG_USER = "f1photo"
DEFAULT_PG_DB = "f1photo_prod"
DEFAULT_PG_PASSWORD = "smokepwd"

# CSV / JSON output column order (single source of truth).
DETECTION_FIELDS: List[str] = [
    "detection_id",
    "photo_id",
    "project_id",
    "project_code",
    "photo_hash",
    "photo_path",
    "photo_width",
    "photo_height",
    "target_type",
    "match_status",
    "score",
    "class_id",
    "bbox_source",
    "bbox_x1",
    "bbox_y1",
    "bbox_x2",
    "bbox_y2",
    "bbox_w_px",
    "bbox_h_px",
    "bbox_area_px",
    "created_at",
]
REVIEWER_FIELDS: List[str] = [
    "corrected_label",
    "corrected_owner_type",
    "corrected_owner_id",
    "suppress",
    "reviewer",
    "reviewed_at",
    "notes",
]
ALL_FIELDS: List[str] = DETECTION_FIELDS + REVIEWER_FIELDS


@dataclasses.dataclass
class Row:
    detection_id: int
    photo_id: str
    project_id: str
    project_code: Optional[str]
    photo_hash: Optional[str]
    photo_path: Optional[str]
    photo_width: Optional[int]
    photo_height: Optional[int]
    target_type: str
    match_status: str
    score: float
    class_id: Optional[int]
    bbox_source: Optional[str]
    bbox_x1: float
    bbox_y1: float
    bbox_x2: float
    bbox_y2: float
    bbox_w_px: float
    bbox_h_px: float
    bbox_area_px: float
    created_at: str

    def to_csv_row(self) -> Dict[str, str]:
        out: Dict[str, str] = {}
        for f in DETECTION_FIELDS:
            v = getattr(self, f)
            out[f] = "" if v is None else str(v)
        for f in REVIEWER_FIELDS:
            out[f] = ""
        return out

    def to_json(self) -> Dict[str, Any]:
        d: Dict[str, Any] = dataclasses.asdict(self)
        for f in REVIEWER_FIELDS:
            d[f] = None
        return d


def _psql_query(
    sql: str,
    *,
    psql: str,
    host: str,
    port: str,
    user: str,
    db: str,
    password: str,
) -> str:
    env = os.environ.copy()
    env["PGPASSWORD"] = password
    cmd = [
        psql,
        "-h", host,
        "-p", port,
        "-U", user,
        "-d", db,
        "-A", "-t",         # unaligned, tuples-only
        "-F", "\x1f",       # ASCII unit-separator -> survives JSON / paths
        "-X",                # ignore .psqlrc
        "-v", "ON_ERROR_STOP=1",
        "-c", sql,
    ]
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if proc.returncode != 0:
        sys.stderr.write(
            "psql failed (rc={rc})\n--- stderr ---\n{e}\n".format(
                rc=proc.returncode, e=proc.stderr
            )
        )
        raise SystemExit(2)
    return proc.stdout


def _maybe_int(v: str) -> Optional[int]:
    v = v.strip()
    if not v:
        return None
    try:
        return int(v)
    except ValueError:
        return None


def _maybe_float(v: str) -> Optional[float]:
    v = v.strip()
    if not v:
        return None
    try:
        return float(v)
    except ValueError:
        return None


def _project_code(
    project_id: str,
    *,
    psql: str,
    host: str,
    port: str,
    user: str,
    db: str,
    password: str,
) -> Optional[str]:
    sql = (
        "SELECT code FROM projects WHERE id = '"
        + project_id.replace("'", "''")
        + "';"
    )
    out = _psql_query(
        sql,
        psql=psql, host=host, port=port, user=user, db=db, password=password,
    ).strip()
    return out or None


def collect(
    *,
    project_id: str,
    target_type: str,
    match_status: str,
    min_score: Optional[float],
    max_score: Optional[float],
    limit: Optional[int],
    psql: str,
    host: str,
    port: str,
    user: str,
    db: str,
    password: str,
) -> List[Row]:
    project_code = _project_code(
        project_id,
        psql=psql, host=host, port=port, user=user, db=db, password=password,
    )

    where: List[str] = [
        "d.project_id = '" + project_id.replace("'", "''") + "'",
        "d.target_type = '" + target_type.replace("'", "''") + "'",
        "d.match_status = '" + match_status.replace("'", "''") + "'",
    ]
    if min_score is not None:
        where.append("d.score >= " + repr(float(min_score)))
    if max_score is not None:
        where.append("d.score <= " + repr(float(max_score)))

    sql = (
        "SELECT d.id, d.photo_id::text, d.project_id::text, p.hash, p.path, "
        "p.width, p.height, d.target_type::text, d.match_status::text, "
        "d.score::double precision, "
        "COALESCE((d.bbox->>'class_id'),''), "
        "COALESCE((d.bbox->>'source'),''), "
        "(d.bbox->>'x1')::double precision, "
        "(d.bbox->>'y1')::double precision, "
        "(d.bbox->>'x2')::double precision, "
        "(d.bbox->>'y2')::double precision, "
        "d.created_at "
        "FROM detections d JOIN photos p ON p.id = d.photo_id "
        "WHERE " + " AND ".join(where) + " "
        "ORDER BY d.score DESC, d.id ASC"
    )
    if limit is not None and limit > 0:
        sql += " LIMIT " + str(int(limit))
    sql += ";"

    raw = _psql_query(
        sql,
        psql=psql, host=host, port=port, user=user, db=db, password=password,
    )

    rows: List[Row] = []
    for line in raw.splitlines():
        if not line:
            continue
        parts = line.split("\x1f")
        if len(parts) != 17:
            sys.stderr.write(
                "unexpected psql column count {n} on line: {l!r}\n".format(
                    n=len(parts), l=line
                )
            )
            continue
        (
            det_id, photo_id, proj_id, photo_hash, photo_path,
            width_s, height_s, ttype, mstatus, score_s,
            class_id_s, bbox_source, x1_s, y1_s, x2_s, y2_s, created_at,
        ) = parts
        x1 = float(x1_s)
        y1 = float(y1_s)
        x2 = float(x2_s)
        y2 = float(y2_s)
        w = max(0.0, x2 - x1)
        h = max(0.0, y2 - y1)
        rows.append(
            Row(
                detection_id=int(det_id),
                photo_id=photo_id,
                project_id=proj_id,
                project_code=project_code,
                photo_hash=photo_hash or None,
                photo_path=photo_path or None,
                photo_width=_maybe_int(width_s),
                photo_height=_maybe_int(height_s),
                target_type=ttype,
                match_status=mstatus,
                score=float(score_s),
                class_id=_maybe_int(class_id_s),
                bbox_source=bbox_source or None,
                bbox_x1=x1,
                bbox_y1=y1,
                bbox_x2=x2,
                bbox_y2=y2,
                bbox_w_px=w,
                bbox_h_px=h,
                bbox_area_px=w * h,
                created_at=created_at,
            )
        )
    return rows


def _quantiles(values: List[float]) -> Dict[str, Optional[float]]:
    if not values:
        return {"min": None, "p25": None, "median": None, "p75": None, "max": None, "count": 0}
    s = sorted(values)
    def pct(p: float) -> float:
        if len(s) == 1:
            return s[0]
        idx = (len(s) - 1) * p
        lo = int(idx)
        hi = min(lo + 1, len(s) - 1)
        frac = idx - lo
        return s[lo] + (s[hi] - s[lo]) * frac
    return {
        "min": s[0],
        "p25": pct(0.25),
        "median": statistics.median(s),
        "p75": pct(0.75),
        "max": s[-1],
        "count": len(s),
    }


def summarise(rows: List[Row]) -> Dict[str, Any]:
    score_q = _quantiles([r.score for r in rows])
    area_q = _quantiles([r.bbox_area_px for r in rows])
    class_hist: Dict[str, int] = {}
    photo_hist: Dict[str, int] = {}
    for r in rows:
        k = "" if r.class_id is None else str(r.class_id)
        class_hist[k] = class_hist.get(k, 0) + 1
        photo_hist[r.photo_id] = photo_hist.get(r.photo_id, 0) + 1
    photos_with_at_least_one = len(photo_hist)
    per_photo = (
        len(rows) / photos_with_at_least_one
        if photos_with_at_least_one else 0.0
    )
    return {
        "detection_count": len(rows),
        "distinct_photos": photos_with_at_least_one,
        "detections_per_photo": per_photo,
        "score_quantiles": score_q,
        "bbox_area_px_quantiles": area_q,
        "class_id_histogram": dict(
            sorted(class_hist.items(), key=lambda kv: (-kv[1], kv[0]))
        ),
    }


def write_csv(rows: List[Row], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=ALL_FIELDS)
        w.writeheader()
        for r in rows:
            w.writerow(r.to_csv_row())


def write_json(
    rows: List[Row],
    path: Path,
    *,
    summary: Dict[str, Any],
    args_repr: Dict[str, Any],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema_version": 1,
        "generated_at": _dt.datetime.now(tz=_dt.timezone.utc).isoformat(),
        "args": args_repr,
        "summary": summary,
        "reviewer_fields": REVIEWER_FIELDS,
        "rows": [r.to_json() for r in rows],
    }
    with path.open("w", encoding="utf-8") as fh:
        json.dump(payload, fh, ensure_ascii=False, indent=2)
        fh.write("\n")


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(
        description=(
            "Pull unmatched detections from a project and emit a CSV+JSON "
            "correction worksheet for the #5-bootstrap correction loop."
        )
    )
    p.add_argument("--project-id", default=DEFAULT_PROJECT_ID)
    p.add_argument("--target-type", default="tool",
                   choices=["tool", "face", "device"])
    p.add_argument("--match-status", default="unmatched",
                   choices=["unmatched", "low", "matched"])
    p.add_argument("--min-score", type=float, default=None)
    p.add_argument("--max-score", type=float, default=None)
    p.add_argument("--limit", type=int, default=None)
    p.add_argument("--out-csv", type=Path, default=None,
                   help="Skip if not provided.")
    p.add_argument("--out-json", type=Path, default=None,
                   help="Skip if not provided.")
    p.add_argument("--psql", default=DEFAULT_PSQL)
    p.add_argument("--pg-host", default=DEFAULT_PG_HOST)
    p.add_argument("--pg-port", default=DEFAULT_PG_PORT)
    p.add_argument("--pg-user", default=DEFAULT_PG_USER)
    p.add_argument("--pg-db", default=DEFAULT_PG_DB)
    p.add_argument("--pg-password", default=os.environ.get(
        "PGPASSWORD", DEFAULT_PG_PASSWORD))
    p.add_argument("--summary-only", action="store_true",
                   help="Print summary JSON to stdout, skip writing rows.")
    args = p.parse_args(argv)

    rows = collect(
        project_id=args.project_id,
        target_type=args.target_type,
        match_status=args.match_status,
        min_score=args.min_score,
        max_score=args.max_score,
        limit=args.limit,
        psql=args.psql,
        host=args.pg_host,
        port=args.pg_port,
        user=args.pg_user,
        db=args.pg_db,
        password=args.pg_password,
    )
    summary = summarise(rows)

    args_repr = {
        "project_id": args.project_id,
        "target_type": args.target_type,
        "match_status": args.match_status,
        "min_score": args.min_score,
        "max_score": args.max_score,
        "limit": args.limit,
    }

    if args.summary_only:
        json.dump(
            {"args": args_repr, "summary": summary},
            sys.stdout, ensure_ascii=False, indent=2,
        )
        sys.stdout.write("\n")
        return 0

    if args.out_csv:
        write_csv(rows, args.out_csv)
    if args.out_json:
        write_json(rows, args.out_json, summary=summary, args_repr=args_repr)

    sys.stderr.write(
        "collected {n} rows across {p} photos (per-photo {pp:.4f})\n".format(
            n=summary["detection_count"],
            p=summary["distinct_photos"],
            pp=summary["detections_per_photo"],
        )
    )
    if args.out_csv:
        sys.stderr.write("  csv  -> {}\n".format(args.out_csv))
    if args.out_json:
        sys.stderr.write("  json -> {}\n".format(args.out_json))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
