# F1-Photo 完整开发计划

> 状态：M0 架构阶段 · 仓库：`/root/F1-photo` · 主语言：Rust + TypeScript + Kotlin

## 1. 项目目标

为现场运维场景提供一套本地化、离线、可自学习的工单照片归档系统：

- 一张工单照 → 自动抽取人员 / 工具 / 设备主体 → 抽特征值 → 与名称绑定 → 自动改名归档。
- 同一人/工具/设备反复出现 → 从「人工填写」逐步过渡到「自动填充」。
- 所有识别走后台异步，不阻塞前端。
- 后台可调：阈值、上传上限、平台名、识别项目、匹配规则。
- **多账号 + 项目级权限隔离**：可配置独立空间或共享空间。
- 纯离线部署，Linux + Windows，一键脚本。
- 提供 Android APK、版本接口。

## 2. 范围与非范围

### 在范围内
- 「项目 → 工单 → 人员/工具/设备 → 照片」四层结构。
- 项目级 RBAC：每个账号对每个项目独立配置 `view / upload / delete / manage` 4 个权限位。
- 人员 / 工具 / 设备 / 工单原图的上传、识别、归档、检索、打包下载。
- 特征值匹配 + 增量自学习（同身份多 Embedding 保留），按项目隔离。
- 人脸角度分类（正 / 侧 / 背）。
- 后台可视化调参与人工纠错。
- Android 拍照、上传、查询、版本自检。
- Linux + Windows 一键部署。

### 不在范围内（或后续版本）
- 跨项目自动身份合并（手动「身份库复制」做兜底）。
- 云端 SaaS / 多机集群。
- GPU 推理优化（预留接口，不优先实现）。
- OCR 读取工单号（如果工单号由人工输入则不需，需要时后期加）。
- 商业 / 跨企业目录树。

## 3. 架构总览

三层划分：

1. **接入层**：Web 后台 + Android APP，走 HTTP/JSON + multipart upload。所有业务请求都带 `project_id`。
2. **服务层**：Rust axum，同进程内启动推理 worker pool，包含项目权限中间件。
3. **存储层**：PostgreSQL 16 + pgvector（按项目隔离的元数据 + 特征向量 + 队列），本地文件系统存原图 / 缩略图 / 归档产物（归档路径以 `project_code` 开头）。

详见 [architecture.md](architecture.md) 与 [permissions.md](permissions.md)。

## 4. 技术栈定版

| 层 | 选型 | 理由 |
|---|---|---|
| 后端 | Rust 1.83+, axum 0.7, tokio 1.x, sqlx 0.8 | 单进制、绿色线程、类型安全 |
| DB | PostgreSQL 16 + pgvector 0.7 | 原生向量检索，免外部组件 |
| 队列 | sqlx 队列表（可迁 pgmq） | 不引 Redis |
| 缓存 | moka in-memory LRU | 足够单机 |
| 推理 | onnxruntime 1.18 (CPU EP) | 跨平台、多语言友好 |
| 人脸 | InsightFace SCRFD-500m + MobileFaceNet (ArcFace) | CPU 可跑，embedding 512d |
| 工具/设备 | YOLOv8n (检测主体) + DINOv2-small (embedding) | 高质量通用视觉 embedding |
| 角度 | 启发式初版（基于 SCRFD landmark yaw） → MobileNetV3 INT8 训练后补上 | 先跑起来，后补模型 |
| 前端 | Vue 3 + Vite + TS + Naive UI + Tailwind + Pinia | 美观 + 快 |
| 手机 | Kotlin + Compose + Retrofit + Coil | 官方栈，体积小 |
| 部署 | rust-embed 嵌静态 + 便携 PG + ONNX runtime + NSSM/systemd | 纯离线 |

## 5. 里程碑

### M0 架构阶段（当前）
- [x] 仓库 + 文档（含 permissions.md）
- [x] 架构图 / API 调参 / 数据模型
- [ ] 技术选型 POC（onnxruntime CPU 跑 InsightFace 手动验证）

### M1 地基（1.5–2.5 周）
- Rust 骨架、配置、日志、错误。
- DB 迁移，启用 pgvector，初始化 `default` 项目。
- JWT 鉴权、关闭开放注册、`admin / member` 角色。
- **项目 + 成员管理**：CRUD `projects` + `project_members`，axum extractor `RequireProjectPerm`。
- 工单 / 人员 / 工具 / 设备 CRUD（全部项目作用域）。
- 上传 + 哈希去重（项目内）+ WebP 缩略图。
- Vue 骨架 + 登录页 + 项目切换器 + 列表页。
- admin 控制台「项目管理」页（建/删项目、加/移成员、配权限位）。

