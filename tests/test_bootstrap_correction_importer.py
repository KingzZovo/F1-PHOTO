"""Unit tests for tools/bootstrap_correction_importer.py.

Covers the deterministic, no-IO surface of the importer:
- _truthy() canonical inputs
- classify() across all 5 categories (unreviewed / suppress / set / clear / invalid)
- classify() invalid sub-cases (half-filled rows, bad owner_type) -- the
  pre-flight failure path that prevents bad rows from ever reaching psql / HTTP
- read_csv() column -> ImportRow mapping (line_no, all 14 columns)
- build_summary() per-category aggregation + failed_rows extraction

Not covered (require live psql / HTTP, exercised by docs/baselines/5-bootstrap-importer-smoke.json):
  validate_owners, lookup_item_ids, login, patch_correct, main.
"""

import csv
import importlib.util
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
IMPORTER_PATH = REPO_ROOT / "tools" / "bootstrap_correction_importer.py"


def _load_importer():
    spec = importlib.util.spec_from_file_location(
        "bootstrap_correction_importer", str(IMPORTER_PATH)
    )
    mod = importlib.util.module_from_spec(spec)
    sys.modules["bootstrap_correction_importer"] = mod
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture(scope="module")
def imp():
    return _load_importer()


@pytest.fixture
def make_row(imp):
    """Build an ImportRow with all reviewer-fillable fields blank."""
    def _factory(**overrides):
        defaults = dict(
            line_no=2,
            detection_id="123",
            project_id="cac02bd7-bfff-41cc-b23b-8230018510d3",
            project_code="proj-a",
            target_type="tool",
            match_status="unmatched",
            score="0.85",
            class_id="0",
            corrected_label="",
            corrected_owner_type="",
            corrected_owner_id="",
            suppress="",
            reviewer="",
            reviewed_at="",
            notes="",
        )
        defaults.update(overrides)
        return imp.ImportRow(**defaults)
    return _factory


# ---------------------------------------------------------- _truthy

@pytest.mark.parametrize(
    "s",
    ["1", "true", "True", "TRUE", "yes", "YES", "y", "Y", "t", "T", " 1 ", "  yes  "],
)
def test_truthy_true(imp, s):
    assert imp._truthy(s) is True


@pytest.mark.parametrize(
    "s",
    ["", "0", "false", "False", "no", "n", "f", "maybe", "    "],
)
def test_truthy_false(imp, s):
    assert imp._truthy(s) is False


# ---------------------------------------------------------- classify (5 categories)

def test_classify_unreviewed_all_blank(imp, make_row):
    r = make_row()
    imp.classify(r)
    assert r.category == "unreviewed"
    assert r.invalid_reason == ""


def test_classify_suppress_minimal(imp, make_row):
    r = make_row(suppress="true")
    imp.classify(r)
    assert r.category == "suppress"


def test_classify_suppress_overrides_set(imp, make_row):
    """suppress=true takes precedence even if owner fields are filled."""
    r = make_row(
        suppress="1",
        corrected_owner_type="tool",
        corrected_owner_id="00000000-0000-0000-0000-000000000000",
        reviewer="king",
    )
    imp.classify(r)
    assert r.category == "suppress"


def test_classify_set_person(imp, make_row):
    r = make_row(
        corrected_owner_type="person",
        corrected_owner_id="383b613a-c5af-4033-9e44-e08c71bb1144",
        reviewer="king",
    )
    imp.classify(r)
    assert r.category == "set"
    assert r.invalid_reason == ""


def test_classify_set_tool(imp, make_row):
    r = make_row(
        corrected_owner_type="tool",
        corrected_owner_id="11111111-1111-1111-1111-111111111111",
    )
    imp.classify(r)
    assert r.category == "set"


def test_classify_set_device(imp, make_row):
    r = make_row(
        corrected_owner_type="device",
        corrected_owner_id="22222222-2222-2222-2222-222222222222",
    )
    imp.classify(r)
    assert r.category == "set"


def test_classify_set_invalid_owner_type(imp, make_row):
    r = make_row(
        corrected_owner_type="alien",
        corrected_owner_id="33333333-3333-3333-3333-333333333333",
    )
    imp.classify(r)
    assert r.category == "invalid"
    assert "alien" in r.invalid_reason
    # surfaces VALID_OWNER_TYPES tuple in the reason text
    assert "person" in r.invalid_reason


def test_classify_clear_with_reviewer(imp, make_row):
    r = make_row(reviewer="king")
    imp.classify(r)
    assert r.category == "clear"


def test_classify_clear_with_reviewer_and_notes(imp, make_row):
    r = make_row(reviewer="king", notes="reverted spurious match")
    imp.classify(r)
    assert r.category == "clear"


# ---------------------------------------------------------- classify (invalid sub-cases / pre-flight failure)

def test_classify_invalid_owner_type_only(imp, make_row):
    r = make_row(corrected_owner_type="tool")
    imp.classify(r)
    assert r.category == "invalid"
    assert "owner_id blank" in r.invalid_reason


def test_classify_invalid_owner_id_only(imp, make_row):
    r = make_row(corrected_owner_id="44444444-4444-4444-4444-444444444444")
    imp.classify(r)
    assert r.category == "invalid"
    assert "owner_type blank" in r.invalid_reason


