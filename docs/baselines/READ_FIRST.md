# Baselines 读你之前先看这

> 最后刷新：2026-04-29。
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

## Stale / 已被取代（不要参考！）

| Stale 文件 | 原因 | 被谁取代 |
|----------|------|----------|
| `2c-recognition-pr.stale-pre-2c-tune.json` | Pre-#2c-tune 阈值 (low_lower=0.5, match_lower=0.62)；server `Thresholds::DEFAULT` 已重调为 (0.30, 0.40) | `2c-tune-recognition-pr.json` |

该文件顶层已加 `"_stale": true` + `"superseded_by": "2c-tune-recognition-pr.json"` 标记。

## 阈值跳起记录

- **#2c 阶段**：`Thresholds::DEFAULT { low_lower: 0.50, match_lower: 0.62, augment_upper: 0.95 }`。F1 在 30 张 fixture 上只有 0.154。
- **#2c-tune 重调 (HEAD `9dba118` 仍生效)**：`Thresholds::DEFAULT { low_lower: 0.30, match_lower: 0.40, augment_upper: 0.95 }`。F1 提到 0.500（overall）。
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
