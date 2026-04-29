"""Unit tests for tools/shadow_eval.py pure functions.

Network-free, no-server, no-ONNX-IO. Covers the deterministic surface that
the #7c-eval-auto gate depends on:
  - sha256_file()       : hashing of arbitrary bytes
  - utc_now_iso()       : ISO-8601 UTC format with Z suffix, no microseconds
  - parse_tool_report() : eval_distribution.py schema parsing + 3 failure paths
  - parse_face_report() : eval_pr.py schema parsing + 3 failure paths
  - assemble_eval_deltas(): EvalDeltas blob shape + delta arithmetic

The live mode (server boots, ONNX swap) and run_baseline() subprocess driver
are intentionally not tested here -- they are integration-only and already
exercised by docs/baselines/7c-eval-auto-smoke.json /
7c-eval-self-wiring-2026-04-29.json.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import re
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
SHADOW_EVAL_PATH = REPO_ROOT / "tools" / "shadow_eval.py"

_spec = importlib.util.spec_from_file_location("shadow_eval", str(SHADOW_EVAL_PATH))
assert _spec is not None and _spec.loader is not None
shadow_eval = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(shadow_eval)


# ---------------------------------------------------------------------------
# sha256_file
# ---------------------------------------------------------------------------


def test_sha256_file_empty(tmp_path: Path) -> None:
    p = tmp_path / "empty.bin"
    p.write_bytes(b"")
    assert (
        shadow_eval.sha256_file(p)
        == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    )


def test_sha256_file_known_string(tmp_path: Path) -> None:
    p = tmp_path / "hello.bin"
    p.write_bytes(b"hello world")
    expected = hashlib.sha256(b"hello world").hexdigest()
    assert shadow_eval.sha256_file(p) == expected


def test_sha256_file_streaming_large(tmp_path: Path) -> None:
    # >1 MiB to exercise the multi-chunk read path.
    p = tmp_path / "big.bin"
    payload = (b"x" * 1024) * 1100  # ~1.1 MiB
    p.write_bytes(payload)
    assert shadow_eval.sha256_file(p) == hashlib.sha256(payload).hexdigest()


# ---------------------------------------------------------------------------
# utc_now_iso
# ---------------------------------------------------------------------------


def test_utc_now_iso_format() -> None:
    s = shadow_eval.utc_now_iso()
    # Must look like 2026-04-29T13:29:00Z -- Z suffix, no microseconds, no offset.
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", s), s
    assert s.endswith("Z")
    assert "." not in s  # no fractional seconds
    assert "+" not in s  # no offset suffix


# ---------------------------------------------------------------------------
# parse_tool_report
# ---------------------------------------------------------------------------


def _write_json(p: Path, obj: dict) -> Path:
    p.write_text(json.dumps(obj))
    return p


def test_parse_tool_report_happy(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "tool.json",
        {
            "per_photo": [
                {"recognition_items_total": 3},
                {"recognition_items_total": 5},
                {"recognition_items_total": 4},
            ]
        },
    )
    mean, n = shadow_eval.parse_tool_report(rp)
    assert mean == pytest.approx(4.0)
    assert n == 3


def test_parse_tool_report_single_photo(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "tool.json",
        {"per_photo": [{"recognition_items_total": 7}]},
    )
    mean, n = shadow_eval.parse_tool_report(rp)
    assert mean == pytest.approx(7.0)
    assert n == 1


def test_parse_tool_report_int_coercion(tmp_path: Path) -> None:
    # eval_distribution.py emits ints, but be tolerant of numeric strings.
    rp = _write_json(
        tmp_path / "tool.json",
        {"per_photo": [{"recognition_items_total": "4"}, {"recognition_items_total": 6}]},
    )
    mean, n = shadow_eval.parse_tool_report(rp)
    assert mean == pytest.approx(5.0)
    assert n == 2


def test_parse_tool_report_missing_per_photo(tmp_path: Path) -> None:
    rp = _write_json(tmp_path / "tool.json", {"some_other_key": []})
    with pytest.raises(SystemExit, match="missing or empty 'per_photo'"):
        shadow_eval.parse_tool_report(rp)


def test_parse_tool_report_empty_per_photo(tmp_path: Path) -> None:
    rp = _write_json(tmp_path / "tool.json", {"per_photo": []})
    with pytest.raises(SystemExit, match="missing or empty 'per_photo'"):
        shadow_eval.parse_tool_report(rp)


def test_parse_tool_report_missing_total(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "tool.json",
        {"per_photo": [{"recognition_items_total": 3}, {"other": 1}]},
    )
    with pytest.raises(SystemExit, match="missing recognition_items_total"):
        shadow_eval.parse_tool_report(rp)


# ---------------------------------------------------------------------------
# parse_face_report
# ---------------------------------------------------------------------------


def test_parse_face_report_happy(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "face.json",
        {"per_bucket_at_default": {"western": {"f1": 0.667, "n": 30}}},
    )
    f1, n = shadow_eval.parse_face_report(rp)
    assert f1 == pytest.approx(0.667)
    assert n == 30


def test_parse_face_report_zero_f1(tmp_path: Path) -> None:
    # F1 == 0.0 is a valid (if bad) score -- it is NOT null. Gate must accept it
    # so the caller can reject the candidate downstream.
    rp = _write_json(
        tmp_path / "face.json",
        {"per_bucket_at_default": {"western": {"f1": 0.0, "n": 20}}},
    )
    f1, n = shadow_eval.parse_face_report(rp)
    assert f1 == 0.0
    assert n == 20


def test_parse_face_report_missing_bucket(tmp_path: Path) -> None:
    rp = _write_json(tmp_path / "face.json", {"per_bucket_at_default": {}})
    with pytest.raises(SystemExit, match="missing per_bucket_at_default.western"):
        shadow_eval.parse_face_report(rp)


def test_parse_face_report_missing_outer(tmp_path: Path) -> None:
    rp = _write_json(tmp_path / "face.json", {"unrelated": 1})
    with pytest.raises(SystemExit, match="missing per_bucket_at_default.western"):
        shadow_eval.parse_face_report(rp)


def test_parse_face_report_null_f1(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "face.json",
        {"per_bucket_at_default": {"western": {"f1": None, "n": 5}}},
    )
    with pytest.raises(SystemExit, match="western F1 is null"):
        shadow_eval.parse_face_report(rp)


def test_parse_face_report_missing_n(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "face.json",
        {"per_bucket_at_default": {"western": {"f1": 0.5}}},
    )
    with pytest.raises(SystemExit, match=r"missing per_bucket_at_default\.western\.n"):
        shadow_eval.parse_face_report(rp)


def test_parse_face_report_n_must_be_int(tmp_path: Path) -> None:
    rp = _write_json(
        tmp_path / "face.json",
        {"per_bucket_at_default": {"western": {"f1": 0.5, "n": "30"}}},
    )
    # n is required to be an int -- string n is rejected.
    with pytest.raises(SystemExit, match=r"missing per_bucket_at_default\.western\.n"):
        shadow_eval.parse_face_report(rp)


# ---------------------------------------------------------------------------
# assemble_eval_deltas
# ---------------------------------------------------------------------------


def test_assemble_eval_deltas_full_shape() -> None:
    out = shadow_eval.assemble_eval_deltas(
        tool_current_mean=3.0,
        tool_candidate_mean=3.5,
        tool_n=42,
        face_current_f1=0.6,
        face_candidate_f1=0.7,
        face_n=20,
        current_onnx_sha="a" * 64,
        candidate_onnx_sha="b" * 64,
    )
    assert set(out.keys()) == {
        "tool",
        "face",
        "current_onnx_sha256",
        "candidate_onnx_sha256",
        "generated_at",
    }
    assert out["tool"] == {
        "current_recognition_items_mean": 3.0,
        "candidate_recognition_items_mean": 3.5,
        "delta": pytest.approx(0.5),
        "fixture_photos": 42,
    }
    assert out["face"] == {
        "current_western_f1": 0.6,
        "candidate_western_f1": 0.7,
        "delta": pytest.approx(0.10000000000000009),  # canonical FP residue
        "fixture_photos": 20,
    }
    assert out["current_onnx_sha256"] == "a" * 64
    assert out["candidate_onnx_sha256"] == "b" * 64
    assert re.fullmatch(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", out["generated_at"]
    )


def test_assemble_eval_deltas_negative_delta() -> None:
    # Candidate worse than current -- delta must be negative; gate caller
    # decides what to do with it.
    out = shadow_eval.assemble_eval_deltas(
        tool_current_mean=4.0,
        tool_candidate_mean=3.0,
        tool_n=10,
        face_current_f1=0.8,
        face_candidate_f1=0.5,
        face_n=10,
        current_onnx_sha="c" * 64,
        candidate_onnx_sha="d" * 64,
    )
    assert out["tool"]["delta"] == pytest.approx(-1.0)
    assert out["face"]["delta"] == pytest.approx(-0.3)


def test_assemble_eval_deltas_self_vs_self() -> None:
    # The wiring smoke (#7c-eval-self-wiring) feeds the same report on both
    # sides; deltas must be exactly zero and the two sha fields must match.
    same_sha = "16569e5752f09ecad96b22a0a1065c9a7c311e84254177ecdf2de541da13dca1"
    out = shadow_eval.assemble_eval_deltas(
        tool_current_mean=3.261904761904762,
        tool_candidate_mean=3.261904761904762,
        tool_n=42,
        face_current_f1=0.2222222222222222,
        face_candidate_f1=0.2222222222222222,
        face_n=20,
        current_onnx_sha=same_sha,
        candidate_onnx_sha=same_sha,
    )
    assert out["tool"]["delta"] == 0.0
    assert out["face"]["delta"] == 0.0
    assert out["current_onnx_sha256"] == out["candidate_onnx_sha256"] == same_sha


def test_assemble_eval_deltas_json_serialisable() -> None:
    # The blob must round-trip through json -- no datetime objects, no NaN, etc.
    out = shadow_eval.assemble_eval_deltas(
        tool_current_mean=1.0,
        tool_candidate_mean=1.0,
        tool_n=1,
        face_current_f1=0.5,
        face_candidate_f1=0.5,
        face_n=1,
        current_onnx_sha="e" * 64,
        candidate_onnx_sha="f" * 64,
    )
    blob = json.dumps(out)
    restored = json.loads(blob)
    assert restored == out
