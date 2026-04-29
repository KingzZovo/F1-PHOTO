# SCRFD-on-eastern-fixture diagnosis (2026-04-29)

> HEAD `e042e99` — zero material drift on face PR vs `#2c-tune` commit `e8c3358`.
>
> Driving question: why is `2c-tune-recognition-pr.json → per_bucket.eastern.face_detection_rate = 0.0` while western = 1.0?

## Verdict

**Eastern fixture image quality is below SCRFD-500m's reliable detection regime, and our `SCORE_THRESHOLD = 0.5` (sigmoid space) filters every candidate. `face_count = 0` → `tp = 0` → `f1 = None`. This is a fixture/model-capacity problem, not a threshold-tuning problem on the recall side.**

## Evidence chain

### 1. eval_pr.py PR computation is single-path (no algorithm bug)

- `compute_pr` in `tools/eval_pr.py:360-389` is the only PR-computing function.
- `bucket()` at line 175 is a small score-band classifier called *inside* `compute_pr`; it is not a separate codepath.
- `per_bucket` / `overall_at_default` / `sweep` all walk through `compute_pr`.
- western.tp (8) + eastern.tp (0) ≡ overall.tp (8) ✓
- sweep@(ll=0.30, ml=0.40) ≡ overall_at_default ✓
- Initial "double-path divergence" hypothesis was a false-positive caused by cross-comparing stale `2c-recognition-pr.json` (DEFAULT 0.50/0.62) against current `2c-tune-recognition-pr.json` (DEFAULT 0.30/0.40). The stale file is now renamed `2c-recognition-pr.stale-pre-2c-tune.json` and flagged with `_stale: true`.

### 2. server `Thresholds::DEFAULT` matches `2c-tune-recognition-pr.json`

```rust
// server/src/inference/recall.rs:52-55
pub const DEFAULT: Self = Self {
    low_lower: 0.30,
    match_lower: 0.40,
    augment_upper: 0.95,
};
```

### 3. Eastern fixture is bicubic-upscaled twice

- Source: `jack139/face-dataset/train2 (Asian A subset, 50 IDs)`, native ~140×147 RGB dlib face crops.
- `tests/fixtures/face/baseline/MANIFEST.json` records `params.upscale_to = [256, 256]` and `params.upscale_resampler = "PIL.Image.BICUBIC"`.
- All 8 verified eastern fixture seed/query files are exactly 256×256 RGB JPEG.
- Server pipeline: `server/src/inference/preprocess.rs:79 letterbox()` re-resizes 256×256 → fits inside 640×640 with `image::FilterType::Triangle` and gray padding (114,114,114).
- Net: original ~140 px face is **bicubic-upscaled to 256, then triangle-resampled into a ~640×640 letterbox**, with the actual face occupying about 256/640 ≈ 40% of one axis (≈256×256 of 640×640, the rest gray padding). High-frequency facial features have been double-interpolated.

### 4. Western fixture is single-resampled

- Source: LFW funneled, native 250×250.
- All 2 verified western seed files are 250×250.
- Server letterbox triangle-resamples 250 → ~640. Single resample, native quality.
- `western.face_detection_rate = 1.0` (n=20, all 20 query photos produced ≥1 candidate clearing 0.5 sigmoid).

### 5. SCRFD config

```rust
// server/src/inference/scrfd.rs:33,41-44
pub const INPUT_SIZE: u32 = 640;
/// Score threshold (sigmoid space). Reference SCRFD inference uses 0.5; we
/// keep it the same so behaviour matches the upstream Python pipeline.
pub const SCORE_THRESHOLD: f32 = 0.5;
```

SCRFD-500m is a small detector trained for ≥80 px native faces; double-interpolated low-quality 140-px crops upscaled to 256 are a known weak spot, frequently producing all-candidates-below-0.5 and thus zero post-threshold output.

## Decision matrix (King to choose)

| Option | What it changes | Effort | Risk | Effect on `eastern.f1` |
|--------|-----------------|--------|------|------------------------|
| **A. Replace eastern fixture with higher-res Asian face dataset** | Swap `jack139/face-dataset/train2` for `glintasia` or `train4 (Asian A 412 IDs, larger native)`; rebuild MANIFEST | medium (~1 hr build_face_fixture script run + manifest regen + commit) | low (test fixture only, no server change) | **High likelihood to fix**. Native ≥200 px feeds SCRFD-500m within design envelope. |
| **B. Lower SCRFD `SCORE_THRESHOLD` for eastern bucket** | Bucket-aware threshold in `scrfd.rs`; or a soft floor like 0.3 | high (server Rust change, requires King approval per hard rule) | medium (hurts precision on western too if not bucketed; loses upstream-Python parity) | Partial — may bring eastern off 0.0 but with weaker bbox quality. |
| **C. Upgrade SCRFD-500m → SCRFD-2.5g** | New ONNX model, larger backbone | high (model swap, retest, possible ORT version pin issues) | medium (slower inference, model file bandwidth) | High likelihood, fully "correct" fix. |
| **D. Drop eastern from #2c gauge** | Mark `2c-tune-recognition-pr.json eastern` block as fixture-limited, not model-limited; gate face PR only on western | low | low | Reframes the problem; eastern stays at 0.0 in metrics but doesn't block roadmap. |

**Recommended (no-server-change-needed path):** **A**. Test fixture is the easiest lever and the cause is fixture-side. After A, if eastern is *still* 0.0, we have a real model-capacity problem and revisit C.

## Why we did not rerun face PR baseline today (Step B)

`git log` on `server/src/inference/{recall,scrfd,preprocess,models,yolov8,mod}.rs`, `server/src/worker/mod.rs`, `models/`, and `tools/eval_pr.py` since `#2c-tune` (e8c3358):

- `f16da9b feat(inference): make YOLOv8 decoder shape-tolerant` — YOLOv8 only, no face path effect.
- `23126a4 feat(worker): collapse detector class to 'tool' + model_versions` — metadata + tool detector class, no face inference change.
- `e042e99 tools(eval_pr): allow F1C_PROJECT_CODE/F1C_WO_CODE env overrides` — today, plumbing only, no algorithm change.

→ Rerunning face PR baseline against HEAD would reproduce `2c-tune-recognition-pr.json` exactly. It would also pollute the gallery: 15 enrolled persons from the existing `P-2C-PR` project are still in `identity_embeddings` and would dominate HNSW queries against any parallel fresh-employee_no enrollment, making any rerun's metrics meaningless. **Decision: skip B; treat `2c-tune-recognition-pr.json` as today's truth.**
