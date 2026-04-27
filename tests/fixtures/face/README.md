# Face fixture (`tests/fixtures/face/`)

This directory contains a small, license-clean face image used by
`packaging/scripts/smoke-e2e.sh` to drive the **owner-known person
bootstrap → matched-bucket recall** path end-to-end (milestone #2b
in `docs/v1.4.x-v1.5.0-roadmap.md`).

The fixture is **committed into git**. It is small (≈65 KB), has no
privacy / likeness concerns (NASA public-domain portrait — see
provenance below), and lets the smoke run with zero external
dependencies.

## Files

| File | What it is |
|---|---|
| `portrait_001.jpg` | 512×512 RGB JPEG of NASA astronaut Eileen Collins. Used both as the gallery seed (`owner_type=person`) and, in a second project, as the `wo_raw` upload that should match back to the seed. |
| `MANIFEST.json` | Provenance + sha256 + license metadata. The smoke script is permitted to assume the file at the recorded sha256; if you regenerate, update the manifest. |
| `README.md` | This file. |

## Provenance

* Source: `skimage.data.astronaut()` from
  [scikit-image](https://scikit-image.org/) — bundled NASA portrait of
  astronaut Eileen Collins, the canonical face-detection demo image
  used in OpenCV / scikit-image tutorials worldwide.
* Fetched: 2026-04-27 (regenerated from the bundled scikit-image
  asset; no network fetch involved).
* Original image: 512×512 RGB uint8 numpy array.
* Post-processing: PIL `Image.fromarray(arr, 'RGB').save(...)` as
  JPEG quality=90 (`optimize=True`, non-progressive). No resize.

### Why we switched away from a StyleGAN synthetic face

An earlier revision of this fixture used a 1024×1024 image from
`thispersondoesnotexist.com` (StyleGAN2). It looked like a clean
portrait to a human eye but SCRFD `det_500m` returned `face_count=0`
on it during the smoke run. This is a known **GAN domain gap**: face
detectors trained on natural photos (WIDERFACE etc.) sometimes fail or
score below threshold on synthetic / GAN-generated faces because the
micro-texture distribution differs from real camera output. The same
property is what makes face-detection-based deepfake detectors
possible. Replacing the fixture with a real (public-domain) photograph
removed the problem and the smoke turned green on the first run.

Lesson: when a smoke needs `face_count >= 1`, use a *real* photograph
that is licence-clean, not a generative-model output.

## License / why this is safe to commit

The NASA astronaut portrait is a work of an officer / employee of the
U.S. Federal government created in the course of official duties, and
is therefore in the public domain under 17 U.S.C. § 105. scikit-image
bundles it under the same public-domain assertion (see
`skimage/data/_registry.py`). No likeness rights need to be cleared.

If you prefer to swap in a different licence-clean face (e.g. a CC0
photo from Wikimedia Commons), regenerate per the steps below. The
smoke script does not care about the *identity* of the face — only
that SCRFD can detect one face in it with ≥ 1 detection above the
configured score threshold.

## Regenerating the fixture

Any SCRFD-detectable real face will do. Reference recipe (uses the
bundled scikit-image asset, no network needed):

```bash
pip install -q scikit-image pillow

python3 - <<'PY'
from skimage.data import astronaut
from PIL import Image
import hashlib, os
arr = astronaut()  # 512x512 RGB uint8 — public-domain NASA portrait
Im = Image.fromarray(arr, mode="RGB")
out = "tests/fixtures/face/portrait_001.jpg"
Im.save(out, "JPEG", quality=90, optimize=True, progressive=False)
h = hashlib.sha256(open(out, "rb").read()).hexdigest()
print("sha256", h, "size", os.path.getsize(out))
PY

# Update MANIFEST.json sha256 + size_bytes to match the printed values.
```

After regenerating, re-run `packaging/scripts/smoke-e2e.sh` to confirm
the new fixture still seeds the bootstrap path (i.e. SCRFD finds a
face) and the wo_raw upload still matches it back.

## Why a single fixture is enough

Milestone #2a wired the bootstrap path: `worker::run_real_pipeline`
branches on `photos.owner_type='person'` to seed the gallery with one
`identity_embeddings` row per detected face (`source='initial'`). The
smoke proves both halves of that with a single image:

1. Upload `portrait_001.jpg` into project A as `owner_type=person,
   owner_id=<P>` → bootstrap path → 1 `identity_embeddings('initial')`
   row anchored to person P, no `recognition_items` rows.
2. Upload **the same** `portrait_001.jpg` into project B as
   `owner_type=wo_raw` → standard pipeline → SCRFD detects the same
   face → ArcFace embed → recall hits the gallery row from step 1 →
   `recognition_items` matched bucket with `matched_owner_id=P`.

The `(project_id, hash)` UNIQUE constraint forces step 2 to live in a
different project; the recall layer is workspace-global so the seed
from project A is visible from project B (this is why `recall.rs`
filters only on `owner_type`, never on `project_id`).

A larger, multi-identity dataset for measuring P/R baselines is
tracked separately as **milestone #2c** (real-dataset eval) in the
roadmap.
