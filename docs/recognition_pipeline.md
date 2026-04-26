# 识别与自学习流水线

> 与 [permissions.md](permissions.md) 配套：所有识别步骤都在「项目作用域」内，不会跨项目串数据。

## 1. 总体阶段

```
上传 → 预处理 → 检测 → Embedding → 角度 → kNN(项目内) 匹配 → 决策 → 归档 ⮕ SSE 推送
```

所有阶段走后台 Worker，上传接口不阻塞。

## 2. 预处理

- 读取 `data/orig/{hash}.{ext}`。
- EXIF 旋转校正。
- 准备两个 tensor 输入：
  - 640×640 BGR float32，YOLOv8n 主体检测。
  - 原图另作人脸 SCRFD 输入（SCRFD 自带 letterbox）。

## 3. 检测分支

### 3.1 人脸 (target_type=face)

- 模型：`scrfd_500m_int8.onnx`。
- 输出：bbox + 5 landmark + score。
- 过滤：score ≥ 0.5，面积 ≥ 32×32。

### 3.2 工具 / 设备 (target_type=tool|device)

- 模型：`yolov8n_int8.onnx`，按项目级 `recognition_projects` 设置限定类号。
- score ≥ 0.4、面积 ≥ 96×96 的主体框。
- 同图多主体按面积降序取前 N。

## 4. Embedding

### 4.1 人脸

- 5 landmark 仿射 → 112×112 → `mobilefacenet_arcface_int8.onnx` → 512 维 L2 归一化向量。

### 4.2 工具 / 设备

- bbox 外扩 10% → 224×224 → `dinov2_small_int8.onnx` → 384 → 线性映射到 512 → L2 归一化。
  - 映射矩阵随模型一起嵌入资源中，不随训练在线更新。
  - 选 512 维与人脸一致，pgvector 列类型统一。

## 5. 角度判定（人员）

### v1 启发式（默认）

- 由 SCRFD 5 landmark 计算 yaw。
- |yaw| < 25° → `front`
- 25° ≤ |yaw| < 70° → `side`
- |yaw| ≥ 70° → `back` 推断；无人脸但检到背影 → 后期分类器
- 未能判定 → `unknown`

### v2 训练后

- 使用 `models/angle_cls.onnx`，项目设置 `angle_classifier=enabled` 后切换。

## 6. 匹配与决策

### 6.1 kNN（强制项目隔离）

```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE project_id = $2
  AND owner_type = $3
ORDER BY embedding <=> $1
LIMIT 5;
```

- 项目级覆盖优先：`projects.overrides.match_threshold` 覆盖全局默认 `0.62`。
- top1.score ≡ 1 - cosine_distance。
- HNSW 全局，但前置 `project_id + owner_type` 过滤；规模上去后可考虑 partitioned index。

### 6.2 状态机

| top1 score | 状态 | 动作 |
|---|---|---|
| ≥ threshold | matched | 绑定 owner、归档、SSE matched |
| [low, threshold) | learning | 写一条 incremental embedding（同 project_id）、SSE learning、归档 |
| < low | unmatched | 写 recognition_items(unmatched)（同 project_id）、SSE unmatched、不归档 |
| matched 且 ∈ [threshold, 0.95) | matched + augment | 额外写一条 incremental embedding 「补上不匹配的 10%」 |

### 6.3 冲突与调和

- 上传时用户已默认填 owner，但识别结果不一致：
  - 保留用户值，`detection.match_status = matched` 不覆盖 `photos.owner_id`。
  - 写一条 `recognition_items(suggested_owner_id, status=manual_corrected)` 给人工复核。
- 用户还未填、识别后才填：识别先 cache 到 photos，用户保存时拿到结果联动。
- 跨项目串图：同一物理 hash 在不同项目独立成行，识别只在本项目库内匹配。

## 7. 归档命名规则

```
data/archive/{project_code}/{wo_code_prefix3}/{YYYYMM}/{wo_code}_{owner_name}_{angle}_{seq:03}.{ext}
```

- `project_code`：来自 `projects.code`，避免跨项目同名工单冲突。
- `wo_code_prefix3`：`wo_code` 前 3 位，避免单目录过大。
- `owner_name`：去除不安全字符。
- `angle`：front / side / back，非人员为 `view`。
- `seq`：同 (project_id, wo_code, owner_id, angle) 的序号。
- 原 `path` 字段保留，归档路径写入 `archive_path`。

## 8. 人工纠错闭环

```mermaid
flowchart LR
    R[recognition_items 列表\n按项目过滤] --> Open[点开详情]
    Open --> Pic[查看红框预绘图]
    Pic --> Choose[选择正确 owner / 新建\n仅本项目实体]
    Choose --> Save[保存]
    Save --> Update[更新 detections + photos]
    Save --> Embed[插入 identity_embeddings\n source=manual, project_id 一致]
    Save --> Audit[audit_log]
    Save --> Reproc[重跑同 project_id 同 hash 未匹配项?]
```

## 9. 手动「快速建库」入口

- `Persons / Tools / Devices` 页（在选定项目下）提供「快速建档」按钮：上传多张同一实体照 → 后端跑一轮检测/embedding → 创建身份 + 写多条 `identity_embeddings(project_id, source=initial)`。
- 让新项目能快速冷启动。

## 10. 错误与重试

- Worker 抓取 `recognition_queue` 记录，失败 attempts++，超过 5 次记录错误进人工复查。
- 模型加载失败使服务处于 `not ready`，`/readyz` 返 503。
- 推理单次超时默认 30s，返回 `failed`。

## 11. 性能预期

- 单张（一人 + 一工具）：检测 ~150ms + face emb ~50ms + tool emb ~200ms = **~400ms**。
- 8 并发 worker、CPU 10C20T 环境，理论吞吐 ~50 photo/s，实际限于磁盘与 DB。
- 项目隔离对吞吐影响可忽略：HNSW 检索本身比加 `project_id` 过滤代价大得多，过滤只是在候选集上快速 prune。
