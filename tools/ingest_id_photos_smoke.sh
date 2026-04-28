#!/usr/bin/env bash
# ingest_id_photos_smoke.sh — parse-only smoke for tools/ingest_id_photos.py.
#
# Builds a synthetic photos-dir of empty .jpg/.png files whose names cover
# the real-world grammar observed on 2026-04-28 plus a few edge cases, runs
# the parser, and asserts every file parses to the expected
# (employee_no, name, dept_tag) triple.
#
# Pure local; does NOT boot the server.
#
# Exit code 0 on success, non-zero on any assertion failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TOOL="$REPO_ROOT/tools/ingest_id_photos.py"

if [[ ! -f "$TOOL" ]]; then
    echo "✗ missing tool: $TOOL" >&2
    exit 1
fi

WORK_DIR="$(mktemp -d -t ingest-id-smoke-XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT
PHOTOS_DIR="$WORK_DIR/photos"
mkdir -p "$PHOTOS_DIR"

# ---------------------------------------------------------------------------
# Synthesize fixture filenames.
# Format: <printable-stem>|<expected-eid>|<expected-name>|<expected-dept-or-->
# Use '-' for dept when no dept tag is expected.
# ---------------------------------------------------------------------------

FIXTURES=(
    # Real-world samples from 2026-04-28 King turn (5 photos):
    "张芊齐+20171805.JPG|20171805|张芊齐|-"
    "张星宝20190746.png|20190746|张星宝|-"
    "杨嘉兴20251646网控电气.jpg|20251646|杨嘉兴|网控电气"
    "王健鸿20241247.jpg|20241247|王健鸿|-"
    "王楷煌+20241261.jpg|20241261|王楷煌|-"
    # Reverse order: <eid><name>.ext
    "20171805张芊齐.jpg|20171805|张芊齐|-"
    # Alternate separators
    "张三 20210001.jpg|20210001|张三|-"
    "李四_20220002.jpg|20220002|李四|-"
    "王五-20230003.jpg|20230003|王五|-"
    # Reverse + dept tag
    "20240004赵六车间一.jpg|20240004|赵六车间一|-"
    # ASCII-only name (defensive: should still work)
    "Alice+20180100.jpg|20180100|Alice|-"
)

for entry in "${FIXTURES[@]}"; do
    stem="${entry%%|*}"
    : > "$PHOTOS_DIR/$stem"
done

