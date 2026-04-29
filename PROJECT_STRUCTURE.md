# F1-Photo 项目结构索引

> 入口锚点。新窗口接手或久未访问时第一份要读的文档。最后刷新：2026-04-29 (HEAD `9dba118`)。
>
> 配套：`AGENTS.md`（长效规则 + 路径索引）/ `PLAN.md`（当前规划与已完成里程碑）。

## 顶层目录

| 路径 | 职责 | 关键入口 |
|------|------|----------|
| `server/` | Rust axum 后端 + 推理 worker + 自学习 + retrain 编排 | `server/src/main.rs`, `server/src/lib.rs` |
| `web/` | Vue 3 + Vite + TS + Naive UI 管理后台 | `web/src/main.ts`, `web/src/router/`, `web/src/views/` |
| `android/` | Kotlin + Jetpack Compose 现场拍照 APK | `android/app/src/main/java/com/f1photo/app/` |
| `tools/` | 离线训练 / 评估 / 数据校正脚本（Python） | 见下文 §tools |
| `tests/` | Python 单测套件（仓库根级 pytest） | 见下文 §tests |
| `docs/` | 设计 / 计划 / 评估基线 / 操作手册 | `docs/plan.md`（架构）/ `docs/v1.4.x-v1.5.0-roadmap.md`（状态板） |
| `models/` | ONNX 模型放置目录（**不进 git**） | `object_detect.onnx` 等 |
| `bundled-pg/` | 便携 PostgreSQL 16 + pgvector（运行时） | `bundled-pg/bin/psql` |
| `packaging/` | Linux/Windows 一键部署脚本 + systemd/NSSM 配置 | `packaging/linux/`, `packaging/windows/`, `packaging/scripts/` |
| `dist/` | 打包产物（**不进 git**） | `f1photo-0.1.0-linux/`, `*.tar.gz` |
| `.github/` | CI 配置 | `.github/workflows/` |

## server/src 模块拓扑

```
server/src/
├── main.rs              二进制入口；axum app + worker pool + bundled-pg 启动
├── lib.rs               crate 入口（library 形态，便于集成测试）
├── cli.rs               子命令分发（serve / retrain-detector promote 等）
├── config.rs            配置加载（env + 文件）
├── db.rs                sqlx pool + 迁移驱动
├── bundled_pg.rs        便携 pg 启停（端口 55444）
├── error.rs             thiserror + axum IntoResponse
├── logging.rs           tracing 初始化
├── audit.rs             审计日志写入
├── static_assets.rs     rust-embed 嵌入 web/dist
├── retrain.rs           retrain orchestration + EvalDeltas schema (#7c-eval-skel)
├── finetune.rs           detector finetune 编排（#5-bootstrap retrain 路径）
├── auth/
│   ├── mod.rs            模块门面
│   ├── jwt.rs            access_token 签发（F1-photo 专用字段名）
│   ├── extractor.rs      axum extractor（项目权限 / admin 中间件）
│   └── password.rs       argon2 hash + verify
├── api/
│   ├── mod.rs            路由聚合（/api/...），含 :142 的 correct 路由
│   ├── health.rs         /healthz（端口 18799）
│   ├── auth.rs           登录（响应字段 access_token，兼容旧 token）
│   ├── projects.rs       项目 CRUD
│   ├── work_orders.rs    工单 CRUD
│   ├── photos.rs         上传 / 缩略图 / 打包下载
│   ├── recognitions.rs   识别条目 + correct_item handler (:335)
│   ├── persons.rs        人员主数据（employee_no 全局唯一）
│   ├── tools.rs          工具主数据（sn 全局唯一）
│   ├── devices.rs        设备主数据（sn 全局唯一）
│   ├── settings.rs       后台可调参数（阈值 / 上限）
│   └── admin.rs          /api/admin/* 跨项目检索
├── inference/
│   ├── mod.rs            model registry + slot 调度（face_detect / face_embed / object_detect / generic_embed）
│   ├── models.rs         ONNX session 初始化（onnxruntime 1.18 CPU）
│   ├── preprocess.rs     图像预处理（resize / normalize / letterbox）
│   ├── scrfd.rs          SCRFD-500m 人脸检测（landmark yaw 启发式角度）
│   ├── yolov8.rs         YOLOv8n COCO + per-crop DINOv2 embedding
│   └── recall.rs         pgvector 召回 + 阈值 sweep
└── worker/
    └── mod.rs            sqlx 队列表 worker pool（不依赖 Redis）
```

## tools/（Python 离线脚本）

