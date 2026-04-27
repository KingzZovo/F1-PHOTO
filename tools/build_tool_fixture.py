#!/usr/bin/env python3
"""Build a 14-class COCO val2017 tool/device fixture for milestone #2-tool.

For each of 14 operator-relevant classes, pick 3 representative images:
  - image must contain at least one annotation of the target class
    with bbox area >= 96*96 (matches recognition_pipeline.md §3.2 floor)
  - target ann must be at least DOMINANCE_RATIO of the largest ann's area
    (so the target is visually prominent, not a tiny aside)
  - de-duplicate: each image appears at most once across the 42-photo set

Write a manifest, and emit a shell script that downloads each image
individually from https://images.cocodataset.org/val2017/<file_name>.
"""
from __future__ import annotations

import json
import random
import sys
from pathlib import Path

ANN = Path("/root/.cache/coco-val2017/instances_val2017.json")
OUT_MANIFEST = Path("/tmp/tool_fixture_manifest.json")
OUT_DOWNLOAD_SH = Path("/tmp/tool_fixture_download.sh")

CLASSES = [
    "knife", "scissors", "fork", "spoon", "bowl",
    "laptop", "mouse", "keyboard", "cell phone",
    "tv", "microwave", "oven", "toaster", "refrigerator",
]
PER_CLASS = 3
MIN_AREA = 96 * 96  # 9216 px
DOMINANCE_RATIO = 0.5  # target_area >= 0.5 * largest_ann.area
SEED = 20260427
IMG_URL_PREFIX = "https://images.cocodataset.org/val2017/"


def main() -> int:
    print("▶ loading", ANN)
    data = json.loads(ANN.read_text())
    print("✓", len(data["images"]), "images,",
          len(data["annotations"]), "anns,",
          len(data["categories"]), "cats")

    by_name = {c["name"]: c["id"] for c in data["categories"]}
    cat_ids = {}
    for cls in CLASSES:
        if cls not in by_name:
            print("✗ missing class:", cls, file=sys.stderr)
            return 2
        cat_ids[cls] = by_name[cls]

    images_by_id = {im["id"]: im for im in data["images"]}
    licenses = {l["id"]: l for l in data["licenses"]}

    anns_by_img = {}
    for a in data["annotations"]:
        anns_by_img.setdefault(a["image_id"], []).append(a)

    rng = random.Random(SEED)
    used_image_ids = set()
    chosen = []
    # per-class diagnostic counts at multiple filter levels
    for cls in CLASSES:
        cid = cat_ids[cls]
        # try increasingly loose filter levels until >= PER_CLASS candidates
        # level 0: dominance>=0.5, area>=MIN_AREA
        # level 1: dominance>=0.3, area>=MIN_AREA
        # level 2: no dominance,    area>=MIN_AREA
        # level 3: no dominance,    area>=MIN_AREA/2 (>= 64x64 = 4608 px)
        levels = [
            ("strict",  DOMINANCE_RATIO, MIN_AREA),
            ("med",     0.3,             MIN_AREA),
            ("loose",   0.0,             MIN_AREA),
            ("fallback",0.0,             MIN_AREA // 2),
        ]
        candidates = []
        used_level = None
        for lvl_name, dom_ratio, min_area in levels:
            candidates = []
            for img_id, anns in anns_by_img.items():
                if img_id in used_image_ids:
                    continue
                target_anns = [a for a in anns if a["category_id"] == cid]
                if not target_anns:
                    continue
                best_target = max(target_anns, key=lambda a: a["area"])
                if best_target["area"] < min_area:
                    continue
                if dom_ratio > 0:
                    largest = max(anns, key=lambda a: a["area"])
                    if largest["area"] > 0 and best_target["area"] < dom_ratio * largest["area"]:
                        continue
                im = images_by_id[img_id]
                candidates.append({
                    "image_id": img_id,
                    "target_area": best_target["area"],
                    "width": im["width"],
                    "height": im["height"],
                    "file_name": im["file_name"],
                    "license_id": im.get("license"),
                })
            if len(candidates) >= PER_CLASS:
                used_level = lvl_name
                break
        if len(candidates) < PER_CLASS:
            print("✗ not enough candidates for", cls, ":",
                  len(candidates), "found at any level", file=sys.stderr)
            return 3
        candidates.sort(key=lambda c: c["target_area"], reverse=True)
        head = candidates[: max(30, PER_CLASS)]
        rng.shuffle(head)
        picked = head[:PER_CLASS]
        for p in picked:
            used_image_ids.add(p["image_id"])
            p["class"] = cls
            p["category_id"] = cid
            p["selection_level"] = used_level
            chosen.append(p)
        print("  ", cls.ljust(14),
              "level=", used_level,
              "candidates=", len(candidates),
              "picked=", [p["file_name"] for p in picked])

    print("✓ chose", len(chosen), "images across", len(CLASSES), "classes")

    manifest = {
        "source": "COCO val2017 instances annotations",
        "annotations_url":
            "https://images.cocodataset.org/annotations/annotations_trainval2017.zip",
        "image_url_prefix": IMG_URL_PREFIX,
        "selection": {
            "classes": CLASSES,
            "per_class": PER_CLASS,
            "min_target_bbox_area_px": MIN_AREA,
            "dominance_ratio": DOMINANCE_RATIO,
            "dominance": "target_area >= dominance_ratio * largest_ann_area",
            "seed": SEED,
        },
        "licenses": licenses,
        "images": chosen,
    }
    OUT_MANIFEST.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    print("✓ wrote", OUT_MANIFEST, OUT_MANIFEST.stat().st_size, "B")

    lines = []
    lines.append("#!/usr/bin/env bash")
    lines.append("# Auto-generated by /tmp/build_tool_fixture.py")
    lines.append("set -euo pipefail")
    lines.append('DEST="${DEST:-/root/F1-photo/tests/fixtures/tool/baseline}"')
    lines.append('mkdir -p "$DEST"')
    lines.append('cd "$DEST"')
    for p in chosen:
        cls_dir = p["class"].replace(" ", "_")
        fname = p["file_name"]
        url = IMG_URL_PREFIX + fname
        lines.append("mkdir -p " + cls_dir)
        lines.append(
            "[ -s '" + cls_dir + "/" + fname + "' ] || "
            "curl -k -sS -L --retry 3 --max-time 60 -o '"
            + cls_dir + "/" + fname + "' '" + url + "'"
        )
    OUT_DOWNLOAD_SH.write_text("\n".join(lines) + "\n")
    OUT_DOWNLOAD_SH.chmod(0o755)
    print("✓ wrote", OUT_DOWNLOAD_SH)
    return 0


if __name__ == "__main__":
    sys.exit(main())
