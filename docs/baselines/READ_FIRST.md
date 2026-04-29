# Baselines 读你之前先看这

> 最后刷新：2026-04-29 PM。
>
> `docs/baselines/` 里有不同阶段的 evaluation 产物，部分**同名不同阈值**，直接读件名会混乱。以下是当前真值表。

## 当前真值（v1.5.0 / post-#2c-tune）

| 主题 | 当前真值文件 |
|------|---------------|
| Face P/R（SCRFD + ArcFace + recall，阈值 (low_lower=0.30, match_lower=0.40)） | `2c-tune-recognition-pr.json` |
| Face 识别分布 baseline | `2-distribution-face-baseline.json` + `2-distribution-face-rerun-2026-04-29.json`（后者为当日重跑快照） |
| Tool 识别分布 baseline | `2-distribution-tool-baseline.json` + `2-distribution-tool-rerun-2026-04-29.json` |
| Asian 在野 face 分布 | `2c-asia.json` |
| ID 照入库 (Path A) | `2a-real-id-photos.json` |
| wo_raw 反向冒烟 | `2a-real-reverse-smoke.json` |
| #5-bootstrap 采集快照 | `5-bootstrap-collector-smoke.json` |
| #5-bootstrap importer dry-run | `5-bootstrap-importer-smoke.json` |
| #7c-eval-self-wiring | `7c-eval-self-wiring-2026-04-29.json` |
| Gallery 质量快照（2026-04-29 PM） | `gallery-quality-audit-2026-04-29.json` |
| SCRFD eastern fixture 诊断（2026-04-29 PM） | `scrfd-eastern-fixture-diagnosis-2026-04-29.md` |

## Stale / 已被取代（不要参考！）

| Stale 文件 | 原因 | 被谁取代 |
|----------|------|----------|
| `2c-recognition-pr.stale-pre-2c-tune.json` | Pre-#2c-tune 阈值 (low_lower=0.5, match_lower=0.62)；server `Thresholds::DEFAULT` 已重调为 (0.30, 0.40) | `2c-tune-recognition-pr.json` |

该文件顶层已加 `"_stale": true` + `"superseded_by": "2c-tune-recognition-pr.json"` 标记。

## 阈值跳起记录

- **#2c 阶段**：`Thresholds::DEFAULT { low_lower: 0.50, match_lower: 0.62, augment_upper: 0.95 }`。F1 在 30 张 fixture 上只有 0.154。
- **#2c-tune 重调 (HEAD `e042e99` 仍生效)**：`Thresholds::DEFAULT { low_lower: 0.30, match_lower: 0.40, augment_upper: 0.95 }`。F1 提到 0.500（overall）。
- 调整在 `server/src/inference/recall.rs:52-55`。所有评估脚本 (`tools/eval_pr.py`) 的 `DEFAULT` 变量必须与 server 保持一致，不然会产生 stale baseline。

## 参考表：2c-tune-recognition-pr.json 自洽验证

| 口径 | tp | fp | fn | recall | F1 | n |
|------|----|-----|-----|--------|-----|---|
| `overall_at_default` | 8 | 0 | 16 | 0.333 | 0.500 | 30 |
| `per_bucket.western` | 8 | 0 | 8 | 0.500 | 0.667 | 20 |
| `per_bucket.eastern` | 0 | 0 | 8 | 0.000 | None | 10 |
| `sweep[ml=0.40, ll=0.30]` | 8 | 0 | 16 | 0.333 | 0.500 | 30 |

western.tp + eastern.tp ≡ overall.tp ✓、overall ≡ sweep@default ✓、算法统一。

## eastern face_detection_rate = 0.0 诊断线索

- `MANIFEST.json:30-37` 明文记：eastern fixture native size **~140×147**（jack139 dlib crops），预先 PIL bicubic upscale 到 **256×256**。
- server `letterbox(src, 640)` 再一次 Triangle resize 到 **640×640**。
- 实际人脸像素 ≈ 256/640 × ~140×147 ÷ 256 × 640 ≈ **140–150 px in 640² input**、但是双重插值后高频信息丢得差不多，看似以 ~70-90 px 质量。
- SCRFD-500m `SCORE_THRESHOLD = 0.5` (sigmoid)，SCRFD 对低质人脸输出低 score，可能整个候选集都低于 0.5，被阈值滤掉 → face_count=0 → face_detection_rate=0。
- western LFW funneled native **250×250**，质量高一别，face_detection_rate=1.0。

