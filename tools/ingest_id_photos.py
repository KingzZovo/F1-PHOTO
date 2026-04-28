#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""
ingest_id_photos.py — F1-photo personnel ID-photo ingestion harness.

Two stages, both exposed via --mode:

  parse-only (default)   Walk a directory of personnel ID-photo files, parse
                         each filename into (employee_no, name, dept_tag),
                         emit a JSON manifest. Pure local; no API calls.

  ingest                 Take an already-validated manifest (or re-parse on
                         the fly), then for each unique employee_no:
                           1. POST /api/persons   (handles 409 reuse)
                           2. POST /api/projects/<pid>/photos
                                  owner_type=person, owner_id=<persons.id>,
                                  employee_no=<eid>
                              -- triggers worker #2a person-bootstrap path:
                                 SCRFD detect -> ArcFace embed ->
                                 identity_embeddings(source='initial')
                         Emits a final JSON report with per-photo status
                         + aggregate counters.

Filename grammar (observed from real samples 2026-04-28):

    <name?><sep?>(\d{8})<sep?><dept_tag?><.ext>
    <sep>     := one of ''  '+'  '_'  '-'  ' '
    <name>    := non-digit chars (typically Han characters)
    <dept>    := non-digit chars (optional; e.g. "网控电气")
    <eid>     := 8 contiguous digits; first 4 = onboarding year

Reverse order ("<eid><name>.ext") is also accepted: when the segment to
the LEFT of the 8-digit run is empty / whitespace, the segment to the
RIGHT is treated as the name.