### M2 推理 v1（1–2 周）
- onnxruntime 接入（动态加载模型、线程池隔离）。
- 人脸检测 + embedding。
- pgvector kNN 查询（强制按 `project_id` 过滤）。
- 未匹配 / 增量学习决策逻辑。
- SSE 识别事件推送（按用户可见项目过滤）。
- 自动改名归档（路径含 `project_code` 前缀）。

### M3 工具 / 设备识别（1–2 周）
- YOLOv8n 主体检测。
- DINOv2-small embedding。
- 工具库 / 设备库建库 + 匹配 + 自学习（项目隔离）。
- POC 验证识别率。

### M4 后台调参 + 识别条目页（1 周）
- 全局 settings + 项目级 overrides 双层热更新。
- 识别条目列表（按项目）+ 红框预绘原图 + 人工纠正。
- 纠正后自动写入新 embedding 到本项目。

### M5 体验层（1 周）
- 按工单 / 人员 / 工具 / 设备检索 + 打包 zip 下载（项目内）。
- 侧栏 3s 浮窗、列表预览图、批量选中管理。
- UI 美观化（Naive UI 主题调优）。

### M6 Android APK + 版本接口（1 周）
- 登录 → 选项目 → 拍照上传 / 工单列表 / 识别结果查看。
- 启动时调 `/api/app/latest` 检查更新。

### M7 部署与发布（1 周）
- Linux：`install_linux.sh`，systemd。
- Windows：`install_windows.ps1`，NSSM。
- 便携 PG 打包（`pg_ctl initdb` + `CREATE EXTENSION vector`）。
- 验证：干净 VM 上冷装走通。

### M8 训练闭环（1 周，需样本）
- 角度分类训练闭环，详见 [training.md](training.md)。

## 6. 依赖与风险

| 风险 | 描述 | 缓解 |
|---|---|---|
| 识别准确率不足 | 工具/设备同型号难区分 | 增量自学习 + 人工纠错 + 按需上专用模型 |
| CPU 推理吞吐 | 突发上传会排队 | 后台 worker 限并发，接口立即返回 |
| 冷启动体验 | 首次上传都需手填 | 「快速建档」入口；项目内复用历史标注 |
| 项目隔离误用 | 把数据建错项目难迁移 | 后台提供「迁移工单到其他项目」工具（admin） |
| Windows 纯离线 | 依赖 onnxruntime DLL | 随包发布，预装 VC++ Redist |
| pgvector 性能 | 超过 10万 embedding 后 | HNSW + 项目过滤前缀，定期 reindex |
| 人脸隐私 | 本地存储人脸照 | 完全本地，不上云；DB 启用加密文件系统可选 |

## 7. 决策记录（ADR 轻量版）

- **ADR-001**：DB 选 PostgreSQL（非 SQLite）— pgvector 生态、服务端集中、后续可扩。
- **ADR-002**：不引入外部消息队列 — 单机场景，队列表 + LISTEN/NOTIFY 足够。
- **ADR-003**：推理走 onnxruntime CPU，不集成 PyTorch — 生产机不训练。
- **ADR-004**：训练与生产分离 — `tools/training/` 只在研发机使用。
- **ADR-005**：Web 只选 Vue 3 + Naive UI，不接入另外 UI 库。
- **ADR-006**：引入「项目」层 + 项目级 RBAC，作为多租户初版；admin 仍可跨项目操作。
- **ADR-007**：`identity_embeddings.project_id NOT NULL`；不支持跨项目自动匹配。
- **ADR-008**：弃用 `operator / viewer` 全局角色，迁到项目内 4 个布尔权限位。

## 8. 验收指标

- 上传返回延迟 < 300ms（10MB 以内）。
- 后台识别起始延迟 < 5s（50 张背压下平均）。
- 人脸识别 TOP-1 准确率 ≥ 95%（库有 50+ 人、每人 5+ 质量可接受照片，**项目内**）。
- 工具/设备识别 TOP-1 准确率 ≥ 85%（同质量样本下，项目内）。
- 项目隔离硬指标：跨项目接口越权调用必须返 `PROJECT_FORBIDDEN`，无任何越权读写。
- 冷装验证：干净 Ubuntu 22.04 / Windows 11 纯离线机（一键脚本 < 10 分钟部署）。

## 9. 发布与版本

- 语义化版本号 `MAJOR.MINOR.PATCH`。
- `app_versions` 表提供 APK 更新元数据。
- 后端与前端同进程发布，版本号嵌入二进制。
- 一键脚本支持「覆盖升级」与「首次安装」两种模式；升级会跑 sqlx-migrate。
