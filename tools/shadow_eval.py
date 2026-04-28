#!/usr/bin/env python3
"""
Shadow-evaluation orchestrator for milestone #7c-eval-auto.

Drives the existing #2-tool distribution and #2c face P/R baselines twice —
once against the currently-installed object_detect.onnx, once against a
candidate ONNX — then assembles an `eval_deltas.json` blob that matches the
schema consumed by `f1photo retrain-detector promote --eval-deltas` (Rust
type: server::retrain::EvalDeltas, locked at milestone #7c-eval-skel).

Output schema (EvalDeltas)::

    {
      "tool": {
        "current_recognition_items_mean": <f64>,
        "candidate_recognition_items_mean": <f64>,
        "delta": <candidate - current>,
        "fixture_photos": <i64>
      },
      "face": {
        "current_western_f1": <f64>,
        "candidate_western_f1": <f64>,
        "delta": <candidate - current>,
        "fixture_photos": <i64>
      },
      "current_onnx_sha256": "...",
      "candidate_onnx_sha256": "...",
      "generated_at": "<ISO-8601 UTC, Z suffix>"
    }

Modes

1. **live (default)** — 4 server boots:
     a) live ONNX → distribution-baseline.sh → tool_current report
     b) live ONNX → recognition-pr-baseline.sh → face_current report
     c) swap candidate in (atomic os.replace) → distribution-baseline.sh
     d) candidate ONNX → recognition-pr-baseline.sh
   Live ONNX is restored from backup in a `finally` block (always).

2. **--from-reports** — given 4 pre-existing report JSONs and the two
   ONNX paths, just compute sha256s and assemble. Used by
   tools/shadow_eval_smoke.sh and for re-runs without re-evaluating.

Atomic ONNX restore: backup BEFORE swap, restore via os.replace
(rename within the same filesystem). A crash mid-candidate still
leaves the production ONNX intact at its original sha256.

Usage::

    tools/shadow_eval.py \
        --candidate /path/to/candidate.onnx \
        --models-dir /root/F1-photo/models \
        --out /tmp/eval_deltas.json

    tools/shadow_eval.py --from-reports \
        --tool-current-report /tmp/tool_cur.json \
        --tool-candidate-report /tmp/tool_cand.json \
        --face-current-report /tmp/face_cur.json \
        --face-candidate-report /tmp/face_cand.json \
        --current-onnx /root/F1-photo/models/object_detect.onnx \
        --candidate-onnx /path/to/candidate.onnx \
        --out /tmp/eval_deltas.json

Then feed the result back to:

    f1photo retrain-detector promote --candidate <path> \
        --eval-deltas /tmp/eval_deltas.json

Part of milestone #7c-eval-auto.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def utc_now_iso() -> str:
    # Z suffix, no microseconds — matches the format record_promotion writes.
    return (
        dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def parse_tool_report(report_path: Path) -> tuple[float, int]:
    """Return (recognition_items_total mean, fixture_photo_count).

    The eval_distribution.py report has top-level `per_photo: list[dict]`
    where each entry has integer `recognition_items_total`.
    """
    data = json.loads(report_path.read_text())
    per_photo = data.get("per_photo")
    if not isinstance(per_photo, list) or not per_photo:
        raise SystemExit(f"\u2717 {report_path}: missing or empty 'per_photo'")
    totals: list[int] = []
    for p in per_photo:
        v = p.get("recognition_items_total")
        if v is None:
            raise SystemExit(
                f"\u2717 {report_path}: per_photo entry missing recognition_items_total"
            )
        totals.append(int(v))
    n = len(totals)
    return float(sum(totals) / n), int(n)


def parse_face_report(report_path: Path) -> tuple[float, int]:
    """Return (western F1, western_n photo count).

    The eval_pr.py report has `per_bucket_at_default.western.f1` (float|null)
    and `.n` (int). Null F1 means insufficient TP/FP — the gate cannot be
    evaluated and we abort.
    """
    data = json.loads(report_path.read_text())
    western = (data.get("per_bucket_at_default") or {}).get("western")
    if not isinstance(western, dict):
        raise SystemExit(
            f"\u2717 {report_path}: missing per_bucket_at_default.western"
        )
    f1 = western.get("f1")
    if f1 is None:
        raise SystemExit(
            f"\u2717 {report_path}: western F1 is null (insufficient TP/FP) — gate cannot be evaluated"
        )
    n = western.get("n")
    if not isinstance(n, int):
        raise SystemExit(f"\u2717 {report_path}: missing per_bucket_at_default.western.n")
    return float(f1), int(n)


def run_baseline(script: Path, report_path: Path, repo_root: Path) -> None:
    """Run one of the baseline shell scripts with REPORT_PATH set."""
    env = os.environ.copy()
    env["REPORT_PATH"] = str(report_path)
    print(f"\u25b6 running {script.name} (REPORT_PATH={report_path})", flush=True)
    rc = subprocess.call(["bash", str(script)], env=env, cwd=str(repo_root))
    if rc != 0:
        raise SystemExit(f"\u2717 {script.name} exit={rc}")


def assemble_eval_deltas(
    *,
    tool_current_mean: float,
    tool_candidate_mean: float,
    tool_n: int,
    face_current_f1: float,
    face_candidate_f1: float,
    face_n: int,
    current_onnx_sha: str,
    candidate_onnx_sha: str,
) -> dict[str, Any]:
    return {
        "tool": {
            "current_recognition_items_mean": tool_current_mean,
            "candidate_recognition_items_mean": tool_candidate_mean,
            "delta": tool_candidate_mean - tool_current_mean,
            "fixture_photos": tool_n,
        },
        "face": {
            "current_western_f1": face_current_f1,
            "candidate_western_f1": face_candidate_f1,
            "delta": face_candidate_f1 - face_current_f1,
            "fixture_photos": face_n,
        },
        "current_onnx_sha256": current_onnx_sha,
        "candidate_onnx_sha256": candidate_onnx_sha,
        "generated_at": utc_now_iso(),
    }


def from_reports_mode(args: argparse.Namespace) -> int:
    paths = {
        "tool-current-report": Path(args.tool_current_report),
        "tool-candidate-report": Path(args.tool_candidate_report),
        "face-current-report": Path(args.face_current_report),
        "face-candidate-report": Path(args.face_candidate_report),
        "current-onnx": Path(args.current_onnx),
        "candidate-onnx": Path(args.candidate_onnx),
    }
    for label, p in paths.items():
        if not p.exists():
            raise SystemExit(f"\u2717 missing --{label}: {p}")

    tcur, tn_cur = parse_tool_report(paths["tool-current-report"])
    tcand, tn_cand = parse_tool_report(paths["tool-candidate-report"])
    if tn_cur != tn_cand:
        print(
            f"\u26a0 tool fixture size differs: current={tn_cur} candidate={tn_cand} (using current)",
            file=sys.stderr,
        )
    fcur, fn_cur = parse_face_report(paths["face-current-report"])
    fcand, fn_cand = parse_face_report(paths["face-candidate-report"])
    if fn_cur != fn_cand:
        print(
            f"\u26a0 face fixture size differs: current={fn_cur} candidate={fn_cand} (using current)",
            file=sys.stderr,
        )

    deltas = assemble_eval_deltas(
        tool_current_mean=tcur,
        tool_candidate_mean=tcand,
        tool_n=tn_cur,
        face_current_f1=fcur,
        face_candidate_f1=fcand,
        face_n=fn_cur,
        current_onnx_sha=sha256_file(paths["current-onnx"]),
        candidate_onnx_sha=sha256_file(paths["candidate-onnx"]),
    )
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(deltas, indent=2, sort_keys=True) + "\n")
    print(f"\u2713 wrote {out}")
    print(json.dumps(deltas, indent=2, sort_keys=True))
    return 0


def live_mode(args: argparse.Namespace) -> int:
    repo = Path(args.repo_root).resolve()
    models_dir = Path(args.models_dir).resolve()
    candidate = Path(args.candidate).resolve()
    live_onnx = models_dir / "object_detect.onnx"
    work = Path(tempfile.mkdtemp(prefix="shadow-eval-"))
    print(f"\u2192 work dir:    {work}")
    print(f"\u2192 live ONNX:   {live_onnx}")
    print(f"\u2192 candidate:   {candidate}")

    if not live_onnx.exists():
        raise SystemExit(f"\u2717 live ONNX missing: {live_onnx}")
    if not candidate.exists():
        raise SystemExit(f"\u2717 candidate ONNX missing: {candidate}")

    cur_sha = sha256_file(live_onnx)
    cand_sha = sha256_file(candidate)
    print(f"\u2192 current sha: {cur_sha}")
    print(f"\u2192 candidate:   {cand_sha}")
    if cur_sha == cand_sha:
        print(
            "\u26a0 candidate sha matches live sha — gate would still run, but deltas will be ~0",
            file=sys.stderr,
        )

    dist_script = repo / "packaging/scripts/distribution-baseline.sh"
    pr_script = repo / "packaging/scripts/recognition-pr-baseline.sh"
    for s in (dist_script, pr_script):
        if not s.exists():
            raise SystemExit(f"\u2717 baseline script missing: {s}")

    tool_cur_rep = work / "tool_current.json"
    tool_cand_rep = work / "tool_candidate.json"
    face_cur_rep = work / "face_current.json"
    face_cand_rep = work / "face_candidate.json"

    backup = work / "object_detect.live.onnx"
    print(f"\u25b6 backing up live ONNX \u2192 {backup}")
    shutil.copy2(live_onnx, backup)
    backup_sha = sha256_file(backup)
    if backup_sha != cur_sha:
        raise SystemExit(f"\u2717 backup sha mismatch: {backup_sha} != {cur_sha}")

    try:
        # Phase 1: current ONNX in place.
        print("\u2550\u2550 phase 1/2: evaluating current ONNX \u2550\u2550")
        run_baseline(dist_script, tool_cur_rep, repo)
        run_baseline(pr_script, face_cur_rep, repo)

        # Phase 2: atomic candidate swap.
        print("\u2550\u2550 phase 2/2: evaluating candidate ONNX \u2550\u2550")
        tmp_swap = live_onnx.with_suffix(".onnx.swap")
        shutil.copy2(candidate, tmp_swap)
        os.replace(tmp_swap, live_onnx)
        swapped_sha = sha256_file(live_onnx)
        if swapped_sha != cand_sha:
            raise SystemExit(
                f"\u2717 swap sha mismatch: {swapped_sha} != {cand_sha}"
            )
        run_baseline(dist_script, tool_cand_rep, repo)
        run_baseline(pr_script, face_cand_rep, repo)
    finally:
        # Always restore live ONNX from backup. Atomic rename within tmp.
        print(f"\u25b6 restoring live ONNX from {backup}")
        try:
            tmp_restore = live_onnx.with_suffix(".onnx.restore")
            shutil.copy2(backup, tmp_restore)
            os.replace(tmp_restore, live_onnx)
            restored_sha = sha256_file(live_onnx)
            if restored_sha != cur_sha:
                print(
                    f"\u26a0 restored sha mismatch: {restored_sha} != {cur_sha}",
                    file=sys.stderr,
                )
            else:
                print("\u2713 live ONNX restored")
        except Exception as e:  # noqa: BLE001
            print(f"\u2717 FAILED to restore live ONNX: {e}", file=sys.stderr)
            print(f"  manual restore: cp {backup} {live_onnx}", file=sys.stderr)
            raise

    tcur, tn = parse_tool_report(tool_cur_rep)
    tcand, _ = parse_tool_report(tool_cand_rep)
    fcur, fn = parse_face_report(face_cur_rep)
    fcand, _ = parse_face_report(face_cand_rep)
    deltas = assemble_eval_deltas(
        tool_current_mean=tcur,
        tool_candidate_mean=tcand,
        tool_n=tn,
        face_current_f1=fcur,
        face_candidate_f1=fcand,
        face_n=fn,
        current_onnx_sha=cur_sha,
        candidate_onnx_sha=cand_sha,
    )
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(deltas, indent=2, sort_keys=True) + "\n")
    print(f"\u2713 wrote {out}")
    print(json.dumps(deltas, indent=2, sort_keys=True))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Shadow-evaluation orchestrator (#7c-eval-auto).",
    )
    ap.add_argument("--out", default="/tmp/eval_deltas.json",
                    help="output path for the assembled eval_deltas.json")
    ap.add_argument("--repo-root", default=str(Path(__file__).resolve().parent.parent),
                    help="repository root (default: directory containing tools/)")

    g_live = ap.add_argument_group("live mode (default)")
    g_live.add_argument("--candidate",
                        help="path to candidate object_detect ONNX")
    g_live.add_argument("--models-dir", default="/root/F1-photo/models",
                        help="models dir containing object_detect.onnx")

    g_rep = ap.add_argument_group("--from-reports mode")
    g_rep.add_argument("--from-reports", action="store_true",
                       help="skip live evaluation; assemble from existing reports")
    g_rep.add_argument("--tool-current-report")
    g_rep.add_argument("--tool-candidate-report")
    g_rep.add_argument("--face-current-report")
    g_rep.add_argument("--face-candidate-report")
    g_rep.add_argument("--current-onnx")
    g_rep.add_argument("--candidate-onnx")

    args = ap.parse_args()

    if args.from_reports:
        required = [
            "tool_current_report", "tool_candidate_report",
            "face_current_report", "face_candidate_report",
            "current_onnx", "candidate_onnx",
        ]
        missing = [r.replace("_", "-") for r in required if not getattr(args, r)]
        if missing:
            ap.error("--from-reports requires: " + ", ".join("--" + m for m in missing))
        return from_reports_mode(args)
    if not args.candidate:
        ap.error("--candidate is required in live mode (or use --from-reports)")
    return live_mode(args)


if __name__ == "__main__":
    sys.exit(main())