Author: F1-photo dev relay agent (milestone #2a real-data ingestion).
"""
from __future__ import annotations

import argparse
import dataclasses
import json
import mimetypes
import os
import re
import sys
import time
import urllib.parse
import urllib.request
import uuid as _uuid_mod
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# ---------------------------------------------------------------------------
# Filename parsing
# ---------------------------------------------------------------------------

EID_RE = re.compile(r"(\d{8})")
SEP_RE = re.compile(r"^[\s+_\-]+|[\s+_\-]+$")
ALLOWED_EXTS = {".jpg", ".jpeg", ".png", ".webp", ".bmp"}


class ParseError(ValueError):
    """Filename did not match the expected grammar."""


@dataclasses.dataclass
class ParsedName:
    employee_no: str
    name: str
    dept_tag: Optional[str]
    raw_stem: str

    def to_dict(self) -> Dict[str, Any]:
        d = dataclasses.asdict(self)
        return d


def _strip_seps(s: str) -> str:
    return SEP_RE.sub("", s).strip()


def parse_filename(stem: str) -> ParsedName:
    """Parse a personnel ID-photo filename stem (no extension).

    Raises ParseError if the stem does not contain a single 8-digit run
    that we can confidently treat as the employee_no.
    """
    if not stem:
        raise ParseError("empty stem")
    matches = list(EID_RE.finditer(stem))
    if not matches:
        raise ParseError(f"no 8-digit employee_no found in {stem!r}")
    # If multiple 8-digit runs: prefer the first (filenames in the wild
    # don't have collisions; the second run would be an artefact).
    m = matches[0]
    eid = m.group(1)
    left = _strip_seps(stem[: m.start()])
    right = _strip_seps(stem[m.end() :])
    if left and right:
        # "<name><eid><dept>" canonical case
        name, dept = left, right
    elif left and not right:
        # "<name><eid>" canonical case
        name, dept = left, None
    elif right and not left:
        # "<eid><name>" reverse-order case
        name, dept = right, None
    else:
        raise ParseError(
            f"could not extract personnel name from {stem!r} (eid={eid})"
        )
    if not name:
        raise ParseError(f"empty name after stripping seps in {stem!r}")
    return ParsedName(
        employee_no=eid,
        name=name,
        dept_tag=dept,
        raw_stem=stem,
    )


# ---------------------------------------------------------------------------
# Directory walk -> manifest
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class ManifestEntry:
    path: str
    parsed: Optional[ParsedName]
    error: Optional[str]

    def to_dict(self) -> Dict[str, Any]:
        return {
            "path": self.path,
            "parsed": self.parsed.to_dict() if self.parsed else None,
            "error": self.error,
        }


def build_manifest(photos_dir: Path) -> List[ManifestEntry]:
    if not photos_dir.is_dir():
        raise FileNotFoundError(f"--photos-dir does not exist: {photos_dir}")
    entries: List[ManifestEntry] = []
    for p in sorted(photos_dir.iterdir(), key=lambda x: x.name):
        if not p.is_file():
            continue
        if p.suffix.lower() not in ALLOWED_EXTS:
            continue
        try:
            parsed = parse_filename(p.stem)
            entries.append(ManifestEntry(str(p), parsed, None))
        except ParseError as e:
            entries.append(ManifestEntry(str(p), None, str(e)))
    return entries


def manifest_summary(entries: List[ManifestEntry]) -> Dict[str, Any]:
    total = len(entries)
    parsed_ok = sum(1 for e in entries if e.parsed is not None)
    parse_failed = total - parsed_ok
    seen_eids: Dict[str, int] = {}
    for e in entries:
        if e.parsed:
            seen_eids[e.parsed.employee_no] = seen_eids.get(
                e.parsed.employee_no, 0
            ) + 1
    duplicate_eids = {k: v for k, v in seen_eids.items() if v > 1}
    return {
        "total_files": total,
        "parsed_ok": parsed_ok,
        "parse_failed": parse_failed,
        "unique_employee_nos": len(seen_eids),
        "duplicate_eids": duplicate_eids,
    }


# ---------------------------------------------------------------------------
# HTTP helpers (stdlib only -- no requests dependency)
# ---------------------------------------------------------------------------


def _http_request(
    url: str,
    method: str = "GET",
    headers: Optional[Dict[str, str]] = None,
    body: Optional[bytes] = None,
    timeout: float = 30.0,
) -> Tuple[int, Dict[str, str], bytes]:
    req = urllib.request.Request(url, data=body, method=method)
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, dict(resp.headers), resp.read()
    except urllib.error.HTTPError as e:  # type: ignore[attr-defined]
        return e.code, dict(e.headers or {}), e.read() or b""


def _build_multipart(
    fields: Dict[str, str],
    file_field: str,
    file_path: Path,
    file_mime: Optional[str] = None,
) -> Tuple[bytes, str]:
    boundary = "----f1photo-ingest-" + _uuid_mod.uuid4().hex
    out = bytearray()
    for k, v in fields.items():
        out.extend(f"--{boundary}\r\n".encode("utf-8"))
        out.extend(
            f'Content-Disposition: form-data; name="{k}"\r\n\r\n'.encode("utf-8")
        )
        out.extend(v.encode("utf-8"))
        out.extend(b"\r\n")
    if file_mime is None:
        file_mime, _ = mimetypes.guess_type(str(file_path))
        if file_mime is None:
            file_mime = "application/octet-stream"
    fname = file_path.name
    out.extend(f"--{boundary}\r\n".encode("utf-8"))
    out.extend(
        (
            f'Content-Disposition: form-data; name="{file_field}"; '
            f'filename="{fname}"\r\n'
        ).encode("utf-8")
    )
    out.extend(f"Content-Type: {file_mime}\r\n\r\n".encode("utf-8"))
    out.extend(file_path.read_bytes())
    out.extend(b"\r\n")
    out.extend(f"--{boundary}--\r\n".encode("utf-8"))
    return bytes(out), boundary


# ---------------------------------------------------------------------------
# Server API client (admin scope; uses bearer token)
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class ServerClient:
    base_url: str
    token: str

    def _h(self, extra: Optional[Dict[str, str]] = None) -> Dict[str, str]:
        h = {"authorization": f"Bearer {self.token}"}
        if extra:
            h.update(extra)
        return h

    @classmethod
    def login(
        cls, base_url: str, admin_user: str, admin_pwd: str
    ) -> "ServerClient":
        body = json.dumps({"username": admin_user, "password": admin_pwd}).encode(
            "utf-8"
        )
        status, _, raw = _http_request(
            f"{base_url.rstrip('/')}/api/auth/login",
            method="POST",
            headers={"content-type": "application/json"},
            body=body,
        )
        if status not in (200, 201):
            raise RuntimeError(
                f"login failed: status={status} body={raw[:300]!r}"
            )
        # Server response field has flipped between releases:
        #   - older builds returned {"token": ...}
        #   - current build returns {"access_token": ..., "token_type": "Bearer", ...}
        # Accept either so the harness keeps working across both.
        body_json = json.loads(raw)
        token = body_json.get("token") or body_json.get("access_token")
        if not token:
            raise RuntimeError(f"login response missing token: {raw[:300]!r}")
        return cls(base_url=base_url.rstrip("/"), token=token)

    def find_person_by_eid(self, employee_no: str) -> Optional[str]:
        """List persons and find by employee_no. Returns person UUID or None."""
        q = urllib.parse.urlencode({"employee_no": employee_no, "limit": 50})
        status, _, raw = _http_request(
            f"{self.base_url}/api/persons?{q}",
            method="GET",
            headers=self._h(),
        )
        if status != 200:
            return None
        data = json.loads(raw)
        # Response may be {items: [...]} or a bare list -- be liberal.
        rows = data.get("items") if isinstance(data, dict) else data
        if not rows:
            return None
        for r in rows:
            if r.get("employee_no") == employee_no:
                return r.get("id")
        return None

    def create_person(
        self,
        employee_no: str,
        name: str,
        department: Optional[str] = None,
    ) -> Tuple[str, bool]:
        """Returns (person_id, created). created=False means it already existed."""
        payload: Dict[str, Any] = {"employee_no": employee_no, "name": name}
        if department:
            payload["department"] = department
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        status, _, raw = _http_request(
            f"{self.base_url}/api/persons",
            method="POST",
            headers=self._h({"content-type": "application/json; charset=utf-8"}),
            body=body,
        )
        if status == 201:
            return json.loads(raw)["id"], True
        if status == 409:
            existing = self.find_person_by_eid(employee_no)
            if existing:
                return existing, False
        raise RuntimeError(
            f"create_person failed: status={status} body={raw[:300]!r}"
        )

    def upload_photo_for_person(
        self,
        project_id: str,
        person_id: str,
        employee_no: str,
        photo_path: Path,
    ) -> Dict[str, Any]:
        body, boundary = _build_multipart(
            fields={
                "owner_type": "person",
                "owner_id": person_id,
                "employee_no": employee_no,
            },
            file_field="file",
            file_path=photo_path,
        )
        status, _, raw = _http_request(
            f"{self.base_url}/api/projects/{project_id}/photos",
            method="POST",
            headers=self._h(
                {"content-type": f"multipart/form-data; boundary={boundary}"}
            ),
            body=body,
            timeout=120.0,
        )
        if status not in (200, 201, 202):
            raise RuntimeError(
                f"upload failed for {photo_path.name}: status={status} body={raw[:300]!r}"
            )
        return json.loads(raw)


# ---------------------------------------------------------------------------
# CLI entry points
# ---------------------------------------------------------------------------


def _emit_manifest(
    entries: List[ManifestEntry], out_path: Optional[Path]
) -> None:
    payload = {
        "summary": manifest_summary(entries),
        "entries": [e.to_dict() for e in entries],
    }
    text = json.dumps(payload, ensure_ascii=False, indent=2)
    if out_path is None or str(out_path) == "-":
        sys.stdout.write(text + "\n")
    else:
        out_path.write_text(text + "\n", encoding="utf-8")


def _do_parse_only(args: argparse.Namespace) -> int:
    photos_dir = Path(args.photos_dir).expanduser().resolve()
    entries = build_manifest(photos_dir)
    out = Path(args.manifest_out) if args.manifest_out else None
    _emit_manifest(entries, out)
    summary = manifest_summary(entries)
    sys.stderr.write(
        "\u25b6 parse-only: "
        f"{summary['parsed_ok']}/{summary['total_files']} parsed, "
        f"{summary['parse_failed']} failed, "
        f"{summary['unique_employee_nos']} unique EIDs, "
        f"{len(summary['duplicate_eids'])} duplicate EID groups\n"
    )
    return 0 if summary["parse_failed"] == 0 else 2


def _do_ingest(args: argparse.Namespace) -> int:
    photos_dir = Path(args.photos_dir).expanduser().resolve()
    entries = build_manifest(photos_dir)
    summary = manifest_summary(entries)
    sys.stderr.write(
        "\u25b6 ingest: "
        f"{summary['parsed_ok']}/{summary['total_files']} parsed, "
        f"{summary['unique_employee_nos']} unique EIDs\n"
    )
    if args.dry_run:
        sys.stderr.write("\u25b6 --dry-run: no API calls will be made\n")
        _emit_manifest(entries, Path(args.manifest_out) if args.manifest_out else None)
        return 0

    # Login
    if args.auth_token:
        client = ServerClient(
            base_url=args.base_url.rstrip("/"), token=args.auth_token
        )
    else:
        if not (args.admin_user and args.admin_pwd):
            sys.stderr.write(
                "\u2717 ingest mode requires --auth-token OR (--admin-user + --admin-pwd)\n"
            )
            return 2
        client = ServerClient.login(args.base_url, args.admin_user, args.admin_pwd)

    # Dedupe entries by employee_no (first-wins)
    seen: Dict[str, ManifestEntry] = {}
    for e in entries:
        if not e.parsed:
            continue
        if e.parsed.employee_no not in seen:
            seen[e.parsed.employee_no] = e

    report = {
        "summary": summary,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "results": [],
        "counters": {
            "persons_created": 0,
            "persons_reused": 0,
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
            "employee_no": eid,
            "name": entry.parsed.name,
            "dept_tag": entry.parsed.dept_tag,
        }
        try:
            person_id, created = client.create_person(
                employee_no=eid,
                name=entry.parsed.name,
                department=entry.parsed.dept_tag,
            )
            item["person_id"] = person_id
            item["person_created"] = created
            if created:
                report["counters"]["persons_created"] += 1
            else:
                report["counters"]["persons_reused"] += 1
        except Exception as e:  # noqa: BLE001
            item["person_error"] = repr(e)
            report["counters"]["uploads_failed"] += 1
            report["results"].append(item)
            if not args.continue_on_error:
                _finalise_report(report, args)
                return 1
            continue

        try:
            up = client.upload_photo_for_person(
                project_id=args.project_id,
                person_id=person_id,
                employee_no=eid,
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
    return 0 if report["counters"]["uploads_failed"] == 0 else 3


def _finalise_report(report: Dict[str, Any], args: argparse.Namespace) -> None:
    report["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    text = json.dumps(report, ensure_ascii=False, indent=2)
    out = args.report_out
    if not out or out == "-":
        sys.stdout.write(text + "\n")
    else:
        Path(out).write_text(text + "\n", encoding="utf-8")
    c = report["counters"]
    sys.stderr.write(
        "\u25b6 ingest done: "
        f"persons created={c['persons_created']} reused={c['persons_reused']} "
        f"uploads ok={c['uploads_ok']} failed={c['uploads_failed']}\n"
    )


def _build_argparser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="ingest_id_photos",
        description=(
            "Parse personnel ID-photo filenames and (optionally) ingest them "
            "into F1-photo via the person-bootstrap path (#2a)."
        ),
    )
    p.add_argument(
        "--mode",
        choices=["parse-only", "ingest"],
        default="parse-only",
        help="parse-only writes a manifest; ingest also calls the server API.",
    )
    p.add_argument(
        "--photos-dir",
        required=True,
        help="Directory containing personnel ID-photo files.",
    )
    p.add_argument("--manifest-out", default=None, help="Manifest JSON path; '-' or omit = stdout.")
    p.add_argument("--report-out", default=None, help="Ingest report JSON path; '-' or omit = stdout.")
    p.add_argument("--base-url", default="http://127.0.0.1:18799", help="Server base URL (ingest mode).")
    p.add_argument("--auth-token", default=None, help="Bearer token (skip --admin-user/--admin-pwd).")
    p.add_argument("--admin-user", default=None, help="Admin username (login flow).")
    p.add_argument("--admin-pwd", default=None, help="Admin password (login flow).")
    p.add_argument("--project-id", default=None, help="Project UUID to upload into (ingest mode).")
    p.add_argument("--continue-on-error", action="store_true", help="Skip failures and keep going.")
    p.add_argument("--dry-run", action="store_true", help="Ingest mode: parse + plan, no API calls.")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = _build_argparser().parse_args(argv)
    if args.mode == "parse-only":
        return _do_parse_only(args)
    if args.mode == "ingest":
        if not args.project_id and not args.dry_run:
            sys.stderr.write("\u2717 ingest mode requires --project-id\n")
            return 2
        return _do_ingest(args)
    sys.stderr.write(f"\u2717 unknown --mode: {args.mode!r}\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

