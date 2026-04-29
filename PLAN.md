# F1-Photo 当前规划与里程碑

> 单一真值源：`docs/v1.4.x-v1.5.0-roadmap.md`（带每条里程碑的 commit / 日期 / 状态）。
>
> 本文件给「人脑视角」的简版状态板，用于新窗口起手。最后刷新：2026-04-29 PM 深夜 (HEAD `84366fc`)。

## 现在在做什么

**v1.5.0 推理真链路 + 自学习闭环**。M0 架构早已结束（README/AGENTS 里的「M0 架构阶段」措辞已过时，待下次刷新）。M1+ 的代码在持续 land。

当前主线：face 识别管线硬化（独立于 tool detector retrain）。

## ABCD 状态（2026-04-29 收尾）

King 19:44 批 ABCD 全推。2026-04-29 PM 运行结果：

| 步骤 | 任务 | 状态 | 产物 |
|------|------|------|------|
| 0 | 三件套补全（PROJECT_STRUCTURE.md + PLAN.md） | ✅ done (`1600cb9`) | 三件套齐 |
| 0.5 | stale baseline 重命名 + 标记 + READ_FIRST | ✅ done (`b553fc3`/`84366fc`) | `2c-recognition-pr.stale-pre-2c-tune.json` + `READ_FIRST.md` |
| A | eastern face_detection_rate=0 诊断 | ✅ done (`ad8f0ac`) | `scrfd-eastern-fixture-diagnosis-2026-04-29.md`。**结论：fixture 双重插值 (140×147→56×256→40×640) 退化 + SCRFD-500m 容量不足**，不是 recall 阈值问题。 |
| B | face PR baseline 重跑 + 漂移快照 | ✅ 判定不需重跑（零漂移已证） | 记在 `READ_FIRST.md`：自 `e8c3358` (#2c-tune) 后仅 3 个不影响 face PR 的 commit。`2c-tune-recognition-pr.json` 仍是真值。 |
| C | gallery 质量审计 | ✅ done (`ad8f0ac`) | `gallery-quality-audit-2026-04-29.json`。**127 persons / 248 embeds**；4 cold-start、6 only-initial、119 complete (93.7%)。 |
| D | face e2e 冒烟 (enroll→embed→self-recall) | ⚭ 不单跱重跑 | 根因同 B：代码路径零漂移。`#2a-real-reverse` (`664afc3`) 已证明 119/119 self-recall score=1.0、119/0 EID truth-table；gallery 119→198 incremental 增长路径也已锁定。重跑不产生新信号。 |

另加一个零动工以便后续重跑不依赖 hardcode：

- `e042e99` tools(eval_pr): 加 `F1C_PROJECT_CODE` / `F1C_WO_CODE` env override（以前响亮名是 hardcode 的，重跑要调代码；现在可环境变量覆盖）。

## 给 King 的选择题（fixture 修复路径）

`scrfd-eastern-fixture-diagnosis-2026-04-29.md` 给了 4 个选项，**需 King 选一个才能推动 eastern.f1 从 None 出来**：

- **A. 换 fixture 源**（上拉质量）— 与现有 #2c-asia (passport-style 身份照、100% detect rate) 一致，拼他们成 eastern PR fixture。限制：可能要重新标 GT。
- **B. 换检测器**（提容量）— SCRFD-500m → SCRFD-2.5g 或 SCRFD-10g。限制：需重新 ONNX 导出、验签 shape、改 server preprocess（如果尺寸变）。
- **C. 降阈值**（SCRFD `SCORE_THRESHOLD` 0.5 → 0.3）— 可能拉高 western 偷接迹 + 性能开销。限制：是 Rust 代码改动，需你明确批准。
- **D. 接受现状**（eastern.f1=None 当作文档记载，overall.f1=0.500 作为 v1.5.0 验收阈）— 零成本。限制：你要接受「eastern 贵州」。

**推荐优先级**：**A > C > B > D**。A 几乎零代码动、与已锁定的 #2c-asia 同源；C 性价高但动 server；B 能彻底修但工量最大；D 是透明使者补魔术。

## 待 King 决策 / 行动

| 项 | 性质 | 描述 |
|----|------|------|
| **eastern fixture 修复 (A/B/C/D)** | 必选 | 以上选择题；D 代表「宁愿记载不动」 |
| #5-bootstrap CSV 214 行 | 待决策（A/B/C） | tool 模型在 ID 照上的全错检；A=一键 suppress 越过 ≥50 门控；B=作废等真实工地照；C=预筛后再决 |
| #2c-asia-wild 现场照 | 待 King 提供 | ≥50 张客户运营/工地现场抓拍照（非 ID 上下文）；用 `tools/reverse_smoke_id_photos.py` 跑评估 |

## 已完成里程碑（v1.4 → v1.5.0，按时间倒序）

### 2026-04-29 PM 深夜
- `84366fc` docs(baselines): 修复 backtick-stripped READ_FIRST PM 补充
- `ad8f0ac` docs(baselines): 2026-04-29 PM gallery audit + SCRFD eastern fixture 诊断
- `e042e99` tools(eval_pr): allow F1C_PROJECT_CODE/F1C_WO_CODE env overrides
- `b553fc3` docs(baselines): mark 2c-recognition-pr as stale (pre-#2c-tune); add READ_FIRST
- `1600cb9` docs: PROJECT_STRUCTURE.md + PLAN.md（rule-page triplet）

### 2026-04-29 PM 上半场
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

### 2026-04-28 / 更早
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
- **当前测试基线**：127 persons / 248 embeds gallery + 30 张 PR 评估夹具（10 eastern + 20 western）。这是**测试样本**，不代表系统上限。
- 任何「够用论」必须以系统真实业务规模为基准评估。`docs/baselines/` 下的 fixture 大小不能当作上限。

## 不要做的事

- 不画 bbox（业务可视化由前端处理，不混进 evaluation）。
- 不在 `manual_corrected < 50` 时触发 detector retrain。
- 不把照片提交进 git。
- 不修 server Rust（除非 King 明确要求；C 选项需 King 明批）。
- **不再使用任何历史 codename**（已在 1486f5c 全部抹除）。
