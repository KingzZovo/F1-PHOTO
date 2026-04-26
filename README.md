# F1-Photo

工单照片归档系统：以人脸 / 工具 / 设备特征值匹配 + 自学习为核心的本地化照片归档平台。

- 后端：Rust (axum + tokio) + PostgreSQL 16 + pgvector
- 推理：onnxruntime（CPU INT8） — InsightFace + YOLOv8n + DINOv2-small
- 前端：Vue 3 + Vite + TypeScript + Naive UI + Tailwind
- 移动端：Android (Kotlin + Jetpack Compose)
- 部署：纯离线，单机版，Linux + Windows 一键脚本

## 目录

```
F1-photo/
├── server/      # Rust 后端
├── web/         # Vue3 前端
├── android/     # Android APK
├── models/      # ONNX 模型放置目录
├── deploy/      # 一键部署脚本 + 便携 PG
├── tools/       # 标注 / 训练脚本（Python，仅离线训练用）
└── docs/        # 全部设计文档
```

## 核心文档

| 文档 | 说明 |
|---|---|
| [docs/plan.md](docs/plan.md) | 完整开发计划与里程碑 |
| [docs/architecture.md](docs/architecture.md) | 系统架构图与组件说明 |
| [docs/api.md](docs/api.md) | HTTP API 完整规范 |
| [docs/data_model.md](docs/data_model.md) | 数据库 schema |
| [docs/recognition_pipeline.md](docs/recognition_pipeline.md) | 识别 / 自学习流水线细节 |
| [docs/training.md](docs/training.md) | 标注 + 训练指南（含环境要求） |
| [docs/deployment.md](docs/deployment.md) | 离线部署与一键脚本 |

## 当前状态

架构与文档阶段（M0），未开始 M1 编码。

## License

内部项目，未对外开源。
