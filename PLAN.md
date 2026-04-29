# F1-Photo 当前规划与里程碑

> 单一真值源：`docs/v1.4.x-v1.5.0-roadmap.md`（带每条里程碑的 commit / 日期 / 状态）。
>
> 本文件给「人脑视角」的简版状态板，用于新窗口起手。最后刷新：2026-04-29 PM (HEAD `9dba118`)。

## 现在在做什么

**v1.5.0 推理真链路 + 自学习闭环**。M0 架构早已结束（README/AGENTS 里的「M0 架构阶段」措辞已过时，待下次刷新）。M1+ 的代码在持续 land。

当前主线：face 识别管线硬化（独立于 tool detector retrain）。

## 当前进行中（face 这条线，King 已批 ABCD 全推 2026-04-29 19:44）

| 步骤 | 任务 | 状态 |
|------|------|------|
| 0 | 三件套补全（PROJECT_STRUCTURE.md + PLAN.md） | 🔄 in progress（本 commit） |
| A | 分 bucket threshold sweep（**重新校准目标**：把 eastern.f1 从 None 拉出来；之前误以为 0.667 是 Asian floor，实际是 western，eastern 默认完全零召回） | ⏳ next |
| B | face PR baseline 重跑 + 漂移快照 | ⏳ queued |
| C | gallery 质量审计（119 人 embedding 距离矩阵） | ⏳ queued |
| D | face e2e 冒烟（enroll → embed → match self-recall） | ⏳ queued |

## 待 King 决策 / 行动

| 项 | 性质 | 描述 |
|----|------|------|
| #5-bootstrap CSV 214 行 | 待决策（A/B/C） | tool 模型在 ID 照上的全错检；A=一键 suppress 越过 ≥50 门控；B=作废等真实工地照；C=预筛后再决 |
| #2c-asia-wild 现场照 | 待 King 提供 | ≥100 张客户运营/工地现场抓拍照（非 ID 上下文）；用 `tools/reverse_smoke_id_photos.py` 跑评估 |

## 已完成里程碑（v1.4 → v1.5.0，按时间倒序）

### 2026-04-29 PM
- `9dba118` docs(roadmap+handover): 记录 #7c-eval-auto 硬化
- `adaa30d` test: shadow_eval 纯函数单测（21/21 PASS in 0.06 s）
- `ff55429` docs(roadmap+handover): 2026-04-29 PM 决策（codename 禁令 / importer 单测 / rule-page 锚点 / #2c-asia-wild 源 A）
- `ff35fc8` test: bootstrap_correction_importer 单测（38/38 PASS in 0.08 s）
- `1486f5c` chore: 抹除历史代号；docs/tools 全部中性化

### 2026-04-29 AM/中午
- `1592514` docs(roadmap+handover+baseline): #7c-eval-self-wiring 烟测 + 修正 #2-face-rerun schema 误判
- `3c9cb50` docs(roadmap+handover): #5-bootstrap-importer 完成 (b7dab53)
- `b7dab53` feat(corrections): bootstrap_correction_importer.py + 5-cat dry-run smoke
- `9a4f6b3` docs(roadmap+handover): #2-face-rerun partial-zero-drift verdict
- `1741590` feat(eval): #2-face partial-zero-drift verification rerun
- `e0fb82d` docs(handover): single-page truth-table consolidation
- `20d98e4` docs(eval): #2c-asia tool_false_positive_finding class_id 量化

### 2026-04-28 / 早期
- ed0a102 collector / c03a908 docs / 更早的 #2 / #2a / #2c / #2c-tune / #7c-eval-skel 系列
- v1.4.x → v1.5.0 真链路切换：Pre-A `a612c7c` env-gated stub fallback；Pre-A `18f020d` DINOv2-small `generic_embed` 真接；Step A `cf201f7` SCRFD `face_detect` + ArcFace `face_embed` 真接；Step B `c037a65` YOLOv8n `object_detect` 真接 + per-crop DINOv2；Step C `bb97a01` 收紧 smoke + 默认关闭 stub fallback

## Backlog（v1.5.0 后续 / v1.6+）

| ID | 描述 | 阻塞条件 |
|----|------|----------|
| #2c-asia-wild | Asian 在野 face 评估（source A locked：客户运营现场抓拍） | King 提供照片 |
| #5-bootstrap (retrain) | detector retrain 触发 | `manual_corrected ≥ 50`（当前 0/50） |
| #3 | recall 精度提升 | — |
| #4 | Android APK 打磨 | — |
| #6 | MobileNetV3 角度分类 INT8 ONNX | — |

## 系统目标 vs 测试样本（提醒）

- **系统目标**：F1-Photo 是工单照片归档系统，按生产环境单机部署 + 多账号 + 项目级 RBAC + 跨工单/跨项目主数据复用设计。容量按真实业务（数千工单 / 数十万照片 / 全员脸库）规划。
- **当前测试基线**：119 人 ID 照 + 30 张 PR 评估夹具（10 eastern + 20 western）。这是**测试样本**，不代表系统上限。
- 任何「够用论」必须以系统真实业务规模为基准评估。`docs/baselines/` 下的 fixture 大小不能当作上限。

## 不要做的事

- 不画 bbox（业务可视化由前端处理，不混进 evaluation）。
- 不在 `manual_corrected < 50` 时触发 detector retrain。
- 不把照片提交进 git。
- 不修 server Rust（除非 King 明确要求）。
- **不再使用任何历史 codename**（已在 1486f5c 全部抹除）。