# Add a couple of bad files that should be reported as parse_failed.
: > "$PHOTOS_DIR/no_digits_at_all.jpg"
: > "$PHOTOS_DIR/short_1234.jpg"
EXPECTED_FAIL=2
EXPECTED_OK=${#FIXTURES[@]}
EXPECTED_TOTAL=$((EXPECTED_OK + EXPECTED_FAIL))

# Run the parser
MANIFEST="$WORK_DIR/manifest.json"
set +e
python3 "$TOOL" --mode parse-only --photos-dir "$PHOTOS_DIR" --manifest-out "$MANIFEST"
RC=$?
set -e
# parse-only exit code 2 = some entries failed (we expect that here)
if [[ $RC -ne 0 && $RC -ne 2 ]]; then
    echo "✗ parser exited with unexpected rc=$RC" >&2
    exit 1
fi

# Validate JSON shape via python (so we can use json + assertions cleanly).
python3 - "$MANIFEST" "$PHOTOS_DIR" <<'PY'
import json, sys
from pathlib import Path
manifest_path = Path(sys.argv[1])
photos_dir = Path(sys.argv[2])
m = json.loads(manifest_path.read_text(encoding="utf-8"))

expected = {
    "张芊齐+20171805":           ("20171805", "张芊齐", None),
    "张星宝20190746":             ("20190746", "张星宝", None),
    "杨嘉兴20251646网控电气":     ("20251646", "杨嘉兴", "网控电气"),
    "王健鸿20241247":             ("20241247", "王健鸿", None),
    "王楷煌+20241261":            ("20241261", "王楷煌", None),
    "20171805张芊齐":             ("20171805", "张芊齐", None),
    "张三 20210001":             ("20210001", "张三", None),
    "李四_20220002":             ("20220002", "李四", None),
    "王五-20230003":             ("20230003", "王五", None),
    "20240004赵六车间一":         ("20240004", "赵六车间一", None),
    "Alice+20180100":          ("20180100", "Alice", None),
}
expected_fail_stems = {"no_digits_at_all", "short_1234"}

s = m["summary"]
errors = []

if s["total_files"] != len(expected) + len(expected_fail_stems):
    errors.append(
        f"summary.total_files={s['total_files']}, expected={len(expected) + len(expected_fail_stems)}"
    )
if s["parsed_ok"] != len(expected):
    errors.append(f"summary.parsed_ok={s['parsed_ok']}, expected={len(expected)}")
if s["parse_failed"] != len(expected_fail_stems):
    errors.append(
        f"summary.parse_failed={s['parse_failed']}, expected={len(expected_fail_stems)}"
    )
if s["unique_employee_nos"] != len({eid for (eid, _, _) in expected.values()}):
    errors.append(
        f"summary.unique_employee_nos={s['unique_employee_nos']}, expected={len({eid for (eid,_,_) in expected.values()})}"
    )

# Each entry: stem -> parsed dict
by_stem = {}
for e in m["entries"]:
    stem = Path(e["path"]).stem
    by_stem[stem] = e

for stem, (want_eid, want_name, want_dept) in expected.items():
    e = by_stem.get(stem)
    if e is None:
        errors.append(f"missing entry for stem={stem!r}")
        continue
    if e["error"] is not None:
        errors.append(f"unexpected parse error for {stem!r}: {e['error']}")
        continue
    p = e["parsed"]
    if p["employee_no"] != want_eid:
        errors.append(f"{stem!r}: employee_no={p['employee_no']!r}, want={want_eid!r}")
    if p["name"] != want_name:
        errors.append(f"{stem!r}: name={p['name']!r}, want={want_name!r}")
    if p["dept_tag"] != want_dept:
        errors.append(f"{stem!r}: dept_tag={p['dept_tag']!r}, want={want_dept!r}")

for stem in expected_fail_stems:
    e = by_stem.get(stem)
    if e is None:
        errors.append(f"missing failure-entry for stem={stem!r}")
        continue
    if e["parsed"] is not None or not e["error"]:
        errors.append(f"{stem!r}: expected parse failure, got parsed={e['parsed']}")

# Duplicate-EID detection: 20171805 appears in two fixture rows.
if "20171805" not in s["duplicate_eids"]:
    errors.append(
        f"duplicate_eids missing 20171805 (got {s['duplicate_eids']})"
    )
elif s["duplicate_eids"]["20171805"] != 2:
    errors.append(
        f"duplicate_eids[20171805]={s['duplicate_eids']['20171805']}, expected 2"
    )

if errors:
    print("✗ schema/value mismatches:", file=sys.stderr)
    for err in errors:
        print(f"   - {err}", file=sys.stderr)
    raise SystemExit(1)

print("✓ all parse assertions passed")
print(f"  total_files       = {s['total_files']}")
print(f"  parsed_ok         = {s['parsed_ok']}")
print(f"  parse_failed      = {s['parse_failed']}")
print(f"  unique_employee_nos = {s['unique_employee_nos']}")
print(f"  duplicate_eids    = {s['duplicate_eids']}")
PY

# --help renders without crashing
python3 "$TOOL" --help > /dev/null

# --dry-run on ingest mode (still requires parsed entries, no project-id needed when dry-run)
python3 "$TOOL" --mode ingest --photos-dir "$PHOTOS_DIR" --dry-run --report-out "$WORK_DIR/dry.json" > /dev/null 2>&1 || {
    echo "✗ ingest --dry-run path crashed" >&2
    exit 1
}

echo ""
echo "✓✓✓ ingest_id_photos smoke PASSED"
