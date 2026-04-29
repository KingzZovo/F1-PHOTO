#!/usr/bin/env python3
"""Import labelled corrections from a `bootstrap_correction_collector.py` CSV
into the running f1photo server via
  PATCH /api/projects/<project_id>/recognition_items/<id>/correct

This is the second half of the >=50-correction accumulation gate that
#5-bootstrap (retrain) is queued behind. The collector (ed0a102) emits a CSV
where each row identifies one detection (by `detection_id`) plus reviewer-
fillable columns. This tool reads that CSV after a human has filled the
reviewer columns and pushes the resulting set/clear/suppress decisions to
the server.

Classification per row
----------------------
  * unreviewed -- all 7 reviewer fields blank. Skipped silently.
  * suppress   -- `suppress` truthy. There is no server-side suppress path
                  (the endpoint is set/clear), so the row is recorded only
                  in the audit JSON. Suppress means "reviewer judges this
                  detection a noise FP that should not become a correction
                  to retrain on". It does NOT mutate the DB.
  * set        -- both `corrected_owner_type` and `corrected_owner_id`
                  populated. PATCH with {owner_type, owner_id} -> sets
                  `recognition_items.corrected_owner_*` and flips status
                  to `manual_corrected`.
  * clear      -- both fields empty AND reviewer is set (explicit clear).
                  PATCH with {} -> NULLs the corrected_* columns and
                  reverts status to matched/unmatched.
  * invalid    -- half-filled (e.g. owner_type without owner_id).
                  Counted in audit, not sent.

The set path requires a SQL lookup from `(detection_id, project_id)` to
`recognition_items.id` (uuid) because the CSV identifies rows by detection
bigint, not item uuid. Lookup is via psql, idempotent, no mutations.

Default mode is dry-run (no HTTP). Use `--apply` to actually PATCH.

Usage
-----
  python3 tools/bootstrap_correction_importer.py \
    --csv /tmp/corrections-2026-04-29-smoke.csv \
    --report-path /tmp/corrections-import-2026-04-29.json
  # dry-run summary printed to stdout, full audit JSON in --report-path.

  python3 tools/bootstrap_correction_importer.py \
    --csv /tmp/corrections-2026-04-29-smoke.csv \
    --apply \
    --report-path /tmp/corrections-import-2026-04-29.json
  # actually issues PATCHes; failures are collected, not raised.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

DEFAULT_BASE_URL = "http://127.0.0.1:18799"
DEFAULT_PSQL_BIN = "/root/F1-photo/bundled-pg/bin/psql"
DEFAULT_PG_HOST = "127.0.0.1"
DEFAULT_PG_PORT = 55444
DEFAULT_PG_USER = "f1photo"
DEFAULT_PG_PWD = "smokepwd"
DEFAULT_PG_DB = "f1photo_prod"
DEFAULT_ADMIN_USER = "smoke_admin"
DEFAULT_ADMIN_PWD = "smoke-admin-pwd-12345"

VALID_OWNER_TYPES = ("person", "tool", "device")
OWNER_TABLE = {"person": "persons", "tool": "tools", "device": "devices"}
TRUTHY = {"1", "true", "yes", "y", "t"}


# ---------------------------------------------------------------- row model

@dataclass
class ImportRow:
    line_no: int
    detection_id: str
    project_id: str
    project_code: str
    target_type: str
    match_status: str
    score: str
    class_id: str
    corrected_label: str
    corrected_owner_type: str
    corrected_owner_id: str
    suppress: str
    reviewer: str
    reviewed_at: str
    notes: str

    # Resolved at runtime.
    recognition_item_id: Optional[str] = None
    category: str = ""          # unreviewed | suppress | set | clear | invalid
    invalid_reason: str = ""    # populated when category==invalid
    http_status: Optional[int] = None
    error: Optional[str] = None


# ---------------------------------------------------------------- helpers

def _truthy(s: str) -> bool:
    return s.strip().lower() in TRUTHY


def classify(row: ImportRow) -> None:
    rt = (row.corrected_owner_type or "").strip()
    rid = (row.corrected_owner_id or "").strip()
    sup = _truthy(row.suppress)
    rev = (row.reviewer or "").strip()
    lbl = (row.corrected_label or "").strip()
    notes = (row.notes or "").strip()
    any_filled = any([rt, rid, sup, rev, lbl, notes, (row.reviewed_at or "").strip()])

    if not any_filled:
        row.category = "unreviewed"
        return
    if sup:
        row.category = "suppress"
        return
    if rt and rid:
        if rt not in VALID_OWNER_TYPES:
            row.category = "invalid"
            row.invalid_reason = f"corrected_owner_type {rt!r} not in {VALID_OWNER_TYPES}"
            return
        row.category = "set"
        return
    if not rt and not rid and rev:
        row.category = "clear"
        return
    row.category = "invalid"
    if rt and not rid:
        row.invalid_reason = "corrected_owner_type set but corrected_owner_id blank"
    elif rid and not rt:
        row.invalid_reason = "corrected_owner_id set but corrected_owner_type blank"
    else:
        row.invalid_reason = "reviewer fields half-filled in unexpected combination"


def read_csv(path: Path) -> List[ImportRow]:
    rows: List[ImportRow] = []
    with path.open("r", encoding="utf-8", newline="") as fh:
        rdr = csv.DictReader(fh)
        for i, raw in enumerate(rdr, start=2):  # header is line 1
            rows.append(
                ImportRow(
                    line_no=i,
                    detection_id=raw.get("detection_id", "") or "",
                    project_id=raw.get("project_id", "") or "",
                    project_code=raw.get("project_code", "") or "",
                    target_type=raw.get("target_type", "") or "",
                    match_status=raw.get("match_status", "") or "",
                    score=raw.get("score", "") or "",
                    class_id=raw.get("class_id", "") or "",
                    corrected_label=raw.get("corrected_label", "") or "",
                    corrected_owner_type=raw.get("corrected_owner_type", "") or "",
                    corrected_owner_id=raw.get("corrected_owner_id", "") or "",
                    suppress=raw.get("suppress", "") or "",
                    reviewer=raw.get("reviewer", "") or "",
                    reviewed_at=raw.get("reviewed_at", "") or "",
                    notes=raw.get("notes", "") or "",
                )
            )
    return rows


# ---------------------------------------------------------------- psql

def psql_query(
    sql: str,
    *,
    psql_bin: str,
    host: str,
    port: int,
    user: str,
    pwd: str,
    db: str,
) -> List[List[str]]:
    """Run a SQL SELECT via psql with `-A -t -F\\t` and return rows as list of
    columns. Mutations are not allowed (caller restricts SQL)."""
    env = os.environ.copy()
    env["PGPASSWORD"] = pwd
    proc = subprocess.run(
        [psql_bin, "-h", host, "-p", str(port), "-U", user, "-d", db,
         "-A", "-t", "-F", "\t", "-X", "-q", "-c", sql],
        capture_output=True, text=True, env=env, check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"psql failed (rc={proc.returncode}): {proc.stderr.strip()}")
    out = []
    for line in proc.stdout.splitlines():
        if line == "":
            continue
        out.append(line.split("\t"))
    return out


def lookup_item_ids(
    rows: List[ImportRow],
    *,
    psql_bin: str, host: str, port: int, user: str, pwd: str, db: str,
) -> Dict[Tuple[str, str], str]:
    """Batch lookup recognition_items.id for all (project_id, detection_id) pairs
    used by `set` rows. Returns dict keyed by (project_id, detection_id_str)."""
    pairs = sorted({(r.project_id, r.detection_id) for r in rows if r.category == "set"})
    if not pairs:
        return {}
    # Build a single SQL with VALUES list to do all lookups at once.
    values_sql = ", ".join(
        f"('{pid}'::uuid, {int(did)}::bigint)" for (pid, did) in pairs
    )
    sql = (
        "SELECT ri.project_id::text, ri.detection_id::text, ri.id::text "
        "FROM recognition_items ri "
        f"JOIN (VALUES {values_sql}) AS v(pid, did) "
        "ON ri.project_id = v.pid AND ri.detection_id = v.did"
    )
    out: Dict[Tuple[str, str], str] = {}
    for cols in psql_query(sql, psql_bin=psql_bin, host=host, port=port,
                           user=user, pwd=pwd, db=db):
        if len(cols) != 3:
            continue
        out[(cols[0], cols[1])] = cols[2]
    return out


def validate_owners(
    rows: List[ImportRow],
    *,
    psql_bin: str, host: str, port: int, user: str, pwd: str, db: str,
) -> Dict[Tuple[str, str], bool]:
    """Pre-flight: confirm every (owner_type, owner_id) referenced by `set`
    rows exists in the corresponding owner table with deleted_at IS NULL."""
    by_type: Dict[str, List[str]] = {}
    for r in rows:
        if r.category != "set":
            continue
        by_type.setdefault(r.corrected_owner_type.strip(), []).append(
            r.corrected_owner_id.strip()
        )
    found: Dict[Tuple[str, str], bool] = {}
    for ot, ids in by_type.items():
        table = OWNER_TABLE[ot]
        ids_uniq = sorted(set(ids))
        in_clause = ", ".join(f"'{i}'::uuid" for i in ids_uniq)
        sql = (
            f"SELECT id::text FROM {table} "
            f"WHERE id IN ({in_clause}) AND deleted_at IS NULL"
        )
        present = {cols[0] for cols in psql_query(sql, psql_bin=psql_bin,
                   host=host, port=port, user=user, pwd=pwd, db=db) if cols}
        for i in ids_uniq:
            found[(ot, i)] = (i in present)
    return found


# ---------------------------------------------------------------- HTTP

def login(base_url: str, user: str, pwd: str) -> str:
    body = json.dumps({"username": user, "password": pwd}).encode("utf-8")
    req = urllib.request.Request(
        base_url.rstrip("/") + "/api/auth/login",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=15) as resp:
        body = resp.read().decode("utf-8")
    payload = json.loads(body)
    tok = payload.get("access_token") or payload.get("token")
    if not tok:
        raise RuntimeError(f"login response missing access_token: {payload}")
    return tok


def patch_correct(
    base_url: str, token: str, project_id: str, item_id: str,
    payload: Dict[str, Any], *, timeout: float = 15.0,
) -> Tuple[int, str]:
    url = (
        base_url.rstrip("/")
        + f"/api/projects/{project_id}/recognition_items/{item_id}/correct"
    )
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, data=body, method="PATCH",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", errors="replace")


# ---------------------------------------------------------------- main

def build_summary(rows: List[ImportRow]) -> Dict[str, Any]:
    by_cat: Dict[str, int] = {}
    by_owner: Dict[str, int] = {}
    by_target: Dict[str, int] = {}
    by_http: Dict[str, int] = {}
    failed: List[Dict[str, Any]] = []
    for r in rows:
        by_cat[r.category] = by_cat.get(r.category, 0) + 1
        if r.category == "set":
            by_owner[r.corrected_owner_type] = by_owner.get(r.corrected_owner_type, 0) + 1
            by_target[r.target_type] = by_target.get(r.target_type, 0) + 1
        if r.http_status is not None:
            by_http[str(r.http_status)] = by_http.get(str(r.http_status), 0) + 1
        if r.error:
            failed.append({
                "line_no": r.line_no,
                "detection_id": r.detection_id,
                "project_id": r.project_id,
                "category": r.category,
                "http_status": r.http_status,
                "error": r.error,
                "invalid_reason": r.invalid_reason or None,
            })
    return {
        "total_rows": len(rows),
        "by_category": by_cat,
        "set_by_corrected_owner_type": by_owner,
        "set_by_target_type": by_target,
        "http_status_distribution": by_http,
        "failed_rows": failed,
    }


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--csv", required=True, type=Path,
                   help="path to a collector CSV")
    p.add_argument("--report-path", type=Path,
                   default=Path("/tmp/corrections-import-report.json"),
                   help="audit JSON output path (default: %(default)s)")
    p.add_argument("--apply", action="store_true",
                   help="actually PATCH the server. Without this, dry-run only.")
    p.add_argument("--summary-only", action="store_true",
                   help="print summary and skip per-row audit details in the JSON")
    p.add_argument("--limit", type=int, default=None,
                   help="only process the first N reviewable rows after classification")
    p.add_argument("--base-url", default=DEFAULT_BASE_URL)
    p.add_argument("--admin-user", default=DEFAULT_ADMIN_USER)
    p.add_argument("--admin-pwd", default=DEFAULT_ADMIN_PWD)
    p.add_argument("--psql-bin", default=DEFAULT_PSQL_BIN)
    p.add_argument("--pg-host", default=DEFAULT_PG_HOST)
    p.add_argument("--pg-port", type=int, default=DEFAULT_PG_PORT)
    p.add_argument("--pg-user", default=DEFAULT_PG_USER)
    p.add_argument("--pg-pwd", default=DEFAULT_PG_PWD)
    p.add_argument("--pg-db", default=DEFAULT_PG_DB)
    args = p.parse_args(argv)

    started = time.time()
    if not args.csv.is_file():
        print(f"ERROR: csv not found at {args.csv}", file=sys.stderr)
        return 2

    rows = read_csv(args.csv)
    for r in rows:
        classify(r)

    pg_kw = dict(psql_bin=args.psql_bin, host=args.pg_host, port=args.pg_port,
                 user=args.pg_user, pwd=args.pg_pwd, db=args.pg_db)

    # Lookups + owner validation only matter when there are `set` rows.
    set_rows = [r for r in rows if r.category == "set"]
    if args.limit is not None and args.limit >= 0:
        # Apply limit deterministically over the union of mutating categories.
        kept_set = 0
        for r in rows:
            if r.category == "set":
                if kept_set >= args.limit:
                    r.category = "unreviewed"  # demote out of consideration
                    r.invalid_reason = ""
                else:
                    kept_set += 1
        set_rows = [r for r in rows if r.category == "set"]

    if set_rows:
        item_lookup = lookup_item_ids(rows, **pg_kw)
        owner_present = validate_owners(rows, **pg_kw)
        for r in set_rows:
            key = (r.project_id, r.detection_id)
            r.recognition_item_id = item_lookup.get(key)
            if not r.recognition_item_id:
                r.error = ("recognition_items lookup miss for "
                           f"(project_id={r.project_id}, detection_id={r.detection_id})")
            elif not owner_present.get(
                (r.corrected_owner_type.strip(), r.corrected_owner_id.strip()), False
            ):
                r.error = (f"owner not found: {r.corrected_owner_type}"
                           f" id={r.corrected_owner_id} (deleted_at IS NULL)")

    # PATCH phase.
    token: Optional[str] = None
    if args.apply and any(r.category in ("set", "clear") and not r.error for r in rows):
        token = login(args.base_url, args.admin_user, args.admin_pwd)

    for r in rows:
        if r.error:
            continue  # already failed pre-flight
        if r.category == "set":
            payload = {"owner_type": r.corrected_owner_type.strip(),
                       "owner_id": r.corrected_owner_id.strip()}
            if not args.apply:
                r.http_status = None  # dry-run
                continue
            try:
                code, body = patch_correct(args.base_url, token, r.project_id,
                                           r.recognition_item_id, payload)
                r.http_status = code
                if code >= 300:
                    r.error = f"PATCH set failed: HTTP {code}: {body[:200]}"
            except Exception as e:  # noqa: BLE001
                r.error = f"PATCH set raised: {e!r}"
        elif r.category == "clear":
            # Clear path requires the item id too.
            if not r.recognition_item_id:
                # We didn't lookup clears in batch; do an on-the-spot lookup.
                try:
                    out = psql_query(
                        "SELECT id::text FROM recognition_items "
                        f"WHERE project_id = '{r.project_id}'::uuid "
                        f"AND detection_id = {int(r.detection_id)}::bigint",
                        **pg_kw,
                    )
                    if out:
                        r.recognition_item_id = out[0][0]
                    else:
                        r.error = "recognition_items lookup miss for clear"
                        continue
                except Exception as e:  # noqa: BLE001
                    r.error = f"clear-lookup raised: {e!r}"
                    continue
            if not args.apply:
                continue
            try:
                code, body = patch_correct(args.base_url, token, r.project_id,
                                           r.recognition_item_id, {})
                r.http_status = code
                if code >= 300:
                    r.error = f"PATCH clear failed: HTTP {code}: {body[:200]}"
            except Exception as e:  # noqa: BLE001
                r.error = f"PATCH clear raised: {e!r}"

    # Audit JSON.
    summary = build_summary(rows)
    audit: Dict[str, Any] = {
        "meta": {
            "tool": "bootstrap_correction_importer.py",
            "version": 1,
            "csv_path": str(args.csv),
            "started_at": dt.datetime.fromtimestamp(started, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z",
            "finished_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z",
            "mode": "apply" if args.apply else "dry-run",
            "limit": args.limit,
            "base_url": args.base_url,
        },
        "summary": summary,
    }
    if not args.summary_only:
        audit["rows"] = [
            {
                "line_no": r.line_no,
                "detection_id": r.detection_id,
                "project_id": r.project_id,
                "project_code": r.project_code,
                "target_type": r.target_type,
                "category": r.category,
                "corrected_owner_type": r.corrected_owner_type or None,
                "corrected_owner_id": r.corrected_owner_id or None,
                "recognition_item_id": r.recognition_item_id,
                "http_status": r.http_status,
                "error": r.error,
                "invalid_reason": r.invalid_reason or None,
                "reviewer": r.reviewer or None,
                "corrected_label": r.corrected_label or None,
            }
            for r in rows
        ]

    args.report_path.parent.mkdir(parents=True, exist_ok=True)
    args.report_path.write_text(json.dumps(audit, indent=2, sort_keys=True) + "\n")

    # Summary to stdout.
    print("=== CORRECTIONS IMPORT SUMMARY ===")
    print(f"csv:         {args.csv}")
    print(f"mode:        {audit['meta']['mode']}")
    print(f"total rows:  {summary['total_rows']}")
    for k in ("unreviewed", "suppress", "set", "clear", "invalid"):
        if summary["by_category"].get(k):
            print(f"  {k:<10} {summary['by_category'][k]}")
    if summary["set_by_corrected_owner_type"]:
        print("set by owner_type: ", summary["set_by_corrected_owner_type"])
    if summary["http_status_distribution"]:
        print("http status:", summary["http_status_distribution"])
    if summary["failed_rows"]:
        print(f"failures:    {len(summary['failed_rows'])} (see report)")
        for f in summary["failed_rows"][:5]:
            print("  -", f["line_no"], f["category"], f.get("error") or f.get("invalid_reason"))
    print(f"report:      {args.report_path}")
    return 1 if summary["failed_rows"] else 0


if __name__ == "__main__":
    sys.exit(main())
