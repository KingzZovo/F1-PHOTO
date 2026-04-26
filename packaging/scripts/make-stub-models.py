#!/usr/bin/env python3
"""
Generate minimal valid ONNX models for f1-photo's five model slots.

These stubs are *load-test fixtures*: they implement an Identity op with
dynamic-rank tensors so `ort::Session::commit_from_file` succeeds and
`ModelRegistry::ready()` returns true. They are NOT real face-recognition
models — the recall worker will produce vacuous outputs when given these.
Use real ONNX exports from tools/train_*.py for production.

Written files (under --out, default `./models/`), all ~150 bytes each:
  face_detect.onnx       Identity, dynamic 4D input/output
  face_embed.onnx        Identity, dynamic 4D
  object_detect.onnx     Identity, dynamic 4D
  generic_embed.onnx     Identity, dynamic 4D
  angle_classify.onnx    Identity, dynamic 4D
"""
import argparse
import os
import sys

import onnx
from onnx import TensorProto, helper


def _save(model: onnx.ModelProto, path: str) -> None:
    onnx.checker.check_model(model)
    with open(path, "wb") as f:
        f.write(model.SerializeToString())
    print(f"  wrote {path}  ({os.path.getsize(path)} bytes)")


def _identity_model(name: str) -> onnx.ModelProto:
    """Build a dynamic-rank Identity ONNX model. Input/output names are
    'input' / 'output'; shapes are 4D with all-symbolic dims."""
    shape = ["N", "C", "H", "W"]
    in_tensor = helper.make_tensor_value_info("input", TensorProto.FLOAT, shape)
    out_tensor = helper.make_tensor_value_info("output", TensorProto.FLOAT, shape)
    node = helper.make_node("Identity", inputs=["input"], outputs=["output"], name="id")
    graph = helper.make_graph([node], name, [in_tensor], [out_tensor])
    op = helper.make_opsetid("", 17)
    model = helper.make_model(graph, opset_imports=[op], producer_name="f1-photo-stub")
    model.ir_version = 9
    return model


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out", default="./models", help="output directory")
    args = p.parse_args()
    os.makedirs(args.out, exist_ok=True)

    print(f"writing stub ONNX models into {args.out}/")
    for name in (
        "face_detect",
        "face_embed",
        "object_detect",
        "generic_embed",
        "angle_classify",
    ):
        _save(_identity_model(name), os.path.join(args.out, f"{name}.onnx"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
