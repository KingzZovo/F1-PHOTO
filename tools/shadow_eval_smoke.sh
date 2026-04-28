#!/usr/bin/env bash
#
# Milestone #7c-eval-auto smoke for tools/shadow_eval.py.
#
# Exercises ONLY the --from-reports assembly path: derives 4 synthetic eval
# reports from the existing #2/#2c-tune baseline JSONs (mutated to simulate a
# strictly-improving candidate), runs shadow_eval.py to produce
# eval_deltas.json, then validates the resulting blob against the Rust
# EvalDeltas schema (server::retrain::load_eval_deltas) by spot-checking key
# sets, types, delta math, sha256 hex shape, and the gate floor invariants
# (tool delta strictly negative, face F1 >= 0.667).
#
# Live-mode (4 server boots) end-to-end smoke is OUT OF SCOPE here — it
# requires bundled-pg + ORT runtime as f1u, and is exercised by manually
# invoking shadow_eval.py with --candidate against a real candidate ONNX.
#
# Run from the repo root:
#   bash tools/shadow_eval_smoke.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

echo "→ work dir: $WORK"
echo "→ root:     $ROOT"

TOOL_CUR="$ROOT/docs/baselines/2-distribution-tool-baseline.json"
FACE_CUR="$ROOT/docs/baselines/2c-tune-recognition-pr.json"
[ -f "$TOOL_CUR" ] || { echo "✗ missing $TOOL_CUR" >&2; exit 1; }
[ -f "$FACE_CUR" ] || { echo "✗ missing $FACE_CUR" >&2; exit 1; }

# Synthesize "candidate" reports that beat the gate.
#   tool: subtract 1 from each per-photo recognition_items_total (clamp ≥0)
#         → strict mean drop
#   face: bump per_bucket_at_default.western.f1 to 0.95 (>= 0.667 floor)
TOOL_CAND="$WORK/tool_candidate.json"
FACE_CAND="$WORK/face_candidate.json"
python3 - "$TOOL_CUR" "$TOOL_CAND" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
d = json.loads(open(src).read())
for p in d["per_photo"]:
    p["recognition_items_total"] = max(0, int(p["recognition_items_total"]) - 1)
open(dst, "w").write(json.dumps(d, indent=2, sort_keys=True))
PY
python3 - "$FACE_CUR" "$FACE_CAND" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
d = json.loads(open(src).read())
d["per_bucket_at_default"]["western"]["f1"] = 0.95
open(dst, "w").write(json.dumps(d, indent=2, ensure_ascii=False) + "\n")
PY

# Synthetic ONNX files (only sha256 of the bytes is consumed).
CUR_ONNX="$WORK/current.onnx"
CAND_ONNX="$WORK/candidate.onnx"
printf 'synthetic-current-onnx-content\n' > "$CUR_ONNX"
printf 'synthetic-candidate-onnx-content\n' > "$CAND_ONNX"

OUT="$WORK/eval_deltas.json"
echo "▶ running shadow_eval.py --from-reports"
python3 "$ROOT/tools/shadow_eval.py" --from-reports \
    --tool-current-report  "$TOOL_CUR" \
    --tool-candidate-report "$TOOL_CAND" \
    --face-current-report  "$FACE_CUR" \
    --face-candidate-report "$FACE_CAND" \
    --current-onnx   "$CUR_ONNX" \
    --candidate-onnx "$CAND_ONNX" \
    --out "$OUT" > "$WORK/orchestrator.log" 2>&1
echo "✓ shadow_eval.py exit 0"
echo "--- eval_deltas.json ---"
cat "$OUT"
echo "--- end eval_deltas.json ---"

echo "▶ schema validation"
python3 - "$OUT" <<'PY'
import json, sys, re
d = json.load(open(sys.argv[1]))
expected_top = {"tool", "face", "current_onnx_sha256", "candidate_onnx_sha256", "generated_at"}
assert set(d) == expected_top, f"top-level keys mismatch: {set(d)}"

expected_tool = {"current_recognition_items_mean", "candidate_recognition_items_mean", "delta", "fixture_photos"}
assert set(d["tool"]) == expected_tool, f"tool keys mismatch: {set(d['tool'])}"
expected_face = {"current_western_f1", "candidate_western_f1", "delta", "fixture_photos"}
assert set(d["face"]) == expected_face, f"face keys mismatch: {set(d['face'])}"

for side, cur_key, cand_key in (
    ("tool", "current_recognition_items_mean", "candidate_recognition_items_mean"),
    ("face", "current_western_f1", "candidate_western_f1"),
):
    blk = d[side]
    cur, cand = blk[cur_key], blk[cand_key]
    assert isinstance(cur, (int, float)), (side, cur)
    assert isinstance(cand, (int, float)), (side, cand)
    assert abs(blk["delta"] - (cand - cur)) < 1e-9, (side, blk["delta"], cand - cur)
    assert isinstance(blk["fixture_photos"], int) and blk["fixture_photos"] > 0, (side, blk["fixture_photos"])

# Gate invariants for the synthetic candidate.
assert d["tool"]["delta"] < 0, ("tool delta should be strictly negative", d["tool"])
assert d["face"]["candidate_western_f1"] >= 0.667, ("face F1 below floor", d["face"])
assert d["face"]["current_western_f1"] < d["face"]["candidate_western_f1"], ("face delta should be positive", d["face"])

# sha256 shape: 64 lowercase hex, distinct.
sha_re = re.compile(r"^[0-9a-f]{64}$")
assert sha_re.match(d["current_onnx_sha256"]),   d["current_onnx_sha256"]
assert sha_re.match(d["candidate_onnx_sha256"]), d["candidate_onnx_sha256"]
assert d["current_onnx_sha256"] != d["candidate_onnx_sha256"], "shas should differ"

# generated_at: ISO-8601 with Z suffix, no microseconds.
assert d["generated_at"].endswith("Z"), d["generated_at"]
assert "." not in d["generated_at"], ("unexpected microseconds in generated_at", d["generated_at"])

# Numeric sanity from the synthetic mutation:
#   tool current mean from baseline is well-defined; candidate mean = current - photos_with_ge_1 / total
#   We don't pin exact numbers (avoid baseline coupling), but require fixture_photos == 42 (the #2-tool fixture).
assert d["tool"]["fixture_photos"] == 42, ("tool fixture should be 42 photos", d["tool"])
# face: the #2c-tune fixture has western n=20.
assert d["face"]["fixture_photos"] == 20, ("face fixture should be 20 western photos", d["face"])

print("✓ schema + gate invariants OK")
print(f"  tool: {d['tool']['current_recognition_items_mean']:.4f} → {d['tool']['candidate_recognition_items_mean']:.4f} (Δ={d['tool']['delta']:+.4f}, n={d['tool']['fixture_photos']})")
print(f"  face: {d['face']['current_western_f1']:.4f} → {d['face']['candidate_western_f1']:.4f} (Δ={d['face']['delta']:+.4f}, n={d['face']['fixture_photos']})")
PY

echo "✓✓✓ shadow-eval smoke PASSED"
