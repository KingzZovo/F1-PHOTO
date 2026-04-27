# Tool fixture

A small, license-clean, owner-known **tool** photo for the smoke e2e to
exercise the milestone #2a-tool bootstrap path (YOLOv8 region proposer +
DINOv2-small per-crop embed → `identity_embeddings('initial')` seeded for
an asserted `owner_type='tool'` / `owner_type='device'`).

## File

- `tool_001.jpg` — 600×400 RGB, JPEG q=90, **71,303 B**,
  `sha256=db702e664b962a9daec5ef9395a7ffe7290aa9ef91471a3886717fc1cceaf325`.
- `MANIFEST.json` — provenance metadata consumed by the smoke + future
  drift checks.

## Provenance

- Source: `skimage.data.coffee()` — a public-domain photograph of a coffee
  cup, bundled with **scikit-image** (BSD-3-Clause project, but the image
  asset itself is PD).
- Why this image specifically: YOLOv8n is COCO-trained and the cup
  occupies the dominant region of the frame at high confidence on COCO
  class 41 (`cup`). That gives the bootstrap path at least one box to
  embed and persist. Cross-checked against the lesson learned at #2b
  (StyleGAN faces failed SCRFD detection due to a domain gap) — using a
  real-photo PD asset avoids the synthetic-image domain gap entirely.
- Depicts a real product in a real setting; no person, no logo of
  consequence, no trade-secret data.

## License

Public domain. The skimage maintainers ship `coffee.png` as part of
`skimage.data` without any per-file copyright notice. We re-encode to JPEG
q=90 only to keep the fixture small; the underlying pixels are unmodified.

## Why we keep it in-tree

The smoke e2e runs offline (no internet during `bash packaging/scripts/
smoke-e2e.sh`), so any external CDN fetch would be a flakiness vector. The
fixture is small enough (~70 KB) to ship in the repo without bloating
clones.

## Regenerate

If you ever need to regenerate this fixture (e.g. after switching skimage
versions and confirming the asset hash), run:

```bash
python3 -c "\
from skimage.data import coffee; \
from PIL import Image; \
Image.fromarray(coffee(), 'RGB').save( \
    'tests/fixtures/tool/tool_001.jpg', \
    'JPEG', quality=90, optimize=True, progressive=False, \
)"
```

Then update `MANIFEST.json` with the new size + sha256 (see
`/tmp/gen_tool_fixture.py` for the helper script used at #2a-tool).

## Notes for future maintainers

- **Domain gap warning** (carried over from #2b): if you ever swap this
  fixture for a synthetic asset (e.g. a procedurally rendered tool), be
  prepared for YOLOv8 to return `object_count=0`. The bootstrap path
  short-circuits with a warn log on zero detections, so the smoke would
  still pass minus the seed assertions — but the fixture would no longer
  be exercising what it claims to exercise. Prefer real-photo PD assets.
- The image content is a coffee cup, not a tool; the smoke uses it
  *symbolically* as a tool ownership shot. The bootstrap pipeline does
  not look at COCO class labels — it uses YOLOv8 strictly as a region
  proposer and reidentifies via DINOv2 over the per-crop embedding.
  Class label is preserved in `detections.bbox.class_id` for audit only.