## 2026-04-29 PM 补充

- `gallery-quality-audit-2026-04-29.json` — 当日 `identity_embeddings` + `persons` 口径快照（只读，无副作用）。**127 persons / 248 embeds**；4 cold-start、6 only-initial、119 complete（93.7%）。
- `scrfd-eastern-fixture-diagnosis-2026-04-29.md` — eastern `face_detection_rate=0.0` 根因定调。**结论：fixture 双重插值 + SCRFD-500m 容量不足**，不是 recall 阈值问题。King 需从 A/B/C/D 四个修复选项中选择（详见该 MD）。
- **今日未重跑 face PR baseline**（未产生 `2c-tune-recognition-pr-2026-04-29.json`）。原因：自 `#2c-tune` (`e8c3358`) 后 server inference 路径零漂移，今日 HEAD 在之后只多了 3 个不影响 face PR 的 commit：`f16da9b` (YOLOv8 decoder shape)、`23126a4` (worker tool class collapse + model_versions)、`e042e99` (eval_pr.py env override)。`2c-tune-recognition-pr.json` 仍是当前真值。如果未来动了 `server/src/inference/{recall,scrfd,preprocess}.rs` 或 `models/`，重跑才有意义。

## 2026-04-29 PM-B：SCRFD-10g 替换实验（**否定结论**）

落地于 `2c-tune-recognition-pr-scrfd-10g-2026-04-29.json`，与 `2c-tune-recognition-pr.json`（500m，当前真值）对照：

| 口径 | 模型 | tp | fp | fn | recall | F1 | face_det_rate |
|------|------|-----|-----|-----|--------|------|---------------|
| overall | SCRFD-500m | 8 | 0 | 16 | 0.333 | 0.500 | 0.667 |
| overall | SCRFD-10g | 9 | 0 | 15 | 0.375 | **0.545** | 0.667 |
| western | SCRFD-500m | 8 | 0 | 8 | 0.500 | 0.667 | 1.0 |
| western | SCRFD-10g | 9 | 0 | 7 | 0.5625 | **0.720** | 1.0 |
| eastern | SCRFD-500m | 0 | 0 | 8 | 0.0 | None | **0.0** |
| eastern | SCRFD-10g | 0 | 0 | 8 | 0.0 | None | **0.0** |

**结论：模型容量从 ~2.5MB → 16.9MB（6.7×）后**
- western 微涨（F1 0.667 → 0.720，+8%），但已经不是瓶颈。
- eastern `face_detection_rate` 仍然 **0.0**，**模型容量上 6.7× 也救不了**。
- 印证 `scrfd-eastern-fixture-diagnosis-2026-04-29.md` 的根因诊断：**fixture 双重插值是元凶**，模型再大也无法在 bicubic-blurred 的 ~140×147→256→640 输入上恢复人脸高频特征。

**生产决策：保留 500m。** 不为 western 微涨付出 6.7× 的内存与延迟代价。`models/face_detect.onnx` 已恢复为 500m（sha `5e4447f5...`）。10g 副本保留在 `models/history/` (gitignored) 以备将来对比。

**B 实验流程**（可重现）：
1. 备份 500m → `models/history/face_detect.v0.det_500m.onnx`
2. 下载 10g `https://huggingface.co/deepghs/insightface/resolve/main/buffalo_l/det_10g.onnx`（sha256 `5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91`）
3. ONNX 形状自检：IR v6 opset 11，input `[1,3,?,?]`，9 outputs `12800/3200/800 × {1,4,10}`，与 500m 完全同构；Rust SCRFD 解码无需改
4. atomic swap → 重启 server → 跑 `tools/eval_pr.py`
5. 还原 500m → 重启 → 健康检查

**SCRFD-2.5g 没测**：deepghs/insightface 上游只发 buffalo_s (500m) 与 buffalo_l (10g)，buffalo_m (2.5g) 缺失；其它公开镜像皆 gated 或 404。鉴于 10g 已证伪 "扩容能解 eastern" 假设，2.5g 不再有探索价值。

