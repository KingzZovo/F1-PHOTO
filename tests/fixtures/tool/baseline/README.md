# Tool / device distribution fixture (`tests/fixtures/tool/baseline/`)

A 14-class, 42-photo COCO val2017 subset used by
`packaging/scripts/distribution-baseline.sh` (via `tools/eval_distribution.py`)
to produce the **tool / device slice** of milestone #2's real-dataset
distribution baseline (see `docs/recognition_pipeline.md` §14 and roadmap
row `#2-tool` in `docs/v1.4.x-v1.5.0-roadmap.md`).

It is the natural counterpart of the **face slice** fixture
(`tests/fixtures/face/baseline/`) used by §13: same harness, same metric
shape, different input distribution. Together they characterize what the
live ONNX pipeline emits for face-dominated vs. tool/device-dominated
inputs without needing labelled identities.

## Why these 14 classes

The 14 COCO categories selected match the operator-tool / kitchen-and-
appliance vocabulary called out in roadmap row #2-tool — they are the
classes YOLOv8n COCO is most likely to fire on for real F1 operator
photos:

| Bucket | Classes |
|---|---|
| **tool-like** (handheld, kitchen / utility) | `knife`, `scissors`, `fork`, `spoon`, `bowl` |
| **device-like — compute periphery** | `laptop`, `mouse`, `keyboard`, `cell phone` |
| **device-like — appliances** | `tv`, `microwave`, `oven`, `toaster`, `refrigerator` |

3 photos per class × 14 classes = **42 photos**, matching the face slice
sample size so the two distributions are directly comparable.

## Selection algorithm

Deterministic, seed = `20260427`, implemented in
`/tmp/build_tool_fixture.py` (the seed + selection rules are also
recorded in `MANIFEST.json`).

1. Load `instances_val2017.json` from the COCO 2017 annotations zip
   (`https://images.cocodataset.org/annotations/annotations_trainval2017.zip`,
   ~253 MB; `instances_val2017.json` is ~20 MB after extraction).
2. For each target class, find all val2017 images whose largest
   annotation of that class has bbox area `≥ 96² = 9216 px` (matches the
   recognition-pipeline §3.2 detection floor) and where the target ann
   is the dominant subject of the image.
3. Pick 3 representative images per class with seed-deterministic
   shuffling over the top-30 (by target bbox area). Each chosen image
   is used at most once across the full 42-photo set.
4. "Dominant subject" is enforced via a 4-level fallback ladder so that
   small-object classes which rarely appear as the largest object in a
   val2017 frame (`knife`, `fork`, `spoon`, `mouse`, `toaster`) still
   yield 3 candidates:

    | level | dominance ratio | min bbox area |
    |---|---|---|
    | `strict`   | `target ≥ 0.5 × largest` | 9216 px |
    | `med`      | `target ≥ 0.3 × largest` | 9216 px |
    | `loose`    | no dominance gate          | 9216 px |
    | `fallback` | no dominance gate          | 4608 px (64²) |

   For this build:

    | level | classes |
    |---|---|
    | `strict`   | `scissors`, `bowl`, `laptop`, `keyboard`, `cell phone`, `tv`, `microwave`, `oven`, `refrigerator` (9 classes) |
    | `med`      | `knife`, `fork` |
    | `loose`    | `spoon`, `mouse` |
    | `fallback` | `toaster` (val2017 has very few large-area toaster images) |

   The level used per image is recorded in
   `MANIFEST.json#images[*].selection_level` so future drift checks can
   tell at a glance whether a class is sample-size-limited.

## Layout

```
tests/fixtures/tool/baseline/
├── MANIFEST.json                ← provenance + selection params + per-image manifest
├── README.md                    ← this file
├── bowl/000000099053.jpg
├── bowl/000000221872.jpg
├── bowl/000000521601.jpg
├── cell_phone/000000099428.jpg
├── …                            ← 14 class subdirs × 3 photos each = 42 jpgs
└── refrigerator/000000404191.jpg
```

