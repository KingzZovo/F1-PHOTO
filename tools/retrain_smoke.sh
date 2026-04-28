#!/usr/bin/env bash
#
# End-to-end smoke for the #7b-train pipeline.
#
# Builds a minimal fixture cycle directory under $TMPDIR (5 images from the
# existing #2-tool baseline fixtures, auto-labelled with the baseline yolov8n
# top-1 detection per image → single class "tool"), invokes
# tools/retrain_train.py for a 1-epoch training run, and validates that the
# exported ONNX has the shape the server's #7b-prep shape-tolerant decoder
# expects ([1, 5, 8400] for nc=1).
#
# Exits non-zero on any failure. Idempotent: cleans/recreates the cycle dir.
#
# Tunable env:
#   PYTHON   Python interpreter (default /root/notion-local-ops-mcp/.venv/bin/python3)
#   YOLO_PT  Base weights (default /tmp/yolo_export/yolov8n.pt)
#   SMOKE_DIR Cycle dir (default /tmp/yolo_smoke/cycle-test)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PYTHON="${PYTHON:-/root/notion-local-ops-mcp/.venv/bin/python3}"
YOLO_PT="${YOLO_PT:-/tmp/yolo_export/yolov8n.pt}"
SMOKE_DIR="${SMOKE_DIR:-/tmp/yolo_smoke/cycle-test}"
FIXTURE_ROOT="$ROOT/tests/fixtures/tool/baseline"

if [[ ! -x "$PYTHON" ]]; then
    echo "✗ PYTHON=$PYTHON not executable" >&2
    exit 1
fi
if [[ ! -d "$FIXTURE_ROOT" ]]; then
    echo "✗ fixture root missing: $FIXTURE_ROOT" >&2
    exit 1
fi

echo "▶ cleaning $SMOKE_DIR"
rm -rf "$SMOKE_DIR"
mkdir -p "$SMOKE_DIR/images" "$SMOKE_DIR/labels"

echo "▶ picking 5 fixture images and auto-labelling via baseline yolov8n"
"$PYTHON" - <<PYEOF
import json, sys
from pathlib import Path
from ultralytics import YOLO

root = Path("$FIXTURE_ROOT")
out_imgs = Path("$SMOKE_DIR/images")
out_lbls = Path("$SMOKE_DIR/labels")

# Pick 5 deterministic images from a few different classes for variety.
pick = [
    root / "bowl" / "000000221872.jpg",
    root / "scissors" / "000000351096.jpg",
    root / "oven" / "000000344795.jpg",
    root / "laptop" / sorted((root / "laptop").glob("*.jpg"))[0].name if (root / "laptop").exists() else root / "bowl" / "000000521601.jpg",
    root / "cell_phone" / sorted((root / "cell_phone").glob("*.jpg"))[0].name if (root / "cell_phone").exists() else root / "bowl" / "000000099053.jpg",
]
pick = [p for p in pick if p.exists()][:5]
if len(pick) < 5:
    print(json.dumps({"status":"error","message":f"only found {len(pick)} fixture images"}), file=sys.stderr)
    sys.exit(2)

model = YOLO("$YOLO_PT")
results = model.predict(source=[str(p) for p in pick], conf=0.05, imgsz=640, verbose=False, save=False)

wrote = 0
for src, res in zip(pick, results):
    dst_img = out_imgs / src.name
    dst_lbl = out_lbls / (src.stem + ".txt")
    # Hard-link first; copy fallback (mirrors retrain.rs prepare).
    try:
        if dst_img.exists():
            dst_img.unlink()
        dst_img.hardlink_to(src)
    except OSError:
        import shutil; shutil.copyfile(src, dst_img)

    h, w = res.orig_shape  # (h, w)
    boxes = res.boxes
    if boxes is None or len(boxes) == 0:
        # Fallback: use a centred 60% box covering the image.
        cx, cy, bw, bh = 0.5, 0.5, 0.6, 0.6
    else:
        # Take the highest-confidence box, relabel as class 0 ("tool").
        confs = boxes.conf.cpu().numpy()
        idx = int(confs.argmax())
        x1, y1, x2, y2 = boxes.xyxy[idx].cpu().numpy().tolist()
        cx = ((x1 + x2) / 2.0) / w
        cy = ((y1 + y2) / 2.0) / h
        bw = (x2 - x1) / w
        bh = (y2 - y1) / h
        cx = max(0.0, min(1.0, cx))
        cy = max(0.0, min(1.0, cy))
        bw = max(1e-3, min(1.0, bw))
        bh = max(1e-3, min(1.0, bh))
    dst_lbl.write_text(f"0 {cx:.6f} {cy:.6f} {bw:.6f} {bh:.6f}\n")
    wrote += 1

print(json.dumps({"status":"ok","wrote":wrote,"images":[str(p.name) for p in pick]}))
PYEOF

echo "▶ writing data.yaml"
cat > "$SMOKE_DIR/data.yaml" <<YAMLEOF
path: $SMOKE_DIR
train: images
val: images
nc: 1
names:
  0: tool
YAMLEOF

ls "$SMOKE_DIR/images" | sed 's/^/  img: /'
ls "$SMOKE_DIR/labels" | sed 's/^/  lbl: /'

echo "▶ invoking tools/retrain_train.py (1 epoch, imgsz=320, batch=2, freeze=10, cpu)"
"$PYTHON" "$ROOT/tools/retrain_train.py" \
    --cycle-dir "$SMOKE_DIR" \
    --base-weights "$YOLO_PT" \
    --epochs 1 \
    --imgsz 320 \
    --export-imgsz 640 \
    --batch 2 \
    --workers 0 \
    --freeze 10 \
    --device cpu \
    --opset 12 \
    --runs-dir "$SMOKE_DIR/runs" \
    --run-name smoke \
    --candidate-out "$SMOKE_DIR/object_detect.candidate.onnx" \
    --summary-out "$SMOKE_DIR/summary.json" \
    >"$SMOKE_DIR/stdout.log" 2>&1

if [[ ! -s "$SMOKE_DIR/summary.json" ]]; then
    echo "✗ retrain_train.py did not write summary.json; full stdout/stderr:" >&2
    cat "$SMOKE_DIR/stdout.log" >&2
    exit 5
fi

echo "▶ verifying exported ONNX shape with the server's #7b-prep contract"
"$PYTHON" - <<PYEOF
import json, sys
from pathlib import Path
import onnxruntime as ort

summary = json.loads(Path("$SMOKE_DIR/summary.json").read_text())
if summary.get("status") != "ok":
    print("✗ retrain_train.py reported non-ok status:", summary, file=sys.stderr); sys.exit(3)

candidate = Path(summary["candidate_onnx"])
sess = ort.InferenceSession(str(candidate), providers=["CPUExecutionProvider"])
outs = sess.get_outputs()
shape = list(outs[0].shape)
# nc=1 → channels=5; NUM_ANCHORS=8400 (server constant). Server #7b-prep accepts
# any nc ≥ 1, so the only invariants we lock here are: 3D tensor, channel
# dim == 5, anchors == 8400. (Batch dim may be int 1 or symbolic.)
ok = (len(shape) == 3 and shape[1] == 5 and shape[2] == 8400)
if not ok:
    print(f"✗ unexpected output shape {shape}; want [1, 5, 8400]", file=sys.stderr); sys.exit(4)
print(f"✓ candidate ONNX shape valid: {shape} (nc=1 → 4+1=5 channels, 8400 anchors)")
print(f"✓ candidate size: {candidate.stat().st_size} bytes")
PYEOF

echo "✓✓✓ RETRAIN SMOKE PASSED"