**下一步路径（A）**：重做 eastern fixture 去除第一次 bicubic-upscale。从 jack139/face-dataset/train2 重新拉 native ~140×147 crops，存盘时不做 PIL upscale，让 server SCRFD 内部 letterbox 单次插值即可。预期能把 eastern `face_detection_rate` 从 0 拉起。


---

## PM-A & PM-A.2 — Eastern fixture rebuild from native jack139 crops (2026-04-29)

**Status:** PM-A (no upscale, no pad) → still face_det_rate=0.0. PM-A.2 (no upscale + 320×320 gray-pad) → **face_det_rate=1.0**, eastern F1 None→0.2.

### Summary table (eastern bucket, 500m, default thresholds ll=0.30 ml=0.40)

| Run | seed file size | face_det_rate | TP | FP | FN | TN | F1 | Δ overall F1 |
|---|---|---|---|---|---|---|---|---|
| Baseline (`2c-tune-recognition-pr.json`) | 256×256 PIL.BICUBIC of jack139 dlib | 0.0 | 0 | 0 | 8 | 2 | None | — |
| PM-B (10g model, same fixture) | 256×256 PIL.BICUBIC | 0.0 | 0 | 0 | 8 | 2 | None | +0.046 (western only) |
| **PM-A** (drop bicubic, native ~140×147) | native jack139 dlib | **0.0** ❌ | 0 | 0 | 8 | 2 | None | 0 |
| **PM-A.2** (native + 320×320 gray-pad) | 320×320 padded | **1.0** ✅ | 1 | 1 | 7 | 2 | 0.2 | **+0.029** |

### Root cause (confirmed)

The original failure was **NOT the double interpolation** as PM-B's READ_FIRST hypothesized. It was the **dlib-cropping of jack139's `test2/`**: dlib aggressively trims to face-only (face occupies ~95% of frame, no hair/neck/shoulder/background). SCRFD-500m and SCRFD-10g both expect *face-with-context* (LFW-funneled style: face ~30-50% of frame). Without context, SCRFD's classification head outputs all-zero probabilities → 0 detections → eastern bucket is a black hole.

**Evidence:**
- PM-A wrote 14 native jack139 jpgs (~131-159×125-154 RGB) directly. Server letterbox to 640×640, no PIL interp, no 256×256 hop. face_det_rate stayed at 0.0.
- PM-A.2 took the same 14 native jpgs, centered them on a 320×320 (128,128,128) gray canvas (face occupies ~45% of frame). face_det_rate jumped to 1.0 (12 detections from 12 photos).
- Western fixtures (LFW-funneled 250×250) always had face_det_rate=1.0 — they already have the LFW-style context.

### Why PM-B's bicubic-double-interp hypothesis was wrong

The 256×256 bicubic upscale of a dlib-tight crop **also** had no context. Both versions failed for the same root reason. The 256×256 vs native size delta was a red herring; what matters is **face/context ratio in the source frame**.

### Procedure (PM-A.2, current production fixture)

