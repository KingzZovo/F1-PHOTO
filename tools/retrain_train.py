#!/usr/bin/env python3
"""Retrain the YOLOv8 tool detector from a #7a cycle directory and export ONNX.

This script is the Python half of the #7b-train pipeline. It is invoked by the
`f1photo retrain-detector train` Rust subcommand (see #7b-train) and can also be
run standalone by operators (`tools/retrain_smoke.sh` exercises it on a tiny
fixture cycle for CI).

Inputs:
  --cycle-dir DIR        Cycle directory written by #7a (`prepare`); must contain
                         data.yaml + images/ + labels/.
  --base-weights PATH    YOLOv8 .pt weights to fine-tune from (default: cached
                         /tmp/yolo_export/yolov8n.pt; falls back to ultralytics
                         download if absent).
  --epochs N             Training epochs (default 20).
  --imgsz N              Training image size (default 640).
  --freeze N             Number of backbone layers to freeze (default 10 →
                         freeze backbone, train head only — fast + low-data
                         friendly).
  --batch N              Batch size (default 4; CPU-friendly).
  --workers N            DataLoader workers (default 0; safe in containers).
  --device STR           Torch device (default "cpu"; "0"/"cuda" for GPU).
  --runs-dir DIR         Where ultralytics writes its `train/<name>/` tree
                         (default <cycle-dir>/runs).
  --run-name NAME        Sub-directory name under runs-dir (default "train").
  --candidate-out PATH   Final ONNX path; we copy the exported file here
                         atomically (default <cycle-dir>/object_detect.candidate.onnx).
  --opset N              ONNX opset (default 12 — matches production baseline).

Outputs (on success):
  - <runs-dir>/<run-name>/weights/best.pt        (ultralytics)
  - <runs-dir>/<run-name>/weights/best.onnx      (ultralytics export)
  - <candidate-out>                              (atomic copy of best.onnx)
  - JSON summary on stdout: {"status":"ok", "candidate_onnx":..., "output_shape":[...], ...}

On failure: non-zero exit + JSON {"status":"error", "message":...} on stderr.

Validates that the exported ONNX has output shape [1, 4 + nc, NUM_ANCHORS] where
NUM_ANCHORS=8400 (server-side `yolov8.rs` constant) and nc matches data.yaml.
The server's #7b-prep shape-tolerant decoder will accept any nc ≥ 1.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path

# Server-side constant (NUM_ANCHORS in server/src/inference/yolov8.rs); must
# match for the candidate to load. ultralytics produces this for imgsz=640.
EXPECTED_NUM_ANCHORS = 8400


def fail(msg: str, code: int = 1) -> None:
    print(json.dumps({"status": "error", "message": msg}), file=sys.stderr)
    sys.exit(code)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cycle-dir", required=True, type=Path)
    p.add_argument("--base-weights", default=Path("/tmp/yolo_export/yolov8n.pt"), type=Path)
    p.add_argument("--epochs", type=int, default=20)
    p.add_argument("--imgsz", type=int, default=640)
    p.add_argument("--freeze", type=int, default=10)
    p.add_argument("--batch", type=int, default=4)
    p.add_argument("--workers", type=int, default=0)
    p.add_argument("--device", default="cpu")
    p.add_argument("--runs-dir", type=Path, default=None)
    p.add_argument("--run-name", default="train")
    p.add_argument("--candidate-out", type=Path, default=None)
    p.add_argument("--opset", type=int, default=12)
    # Export imgsz is decoupled from train imgsz: the server's NUM_ANCHORS=8400
    # constant assumes 640×640 input (80² + 40² + 20² = 8400 anchors). Smoke /
    # CI may train at imgsz=320 for speed, but production candidates must export
    # at 640 to match the server-side decoder anchor grid.
    p.add_argument("--export-imgsz", type=int, default=None,
                   help="ONNX export imgsz (default: same as --imgsz; pass 640 to match server NUM_ANCHORS=8400)")
    p.add_argument("--summary-out", type=Path, default=None,
                   help="Optional path to write the JSON summary to (in addition to stdout). "
                        "Recommended for callers (e.g. the #7b-train Rust CLI or smoke scripts) "
                        "that need to parse the summary cleanly without ultralytics' stdout noise.")
    return p.parse_args()


def load_data_yaml(cycle_dir: Path) -> dict:
    data_yaml = cycle_dir / "data.yaml"
    if not data_yaml.exists():
        fail(f"data.yaml not found at {data_yaml}")
    # Avoid hard PyYAML dep — ultralytics already pulls it, but keep parsing
    # minimal here so this script can also be lint-checked without ultralytics.
    import yaml  # type: ignore[import-not-found]
    with data_yaml.open() as fh:
        cfg = yaml.safe_load(fh)
    if not isinstance(cfg, dict):
        fail(f"data.yaml is not a mapping: {data_yaml}")
    if "nc" not in cfg:
        fail(f"data.yaml missing required key 'nc': {data_yaml}")
    return cfg


def main() -> None:
    args = parse_args()
    cycle_dir = args.cycle_dir.resolve()
    if not cycle_dir.is_dir():
        fail(f"cycle-dir is not a directory: {cycle_dir}")
    cfg = load_data_yaml(cycle_dir)
    nc = int(cfg["nc"])
    expected_channels = 4 + nc

    runs_dir = (args.runs_dir or (cycle_dir / "runs")).resolve()
    runs_dir.mkdir(parents=True, exist_ok=True)
    candidate_out = (args.candidate_out or (cycle_dir / "object_detect.candidate.onnx")).resolve()
    candidate_out.parent.mkdir(parents=True, exist_ok=True)

    base_weights = args.base_weights.resolve()
    if not base_weights.exists():
        # ultralytics will auto-download yolov8n.pt by name; surface what we used.
        print(f"[retrain_train] base weights {base_weights} missing; falling back to 'yolov8n.pt' (ultralytics auto-download)", file=sys.stderr)
        base_weights_arg: str = "yolov8n.pt"
    else:
        base_weights_arg = str(base_weights)

    # Import ultralytics late so --help works without it.
    try:
        from ultralytics import YOLO  # type: ignore[import-not-found]
    except Exception as exc:  # pragma: no cover
        fail(f"ultralytics import failed: {exc!r}")

    t0 = time.time()
    print(f"[retrain_train] loading base weights: {base_weights_arg}", file=sys.stderr)
    model = YOLO(base_weights_arg)

    print(f"[retrain_train] training: data={cycle_dir / 'data.yaml'} epochs={args.epochs} imgsz={args.imgsz} freeze={args.freeze} batch={args.batch} device={args.device}", file=sys.stderr)
    train_kwargs = dict(
        data=str(cycle_dir / "data.yaml"),
        epochs=args.epochs,
        imgsz=args.imgsz,
        freeze=args.freeze,
        batch=args.batch,
        workers=args.workers,
        device=args.device,
        project=str(runs_dir),
        name=args.run_name,
        exist_ok=True,
        verbose=False,
        plots=False,
        save=True,
        # CPU-friendly defaults for the smoke; production callers can override.
        cache=False,
        amp=False,
    )
    train_result = model.train(**train_kwargs)  # noqa: F841
    train_dir = runs_dir / args.run_name
    best_pt = train_dir / "weights" / "best.pt"
    if not best_pt.exists():
        # When epochs<=1 ultralytics may not write best.pt — fall back to last.pt.
        last_pt = train_dir / "weights" / "last.pt"
        if last_pt.exists():
            best_pt = last_pt
        else:
            fail(f"no weights produced at {train_dir / 'weights'}")

    train_secs = time.time() - t0

    print(f"[retrain_train] exporting ONNX from {best_pt} (opset={args.opset}, imgsz={args.imgsz})", file=sys.stderr)
    export_model = YOLO(str(best_pt))
    export_imgsz = args.export_imgsz if args.export_imgsz is not None else args.imgsz
    print(f"[retrain_train] export imgsz={export_imgsz} (anchor grid = (imgsz/8)² + (imgsz/16)² + (imgsz/32)²)", file=sys.stderr)
    onnx_path_str = export_model.export(
        format="onnx",
        opset=args.opset,
        imgsz=export_imgsz,
        dynamic=False,
        simplify=False,
        device=args.device,
    )
    onnx_path = Path(onnx_path_str)
    if not onnx_path.exists():
        fail(f"export reported {onnx_path_str} but file missing")

    # Atomic copy to candidate-out (cross-fs safe).
    tmp_out = candidate_out.with_suffix(candidate_out.suffix + ".tmp")
    shutil.copyfile(onnx_path, tmp_out)
    os.replace(tmp_out, candidate_out)

    # Validate output tensor shape with onnxruntime.
    try:
        import onnxruntime as ort  # type: ignore[import-not-found]
    except Exception as exc:  # pragma: no cover
        fail(f"onnxruntime import failed: {exc!r}")
    sess = ort.InferenceSession(str(candidate_out), providers=["CPUExecutionProvider"])
    outs = sess.get_outputs()
    if not outs:
        fail("exported ONNX has no outputs")
    out0_shape = list(outs[0].shape)
    # Expected [1, 4+nc, NUM_ANCHORS] (str dims allowed for batch).
    shape_ok = (
        len(out0_shape) == 3
        and (out0_shape[0] in (1, "batch", None))
        and out0_shape[1] == expected_channels
        and out0_shape[2] == EXPECTED_NUM_ANCHORS
    )
    if not shape_ok:
        fail(
            f"output shape mismatch: got {out0_shape}, expected [1, {expected_channels}, {EXPECTED_NUM_ANCHORS}] (4 + nc={nc} channels, {EXPECTED_NUM_ANCHORS} anchors). Server #7b-prep decoder accepts any nc ≥ 1 but anchor count must match."
        )
    out_size = candidate_out.stat().st_size

    summary = {
        "status": "ok",
        "cycle_dir": str(cycle_dir),
        "base_weights": base_weights_arg,
        "candidate_onnx": str(candidate_out),
        "candidate_size_bytes": out_size,
        "output_shape": out0_shape,
        "nc": nc,
        "expected_channels": expected_channels,
        "epochs": args.epochs,
        "imgsz": args.imgsz,
        "export_imgsz": export_imgsz,
        "freeze": args.freeze,
        "batch": args.batch,
        "device": args.device,
        "opset": args.opset,
        "train_seconds": round(train_secs, 1),
        "weights_pt": str(best_pt),
        "runs_dir": str(runs_dir),
    }
    summary_json = json.dumps(summary)
    if args.summary_out is not None:
        out = args.summary_out.resolve()
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = out.with_suffix(out.suffix + ".tmp")
        tmp.write_text(summary_json + "\n")
        os.replace(tmp, out)
    print(summary_json)


if __name__ == "__main__":
    main()