def test_classify_invalid_label_only_no_reviewer(imp, make_row):
    """Label / notes filled but no reviewer and no owner -> ambiguous, invalid.

    This is the pre-flight failure path that catches half-filled rows before
    they ever reach psql validate_owners / HTTP PATCH.
    """
    r = make_row(corrected_label="badge")
    imp.classify(r)
    assert r.category == "invalid"
    assert "half-filled" in r.invalid_reason


# ---------------------------------------------------------- read_csv round-trip

def test_read_csv_roundtrip(imp, tmp_path):
    csv_path = tmp_path / "smoke.csv"
    fields = [
        "detection_id", "project_id", "project_code", "target_type",
        "match_status", "score", "class_id", "corrected_label",
        "corrected_owner_type", "corrected_owner_id", "suppress",
        "reviewer", "reviewed_at", "notes",
    ]
    rows = [
        {
            "detection_id": "100", "project_id": "p1", "project_code": "C1",
            "target_type": "tool", "match_status": "unmatched", "score": "0.9",
            "class_id": "0", "corrected_label": "", "corrected_owner_type": "tool",
            "corrected_owner_id": "uuid1", "suppress": "",
            "reviewer": "king", "reviewed_at": "2026-04-29", "notes": "",
        },
        {
            "detection_id": "101", "project_id": "p2", "project_code": "C2",
            "target_type": "person", "match_status": "matched", "score": "0.95",
            "class_id": "", "corrected_label": "", "corrected_owner_type": "",
            "corrected_owner_id": "", "suppress": "true",
            "reviewer": "king", "reviewed_at": "2026-04-29", "notes": "fp",
        },
    ]
    with csv_path.open("w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=fields)
        w.writeheader()
        w.writerows(rows)

    out = imp.read_csv(csv_path)
    assert len(out) == 2
    assert out[0].line_no == 2  # first data row after header
    assert out[1].line_no == 3
    assert out[0].detection_id == "100"
    assert out[0].corrected_owner_type == "tool"
    assert out[0].corrected_owner_id == "uuid1"
    assert out[1].suppress == "true"
    assert out[1].notes == "fp"


def test_read_csv_missing_optional_fields(imp, tmp_path):
    """read_csv tolerates a CSV that lacks some reviewer columns -> blanks."""
    csv_path = tmp_path / "min.csv"
    fields = ["detection_id", "project_id", "target_type"]
    with csv_path.open("w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=fields)
        w.writeheader()
        w.writerow({"detection_id": "5", "project_id": "p", "target_type": "tool"})
    out = imp.read_csv(csv_path)
    assert len(out) == 1
    assert out[0].detection_id == "5"
    assert out[0].corrected_owner_type == ""
    assert out[0].suppress == ""
    assert out[0].notes == ""


# ---------------------------------------------------------- build_summary

def test_build_summary_counts_each_bucket(imp, make_row):
    rows = [
        make_row(),  # unreviewed
        make_row(suppress="true"),  # suppress
        make_row(corrected_owner_type="tool", corrected_owner_id="u1"),  # set
        make_row(corrected_owner_type="person", corrected_owner_id="u2"),  # set
        make_row(reviewer="king"),  # clear
        make_row(corrected_owner_type="alien", corrected_owner_id="u3"),  # invalid
    ]
    for r in rows:
        imp.classify(r)
    rows[5].error = "bad owner_type"

    summary = imp.build_summary(rows)
    assert summary["total_rows"] == 6
    assert summary["by_category"] == {
        "unreviewed": 1, "suppress": 1, "set": 2, "clear": 1, "invalid": 1,
    }
    assert summary["set_by_corrected_owner_type"] == {"tool": 1, "person": 1}
    # default factory target_type="tool" on all rows
    assert summary["set_by_target_type"] == {"tool": 2}
    # only the row with explicit error gets into failed_rows
    assert len(summary["failed_rows"]) == 1
    assert summary["failed_rows"][0]["category"] == "invalid"
    assert summary["failed_rows"][0]["error"] == "bad owner_type"


def test_build_summary_records_invalid_reason_in_failed(imp, make_row):
    """An invalid row with an explicit error surfaces invalid_reason in audit."""
    r = make_row(corrected_owner_type="alien", corrected_owner_id="u")
    imp.classify(r)
    r.error = "skipped: invalid"
    s = imp.build_summary([r])
    assert s["failed_rows"][0]["invalid_reason"]
    assert "alien" in s["failed_rows"][0]["invalid_reason"]


def test_build_summary_http_status_distribution(imp, make_row):
    """http_status is bucketed by string key in the audit summary."""
    r1 = make_row(corrected_owner_type="tool", corrected_owner_id="u1")
    r2 = make_row(corrected_owner_type="tool", corrected_owner_id="u2")
    r3 = make_row(corrected_owner_type="person", corrected_owner_id="u3")
    for r in (r1, r2, r3):
        imp.classify(r)
    r1.http_status = 200
    r2.http_status = 200
    r3.http_status = 422
    s = imp.build_summary([r1, r2, r3])
    assert s["http_status_distribution"] == {"200": 2, "422": 1}