Class directory names are the COCO class name with spaces replaced by
underscores (so `cell phone` → `cell_phone`). Image basenames are the
verbatim COCO val2017 file names (12-digit zero-padded image_id), so
given any file you can resolve back to the original COCO record by
extracting the integer prefix.

Total on-disk size: ~5.7 MB.

## Provenance / license

- **Image source**: COCO 2017 validation split.
  Each image was fetched individually from
  `https://images.cocodataset.org/val2017/<file_name>` (no zip download
  needed — bypasses the 1 GB val2017.zip).
- **Annotation source**: `instances_val2017.json` extracted from
  `annotations_trainval2017.zip` of the same release.
- **License**: COCO images are released under the
  Creative Commons Attribution 4.0 license
  (https://creativecommons.org/licenses/by/4.0/) per the COCO terms of
  use. Per-image `license_id` is preserved in `MANIFEST.json#images[*]`
  and the corresponding `licenses` block is copied verbatim from the
  upstream annotation file.
- **Citation** (for any downstream paper / report):
  *Lin et al., "Microsoft COCO: Common Objects in Context", ECCV 2014*.

## Regenerating the fixture

The build tool is `/tmp/build_tool_fixture.py` (seed-deterministic). It
produces the manifest plus a download script; the download script
`curl -k -L`-fetches the 42 images from `images.cocodataset.org`
(the `-k` is required because the host's TLS cert is served on a
CloudFront distribution whose SAN list does not include the bare
`images.cocodataset.org` hostname — content is unchanged either way).

```bash
# 1. Download COCO 2017 annotations once (~253 MB cached locally)
mkdir -p /root/.cache/coco-val2017 && cd /root/.cache/coco-val2017
curl -k -sS -L --retry 3 -o annotations_trainval2017.zip \
     https://images.cocodataset.org/annotations/annotations_trainval2017.zip
unzip -p annotations_trainval2017.zip annotations/instances_val2017.json \
     > instances_val2017.json

# 2. Build manifest + download script (seed-deterministic)
python3 /tmp/build_tool_fixture.py
# → /tmp/tool_fixture_manifest.json
# → /tmp/tool_fixture_download.sh

# 3. Pull the 42 images into tests/fixtures/tool/baseline/
bash /tmp/tool_fixture_download.sh

# 4. Refresh the in-tree manifest copy
cp /tmp/tool_fixture_manifest.json \
   tests/fixtures/tool/baseline/MANIFEST.json
```

## Why this is committed in-tree

The distribution baseline is meant to be reproducible without any
external-CDN flakiness. 5.7 MB of jpgs is small enough to ship in the
repo and avoids re-fetching from `images.cocodataset.org` every time
`packaging/scripts/distribution-baseline.sh` runs. The SHAs of every
image are not pinned (COCO images are static, but treating them as
drift-safe avoids a pre-flight hashing step on every run); if drift is
ever observed, regenerate via the steps above and re-pin via the
manifest's `images[*].file_name` list.

## Running the harness against this fixture

From repo root, as user `f1u` (the harness expects to write under the
bundled-PG data dir, which is owned by `f1u`):

```sh
PHOTOS_GLOB='tests/fixtures/tool/baseline/**/*.jpg' \
REPORT_PATH=/root/F1-photo/docs/baselines/2-distribution-tool-baseline.json \
sudo -u f1u bash -lc \
    'cd /root/F1-photo && bash packaging/scripts/distribution-baseline.sh'
```

The harness will boot a bundled-PG-backed server on `:18799`, upload
all 42 photos as `owner_type=wo_raw`, drain the recognition queue via
`/api/admin/queue/stats`, then aggregate per-photo + distribution
results from `detections` and `recognition_items`. The face-slice
counterpart of this run lives at
`docs/baselines/2-distribution-face-baseline.json` (see
`docs/recognition_pipeline.md` §13).