Script: `/tmp/rebuild-eastern-padded.py` (kept under /tmp; not committed — rerun-as-needed via the manifest's `params.eastern_pad_to`/`pad_fill`/`upstream_path`).

1. For each enrolled eastern slug `{aidai, baijingting, baobeier, caihancen}`:
   - GET `https://api.github.com/repos/jack139/face-dataset/contents/test2/<slug>`.
   - Sort filenames lexicographically; take first 3 (`seed_01`, `query_01`, `query_02`).
   - Download raw jpg bytes from `https://raw.githubusercontent.com/jack139/face-dataset/master/test2/<slug>/<name>`.
2. For distractor `caiyilin`: same GET, take first 2 (`query_01`, `query_02`).
3. Open each with PIL, paste centered onto 320×320 gray (128,128,128), JPEG q=92.
4. Atomic write to `tests/fixtures/face/baseline/eastern_<slug>/{seed_01,query_01,query_02}.jpg` (+ `_distractor_eastern_caiyilin/{query_01,query_02}.jpg`).
5. Recompute sha256 + bytes; patch each affected `MANIFEST.json` `files[]` entry.
6. Set `params.upscale_to=null`, `params.upscale_resampler=null`, `params.eastern_pad_to=[320,320]`, `params.eastern_pad_fill=[128,128,128]`, `params.eastern_padded_at="2026-04-29 PM-A.2"`.
7. Update `sources.eastern.note` with the rebuild rationale.

### Detections (PM-A.2 raw counts)

- Seed (12 photos): 12 detections — all 12 enrolled (8 W + 4 E) seeds detected on first attempt. (Up from 8 in PM-A: the 4 eastern seeds now detect.)
- Query (30 photos = 24 enrolled queries + 6 distractors): see SUMMARY in `2c-tune-recognition-pr-fix-A2-2026-04-29.json`.
- Eastern queries (8 enrolled): TP=1, FN=7 — SCRFD now *detects* the face but ArcFace embedding distance from seed exceeds match_lower=0.40 in 7 of 8 cases. Most slip into `learning` bucket (cos_sim in [0.30, 0.40)).
- Eastern distractors (2 caiyilin queries against seeded persons): TP=0 FP=1 — one false-positive eastern match (a caiyilin query tagged as one of {aidai,baijingting,baobeier,caihancen}).

### Threshold sweep (eastern fix path forward)

Default `ml=0.40` is too strict for jack139 + LFW seed embedding distance. At `ml=0.30`:
- Overall: P=0.700 R=0.583 F1=**0.636** (up from 0.529 at ml=0.40).
- TP=14 FP=6 FN=10 TN=5 (eastern contributes ~5 of those 14 TPs).
- Cost: more eastern false-positives (FP grew 1→6 across the sweep).

Decision pending: keep server's `Thresholds::DEFAULT.match_lower=0.40` (current) vs lower to 0.30/0.35 (better R, more FP) vs introduce per-bucket thresholds.

### Truth-baseline supersession

- `docs/baselines/2c-tune-recognition-pr.json` (the legacy "truth" — eastern.F1=None) is now **historical-reference only**. The fixture it referenced no longer exists on disk; HEAD's fixture is the PM-A.2 padded version.
- New de-facto truth: `docs/baselines/2c-tune-recognition-pr-fix-A2-2026-04-29.json` (overall F1=0.529, eastern.F1=0.200, eastern face_det_rate=1.0, western F1=0.667 unchanged).
- Future regressions are measured against PM-A.2.

### What did not change

- Server: SCRFD-500m (`5e4447f5...` 2.4 MiB), `Thresholds::DEFAULT { low_lower:0.30, match_lower:0.40, augment_upper:0.95 }`, no Rust changes.
- Western fixture: LFW-funneled 250×250, untouched.
- Western metrics: F1=0.667, face_det_rate=1.0 — both unchanged across PM-B / PM-A / PM-A.2.
- Distractor counts (3 total: caiyilin eastern, marcelo_rios western, thomas_rupprath western).

### Followups (not done in this run)

- Decide on threshold tuning: per-bucket `match_lower=0.30` for eastern only? Or train an Asian-fine-tuned ArcFace head? Current eastern recall (0.125 at default) is the next bottleneck.
- Consider richer eastern source: jack139's `test4/` (112×112) or RMFD/glintasia — anything pre-funneled rather than dlib-tight.
- Audit why one caiyilin distractor query crossed match_lower=0.40 against an enrolled eastern person (single eastern FP at default threshold).
- The 3 fewer photo_unmatched between PM-A and PM-A.2 (87→83 seed-drain) and 9 more photo_matched (250 vs 246) confirm SCRFD now sees eastern seeds.


---

## PM-A.3 / Path C — Eastern fixture switched to jack139/test3 (2026-04-29)

**Status:** Fixture source switched from `test2` (dlib-tight ~140×147 + 320 gray-pad) to `test3` (native 250×250 funneled-equivalent). Identity slugs are now anonymous numeric IDs (`t3_3131124`, `t3_3131306`, `t3_3132111`, `t3_3134589` enrolled; `t3_3135437` distractor). The 320×320 gray-pad hack is removed entirely; PM-A.2's padded test2 fixture is fully superseded.

### Result table (eastern bucket, 500m, default thresholds ll=0.30 ml=0.40)

| Run | seed source | face_det_rate | TP | FP | FN | TN | F1 | overall F1 |
|---|---|---|---|---|---|---|---|---|
| Baseline (`2c-tune-recognition-pr.json`) | jack139 dlib + 256 bicubic | 0.0 | 0 | 0 | 8 | 2 | None | 0.500 |
| PM-B (10g) | same fixture | 0.0 | 0 | 0 | 8 | 2 | None | 0.546 |
| PM-A (no upscale) | jack139 dlib native | 0.0 | 0 | 0 | 8 | 2 | None | 0.500 |
| PM-A.2 (320 pad) | dlib + 320 gray-pad | 1.0 | 1 | 1 | 7 | 2 | 0.200 | 0.529 |
| **PM-A.3 (test3 native)** | **test3 250×250 funneled-equiv** | **1.0** | 0 | 0 | 8 | 2 | None | 0.500 |

### But the threshold sweep tells a very different story

At match_lower=0.30 (one tick below default):

| Run | overall F1 | overall P | overall R | TP | FP | FN | TN |
|---|---|---|---|---|---|---|---|
| PM-A.2 (padded test2) | 0.636 | 0.700 | 0.583 | 14 | **6** | 10 | 5 |
| **PM-A.3 (native test3)** | **0.700** | **0.875** | 0.583 | 14 | **2** | 10 | 5 |

Same TP (14), same FN (10), same R (0.583) — but **A.3 has 1/3 the false positives** of A.2. Inferred eastern at ml=0.30:
- A.2: eastern TP=6 FP≈5 → eastern F1 ≈0.63
- **A.3: eastern TP=6 FP≈1 → eastern F1 ≈0.80**

The two fixtures detect the same number of true matches at a lowered threshold, but test3's funneled-style images produce ArcFace embeddings that are dramatically cleaner across identities. The padded-dlib hack of A.2 manufactured fake "context" that ArcFace partially encoded as identity noise, raising cross-person similarity — hence the FP=6.

### Why A.3 still scores None at default ml=0.40

ArcFace (face_embed.onnx) was trained on Western/global photo distributions. Test3 (anonymous Asian celeb subset) embeddings are at greater absolute cosine distance from same-identity seed photos than LFW Western embeddings are — not because the *quality* is worse, but because the *distribution shift* is real. At default ml=0.40 nothing crosses; at ml=0.30 the same set of true matches lights up, with very few false-positive crossings.

This is the cleanest possible signal that **the problem is no longer fixture-quality**. The remaining gap is the threshold (or, equivalently, an Asian-fine-tuned ArcFace head). A future per-bucket threshold (`match_lower=0.30` for eastern, `match_lower=0.40` for western) would lift overall F1 to ~0.70 with no western regression.

### Procedure (PM-A.3, current production fixture)

Script: `/tmp/rebuild-eastern-test3.py` (kept under /tmp; rerun-as-needed via the manifest's `sources.eastern.upstream_path`).

1. List `https://api.github.com/repos/jack139/face-dataset/contents/test3` → 51 anonymous-ID slug dirs.
2. Pick first 5 lex-sorted: 4 enrolled (`3131124, 3131306, 3132111, 3134589`) + 1 distractor (`3135437`).
3. For each, list contents, take first 3 (or 2 for distractor) jpgs lex-sorted.
4. Download raw bytes from `raw.githubusercontent.com/jack139/face-dataset/master/test3/<slug>/<name>` — 250×250 RGB, ~5-15 KiB each.
5. Write **native bytes, no resize, no pad** to `tests/fixtures/face/baseline/eastern_t3_<slug>/{seed_01,query_01,query_02}.jpg` (or `_distractor_eastern_t3_<slug>/{query_01,query_02}.jpg`).
6. Manifest rewrite:
   - Replace eastern entries in `enrolled_roster` (slug=`t3_<id>`, employee_no=`E-2C-E-t3_<id>`).
   - Replace eastern entry in `distractor_roster`.
   - Replace all eastern `files[]` entries (paths, sha256, bytes, native_size, src_filename, source_origin=`jack139/face-dataset/test3`).
   - `sources.eastern` rewritten with new note + `upstream_path: "test3"`.
   - Drop `params.eastern_pad_to`/`pad_fill`/`padded_at`. Add `params.eastern_native_at: "2026-04-29 PM-A.3"`.

### Comparison: padded-dlib (A.2) vs funneled-native (A.3)

| dimension | A.2 (test2 + 320 pad) | A.3 (test3 native) |
|---|---|---|
| dimension | 320×320 (synthetic) | 250×250 (matches LFW) |
| face fill | ~45% (gray border around tight crop) | ~30-50% (funneled-style real bg) |
| identity slug | celeb names (`aidai/baijingting/...`) | anon IDs (`t3_3131124/...`) |
| seed bytes (median) | ~7 KiB | ~9.5 KiB |
| ArcFace embed quality | noisy (FP=6 at ml=0.30) | clean (FP=2 at ml=0.30) |
| signals at default ml=0.40 | 1 TP eastern | 0 TP eastern |
| signals at sweep ml=0.30 | 6 TP eastern, ~5 FP | 6 TP eastern, ~1 FP |
| readability of fixture | familiar names | anonymous IDs |
| dependency on hack | yes (gray-pad) | no |

A.3 is strictly cleaner; the only "loss" is the cosmetic anonymity of slugs and the 1-TP-at-default illusion that A.2 happened to produce.

### Truth-baseline supersession

- `2c-tune-recognition-pr.json` and `2c-tune-recognition-pr-fix-A2-2026-04-29.json` are now **historical-reference only**. Their fixtures no longer exist on disk.
- New de-facto truth: `docs/baselines/2c-tune-recognition-pr-fix-A3-2026-04-29.json` (overall F1=0.500 @ default; 0.700 @ sweep ml=0.30; eastern face_det=1.0; western F1=0.667 unchanged).
- Future regressions are measured against PM-A.3 at default thresholds AND at the ml=0.30 sweep row.

### What did not change

- Server: SCRFD-500m (`5e4447f5...` 2.4 MiB). `Thresholds::DEFAULT { low_lower:0.30, match_lower:0.40, augment_upper:0.95 }` unchanged. No Rust changes.
- Western fixture: LFW-funneled 250×250, untouched. F1=0.667, face_det=1.0 unchanged.
- Distractor counts (3 total: 1 eastern + 2 western).

### Followups (the recommended next step)

- **Per-bucket match_lower** (combine Path C with Path B from the earlier menu): set `match_lower=0.30` for eastern bucket, keep 0.40 for western. Projected: overall F1 ~0.70, eastern F1 ~0.80, western F1 unchanged. Requires ~30 LoC in `server/src/inference/recall.rs` to thread bucket through to `Hit::bucket(t)`.
- Alternative: train/swap an Asian-fine-tuned ArcFace (face_embed.onnx). Higher payoff long term but multi-week scope.
- Document anonymous-ID slug convention: future eastern additions follow `t3_<numeric_id>` to keep upstream traceable.
- Audit one residual concern: a single learning-bucket entry exists for an eastern query. At ml=0.30 it crosses to TP; consider whether learning bucket should auto-flag eastern queries for human review.


---

## 2026-04-29 PM-D — Per-bucket recall thresholds (BucketThresholds)

**What changed:** In `server/src/inference/recall.rs`, introduce `BucketThresholds` and dispatch per-hit thresholds based on `persons.employee_no` prefix:
- Eastern bucket (`E-2C-E-*`): low_lower=0.20, match_lower=0.30
- Default/western: low_lower=0.30, match_lower=0.40

This is a recall-only change (no SCRFD / YOLO decode changes). The original motivation was to recover Eastern matches projected to land in the 0.30–0.40 band without lowering Western precision.

**Measured result (PM-A.3 fixture; per-bucket dispatch enabled):**

| run | overall P | overall R | overall F1 | TP | FP | FN | TN |
|---|---:|---:|---:|---:|---:|---:|---:|
| PM-A.3 baseline @default (ml=0.40) | 1.000 | 0.333 | 0.500 | 8 | 0 | 16 | 6 |
| **PM-D per-bucket** (E:0.30 / W:0.40) | **1.000** | **0.375** | **0.545** | **9** | **0** | 15 | 6 |
| reference: global ml=0.30 | 0.875 | 0.583 | 0.700 | 14 | 2 | 10 | 5 |

**Key finding (corrects the earlier hypothesis):** Eastern does *not* mostly sit in 0.30–0.40. On this fixture, eastern top1 scores are mostly ≤ 0.22; only 1 query crosses 0.30. This indicates the embedder is the real bottleneck for test3-eastern. Per-bucket thresholds still deliver a real incremental gain (+1 TP, 0 FP) but not a full unlock.

**Artifacts:**
- Baseline JSON: `docs/baselines/2c-tune-recognition-pr-fix-D-percell-2026-04-29.json`
- Eval log: `/tmp/eval-pr-D.log`
