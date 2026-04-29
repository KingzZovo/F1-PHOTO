# AGENTS.md

本仓库为本地运维与编码代理提供的工作上下文。

## 项目定位

F1-Photo 是一个本地化、离线、单机部署的工单照片归档系统：Rust + PostgreSQL + ONNX (CPU)。

- 当前推进以 `docs/v1.4.x-v1.5.0-roadmap.md` 为单一事实源；所有 milestone 以 baseline JSON/MD + 可复现脚本为准。

## 关键决策（不要随意改）

- DB 固定为 **PostgreSQL 16 + pgvector**。不引入 Redis / FAISS / Milvus。队列也走 PG（sqlx 队列表或 pgmq）。
- 推理走 onnxruntime CPU EP，所有模型必须是 INT8 量化后的轻量级。
- 前端只用 Vue 3 + Naive UI + Tailwind，不加多个 UI 库。
- 纯离线部署，不依赖任何外部 SaaS。

## 训练 vs 部署

- **训练走单独的 `tools/training/`**，仅在研发机器运行，用 Python。
- **生产环境只跑推理**，不集成 PyTorch / GPU 依赖。
- 训练产出的 ONNX 模型量化后复制到生产机的 `models/`。

## 代码风格

- Rust：`cargo fmt`，`clippy -- -D warnings`，错误用 `thiserror` + `anyhow`。
- TS：`eslint --max-warnings 0`，类型全部显式导出。
- 提交走 conventional commits，一个 commit 只做一件事。

## 服务入口

- HTTP 默认 8080（可后台改，也可部署时交互输入）。
- PostgreSQL 默认 5544（使用便携包，不冲突系统 PG）。
- 静态资源嵌入 Rust 二进制（rust-embed）。

## 目录职责

- `server/`：axum 服务 + 推理 worker + 调度。
- `web/`：管理后台 + 操作员上传页面。
- `android/`：现场拍照上传 + 轻量查看。
- `models/`：只放 ONNX，不进 git。
- `tools/training/`：离线训练 + 标注辅助脚本（Python）。
- `deploy/`：一键脚本 + systemd unit + NSSM 配置。
- `docs/`：架构 / API / 训练 / 部署文档。

## 参考

阅读顺序建议：`docs/plan.md` → `docs/architecture.md` → `docs/data_model.md` → `docs/recognition_pipeline.md` → `docs/api.md` → `docs/training.md` → `docs/deployment.md`。
