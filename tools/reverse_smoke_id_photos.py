#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Reverse smoke for milestone #2a-real.

Given a directory of real ID photos already ingested via
``ingest_id_photos.py --mode ingest`` against a SOURCE project (Path A,
seeds the persons + identity_embeddings gallery), this tool re-uploads
the SAME photo bytes into a SECOND project as ``owner_type='wo_raw'``
and expects the production recognition path to match every photo back
to its corresponding ``persons`` row via the cross-project gallery
lookup driven by ``identity_embeddings(source='initial', source_project=<source>)``.

This closes the #2a -> #2b end-to-end recall loop on real-world
Asian-face data. Verification (matched_owner_type + matched_owner_id +
matched_score distribution) is done out-of-band via SQL.

Report schema (per-photo):
    {
      "path": str,
      "expected_employee_no": str,
      "expected_name": str,
      "photo_id": uuid,
      "upload_status": str,        # API response status field
      "upload_error": str | absent,
    }

The SQL verification block lives outside this tool; it joins
``photos`` -> ``detections`` -> ``persons`` on ``matched_owner_id`` and
asserts the EID round-trips.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

# Reuse parser + client + helpers from the Path A tool.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from ingest_id_photos import (  # noqa: E402
    ManifestEntry,
    ServerClient,
    build_manifest,
    manifest_summary,
)


def _do_reverse(args: argparse.Namespace) -> int:
    photos_dir = Path(args.photos_dir).expanduser().resolve()
    entries: List[ManifestEntry] = build_manifest(photos_dir)
    summary = manifest_summary(entries)
    sys.stderr.write(
        "\u25b6 reverse-smoke: "
        f"{summary['parsed_ok']}/{summary['total_files']} parsed, "
        f"{summary['unique_employee_nos']} unique EIDs\n"
    )
    if args.dry_run:
        sys.stderr.write("\u25b6 --dry-run: no API calls will be made\n")
        return 0

    if args.auth_token:
        client = ServerClient(
            base_url=args.base_url.rstrip("/"), token=args.auth_token
        )
    else:
        if not (args.admin_user and args.admin_pwd):
            sys.stderr.write(
                "\u2717 reverse-smoke requires --auth-token OR (--admin-user + --admin-pwd)\n"
            )
            return 2
        client = ServerClient.login(args.base_url, args.admin_user, args.admin_pwd)

    # First-wins per EID (same dedup as Path A so the 1:1 expected mapping holds).
    seen: Dict[str, ManifestEntry] = {}
    for e in entries:
        if not e.parsed:
            continue
        seen.setdefault(e.parsed.employee_no, e)

    report: Dict[str, Any] = {
        "summary": summary,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "target_project_id": args.project_id,
        "results": [],
        "counters": {
            "uploads_ok": 0,
            "uploads_failed": 0,
            "parse_failed": summary["parse_failed"],
        },
    }

    for eid, entry in seen.items():
        assert entry.parsed is not None
        photo_path = Path(entry.path)
        item: Dict[str, Any] = {
            "path": entry.path,
            "expected_employee_no": eid,
            "expected_name": entry.parsed.name,
            "dept_tag": entry.parsed.dept_tag,
        }
        try:
            up = client.upload_photo_as_wo_raw(
                project_id=args.project_id,
                photo_path=photo_path,
            )
            item["photo_id"] = up.get("id")
            item["upload_status"] = up.get("status")
            report["counters"]["uploads_ok"] += 1
        except Exception as e:  # noqa: BLE001
            item["upload_error"] = repr(e)
            report["counters"]["uploads_failed"] += 1
            if not args.continue_on_error:
                report["results"].append(item)
                _finalise_report(report, args)
                return 1
        report["results"].append(item)

    _finalise_report(report, args)
    sys.stderr.write(
        "\u25b6 reverse-smoke done: "
        f"uploads ok={report['counters']['uploads_ok']} "
        f"failed={report['counters']['uploads_failed']}\n"
    )
    return 0 if report["counters"]["uploads_failed"] == 0 else 3


def _finalise_report(report: Dict[str, Any], args: argparse.Namespace) -> None:
    report["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    text = json.dumps(report, ensure_ascii=False, indent=2)
    out = args.report_out
    if not out or out == "-":
        sys.stdout.write(text + "\n")
    else:
        Path(out).write_text(text + "\n", encoding="utf-8")


def _build_argparser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Reverse smoke for #2a-real: re-upload ingested ID photos"
        " as wo_raw into a second project and expect cross-project gallery match.",
    )
    p.add_argument(
        "--photos-dir",
        required=True,
        help="Directory of real ID photos (same set used by Path A ingest).",
    )
    p.add_argument("--report-out", default=None, help="Reverse-smoke report JSON; '-' or omit = stdout.")
    p.add_argument("--base-url", default="http://127.0.0.1:18799", help="Server base URL.")
    p.add_argument("--auth-token", default=None, help="Bearer token (skip --admin-user/--admin-pwd).")
    p.add_argument("--admin-user", default=None, help="Admin username (login flow).")
    p.add_argument("--admin-pwd", default=None, help="Admin password (login flow).")
    p.add_argument(
        "--project-id",
        required=True,
        help="TARGET project UUID for the wo_raw uploads (NOT the source persons project).",
    )
    p.add_argument("--continue-on-error", action="store_true", help="Skip failures and keep going.")
    p.add_argument("--dry-run", action="store_true", help="Parse + plan, no API calls.")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = _build_argparser().parse_args(argv)
    return _do_reverse(args)


if __name__ == "__main__":
    raise SystemExit(main())