| 文件 | 职责 |
|------|------|
| `ingest_id_photos.py` | ID 照入库（enrollment）。登录字段兼容 `access_token` / `token`。 |
| `ingest_id_photos_smoke.sh` | ↑ 烟测包装。 |
| `reverse_smoke_id_photos.py` | wo_raw 反向冒烟（验证识别召回链路）。 |
| `eval_distribution.py` | #2-tool 分布评估（per_photo / recognition_items_total）。 |
| `eval_pr.py` | #2c face PR 评估（per_bucket_at_default + threshold_sweep）。 |
| `build_tool_fixture.py` | 工具固定夹具构建。 |
| `bootstrap_correction_collector.py` | #5-bootstrap 校正数据采集 → CSV+JSON 工作表（read-only）。 |
| `bootstrap_correction_importer.py` | #5-bootstrap 校正数据导入 → PATCH `/correct`（dry-run 默认）。 |
| `shadow_eval.py` | #7c-eval-auto 候选 ONNX 影子评估，产出 `EvalDeltas`。 |
| `shadow_eval_smoke.sh` | ↑ 烟测包装。 |
| `retrain_train.py` | detector retrain 训练驱动（YOLOv8n + ultralytics）。 |
| `retrain_smoke.sh` | ↑ 烟测包装。 |

## tests/（pytest）

```
tests/
├── fixtures/                                   测试夹具（图片、JSON）
├── test_bootstrap_correction_importer.py       38/38 PASS（_truthy / classify / read_csv / build_summary）
└── test_shadow_eval.py                          21/21 PASS（sha256_file / utc_now_iso / parse_*_report / assemble_eval_deltas）
```
仓库全测：**59/59 PASS in 0.10 s**（HEAD `9dba118`）。运行：`python3 -m pytest tests/ -v`。

## web/src（Vue 3）

```
web/src/
├── main.ts             入口
├── router/             vue-router
├── stores/             pinia stores
├── api/                axios 客户端封装
├── views/              路由级视图
├── layouts/            布局壳
├── components/         可复用组件
└── composables/        组合式逻辑
```

## android/app/src/main/java/com/f1photo/app（Kotlin + Compose）

```
app/
├── data/
│   ├── api/            Retrofit 客户端
│   ├── db/             Room 本地缓存
│   └── work/           WorkManager 上传任务
├── di/                 依赖注入（Hilt 风格）
└── ui/
    ├── upload/         拍照 + 上传
    ├── workorders/     工单列表
    ├── queue/          待传队列
    ├── settings/       设置页
    └── theme/          Compose 主题
```

## docs/

| 文档 | 内容 |
|------|------|
| `plan.md` | 完整开发计划（项目目标 / 范围 / 架构 / 技术栈 / M0–M*） |
| `architecture.md` | 架构图与组件说明 |
| `data_model.md` | DB schema |
| `permissions.md` | 项目级 RBAC + 全局主数据权限 |
| `recognition_pipeline.md` | 识别 / 自学习流水线细节 |
| `api.md` | HTTP API 完整规范 |
| `training.md` | 标注 + 训练指南 |
| `deployment.md` | 离线部署与一键脚本 |
| `operations.md` | 运维 SOP |
| `v1.4.x-v1.5.0-roadmap.md` | 当前版本里程碑状态板（**单一真值源**） |
| `handover-2026-04-29.md` | 跨窗口接手 prompt（每天刷新） |
| `baselines/` | 评估基线 JSON（#2 / #2a / #2c / #5 / #7c） |
| `diagrams/` | 架构图 / 流程图 |
| `TODO-android.md`, `TODO-deferred.md` | 延后队列 |

## 服务运行口径（运行时实参，非默认值）

| 项 | 值 |
|----|----|
| 仓库 | `/root/F1-photo` |
| 分支 | `main` |
| Server HTTP | `127.0.0.1:18799`（PID 2955495） |
| Bundled-pg | `127.0.0.1:55444`（PID 2954647），用户 `f1photo`，密码 `smokepwd`，库 `f1photo_prod` |
| psql | `bundled-pg/bin/psql` |
| Admin | `smoke_admin` / `smoke-admin-pwd-12345`，user.id `338d6733-26a8-43bf-83e0-f4c666619c06` |
| Login response | 字段 `access_token`（客户端兼容旧 `token`） |
| Detector retrain gate | `manual_corrected_total ≥ 50`（当前 0/50） |

## ID 主键参考

| 项目 | code | UUID |
|------|------|------|
| ID 照源 (Path A) | `id-photo-2a` | `f06e30b8-8aeb-487a-b67c-59354442bb01` |
| wo_raw 反向冒烟 | `id-photo-2a-reverse` | `f54d2a48-537c-4ab3-8f5b-826662465410` |
| #2-tool baseline | — | `f9a13cf7-9bb8-440a-b99d-03b799ce5abf` (WO `30af3e75-406e-43f2-9c5c-12404484e623`) |
| #2-face rerun | — | `cac02bd7-bfff-41cc-b23b-8230018510d3` (WO `a7895bfc-1e18-424d-a58a-02bb497999d5`) |

## 已固化的 schema 差异（踩坑记录）

- `recognition_items` 没有 `target_type` 列（target 走关联结构）。
- `match_status` enum：`matched, learning, unmatched, manual_corrected`。
- `owner_type` enum：`person, tool, device, wo_raw`。
- `CORRECTION_OWNER_TYPES`（importer 端）：`["person", "tool", "device"]`（不含 wo_raw）。
- `correct_item` 路由：`server/src/api/mod.rs:142` → handler `recognitions.rs:335 pub async fn correct_item`。
- `CorrectInput { owner_type: Option<String>, owner_id: Option<Uuid> }`。
