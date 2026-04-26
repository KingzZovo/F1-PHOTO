# 标注与训练指南

> 本文回答：模型如何训、怎么标注、需要什么环境、是否在生产机训练。

## 0. TL;DR

- **训练在独立的研发机上做，不在生产机训。**
- 生产机只跑 ONNX 推理，不装 PyTorch / CUDA。
- 训练产出 = ONNX 文件，量化后复制到生产机的 `models/`。
- F1-Photo 在生产运行中的「自学习」是 *embedding 库的增量*，不是重训模型。

## 1. F1-Photo 中模型来源

| 模型 | 是否需要自训 | 说明 |
|---|---|---|
| 人脸检测 SCRFD-500m | 不需要，直接用 InsightFace 官方 ONNX | 通用性强 |
| 人脸表示 MobileFaceNet ArcFace | 不需要，直接用官方权重 | 中亚人脸鲁棒 |
| 工具/设备检测 YOLOv8n | 优先不训。后台 setting 限制 COCO 类别。如果工具/设备完全不在 COCO 80 类内 → 才补一头训练 | 低优先 |
| DINOv2-small embedding | 不需要，官方权重 | zero-shot 表达能力够 |
| 角度分类（front/side/back） | **需要训，样本量小**。v1 启发式占位，v2 训练补上 | 训练闭环主体 |

下面重点描写 *角度分类* 与 *YOLOv8n 头补训* 两个闭环。

## 2. 环境与硬件要求

### 推荐配置（独立训练机，任一可用）

| 场景 | 最低配置 | 推荐配置 |
|---|---|---|
| 角度分类（MobileNetV3） | 8 核 CPU + 16GB RAM 即可 | RTX 3060 12GB / RTX 4070 12GB / Mac M2 16GB |
| YOLOv8n head 补训（可选） | RTX 3060 12GB + 32GB RAM | RTX 4070 / RTX 4090 / A10 24GB |
| 数据集校验 + ONNX 导出 | 任何 8 核 16GB 机器 | 同上 |

### 软件依赖

```
Python 3.10/3.11
CUDA 12.1（可选；纯 CPU 也能训）
PyTorch 2.3 (cu121 / cpu)
torchvision 0.18
ultralytics 8.x          # YOLO 训 / 导出
opencv-python 4.x
onnx 1.16, onnxruntime 1.18
onnxsim 0.4              # 简化
onnxruntime-tools        # 量化
numpy / pandas / pillow / albumentations / scikit-learn
```

一键安装在 `tools/training/requirements.txt`。

### 与生产机的关系

```
[研发机（有 GPU 或快 CPU）]                [生产机（无 GPU）]
  python tools/training/...        复制      models/
  训练 / 量化 / 导出 ONNX  ---------------->  *.onnx
  不安装到生产机                              只跑 onnxruntime
```

生产机不装 PyTorch / CUDA / Conda。

## 3. 角度分类 训练闭环详解

### 3.1 数据采集

- 目标：每类（front / side / back）≥1000 张，理想 3000+。
- 现场拍摄可复用：生产里上传的人脸裁切图可以导出。
- 补充：公开数据集（如 BIWI HeadPose、AFLW2000-3D、CelebA-HQ 拼凑）。

导出脚本 `tools/training/export_face_crops.py`（M2 完成后提供）：读生产 DB 中 `detections(target=face)` 与 bbox，裁出 face crop 打包到 zip，复制到研发机。

### 3.2 标注方式

#### 方式 A：后台「识别条目页」充当标注器（推荐）

- 在生产后台中给该页加一个「角度标注」标签开关。
- 后台过滤出 target=face 且 angle=unknown / heuristic 的项，点三个按钮之一（front/side/back）。
- 标注结果进 `detections.angle`，同时记入 `audit_log(action=label_angle)`。
- 导出脚本 `tools/training/export_angle_dataset.py` 从生产拉 face crop + label 打包 zip。
- 优点：标注同时改善生产匹配质量，一鱼两吃。

#### 方式 B：本地文件夹手工标注

- 目录结构：
  ```
  dataset/angle/
  ├── train/  front/  side/  back/
  └── val/    front/  side/  back/
  ```
- 文件名不重要，只看所在子目录。
- 推荐用于补充公开数据。

#### 方式 C：Label Studio（多人协作时）

- 标注后导出 CSV，转换成 A/B 其中一种结构。

### 3.3 训练

入口：`tools/training/train_angle.py`（M8 交付）。

```bash
cd tools/training
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt

python train_angle.py \
  --data ./dataset/angle \
  --backbone mobilenetv3_small \
  --img-size 112 \
  --epochs 25 \
  --batch-size 128 \
  --lr 1e-3 \
  --output runs/angle_v1
```

- 堆叠常规 augmentation（hflip / brightness / blur / cutout）。
- 选 mobilenetv3_small（~2.5M 参数），CPU 推理 < 10ms。
- 验证集 macro-F1 ≥ 0.92 才算通过。

训练日志/混淆矩阵写到 `runs/angle_v1/`。

### 3.4 导出 ONNX + 量化

```bash
python export_onnx.py \
  --ckpt runs/angle_v1/best.pt \
  --out  runs/angle_v1/angle_cls.onnx \
  --img-size 112

python quantize_int8.py \
  --in  runs/angle_v1/angle_cls.onnx \
  --out runs/angle_v1/angle_cls_int8.onnx \
  --calib ./dataset/angle/val
```

- 验证 INT8 模型在 val 上掉点 < 1% F1。
- 复制 `angle_cls_int8.onnx` 到生产机 `models/`。
- 在后台 `Settings → Recognition Projects → angle_classifier → enable=true`。
- 不需要重启服务，热加载。

### 3.5 上线后回流

- 用户每次纠正角度 → audit_log + detections.angle。
- 周期性（半月一次）跑 `export_angle_dataset.py` 重训 → 出 v2 → 替换。
- 模型版本写入 `settings`，便于回滚。

## 4. YOLOv8n 补训（可选闭环）

仅当工具/设备明显不在 COCO 80 类内（典型场景：仪表、特殊扳手、专用配件）才需要走这一步。

### 4.1 标注

- 用 Roboflow / CVAT / Label Studio 之一画 bbox。
- 类别数量控制在 ≤30 类，其余归 `other`。
- 训练集每类 ≥300 框，验证集每类 ≥50 框。

### 4.2 训练 + 导出

```bash
yolo task=detect mode=train \
     model=yolov8n.pt \
     data=tools.yaml \
     epochs=80 imgsz=640 batch=32

yolo export model=runs/detect/train/weights/best.pt format=onnx opset=17 simplify=True
python quantize_int8.py --in best.onnx --out yolov8n_tools_int8.onnx --calib data/val/images
```

### 4.3 上线

- 复制到生产 `models/yolov8n_tools_int8.onnx`。
- 后台 `Settings → Recognition Projects → tool_detector` 切换为该模型。
- 类别 → 业务类型映射在 `settings.recognition_projects.tool_detector.class_map`。

## 5. 「自学习」与「重训」的边界

| 行为 | 何时发生 | 数据存哪 | 是否需要研发机 |
|---|---|---|---|
| 增量 embedding（增量自学习） | 生产线上每张图 | `identity_embeddings` (PG) | **不需要**，全自动 |
| 人工纠错 → 写新 embedding | 后台识别条目页 | `identity_embeddings(source=manual)` | **不需要** |
| 角度分类器重训 | 标注积累到一定量 | 研发机 dataset/ | **需要**研发机 |
| YOLO head 补训 | 工具不在 COCO 类时 | 研发机 dataset/ | **需要**研发机 |

常见误解：
- 「再上传一张就能让模型变聪明」 → 只对 *embedding 库* 成立，不会更新模型权重。
- 「人脸库变大要不要重训人脸模型」 → 不用，只动 `identity_embeddings`。

## 6. 数据治理

- 训练数据导出仅 admin 可触发，写 audit_log。
- 训练数据离线存放，研发机训完后销毁副本（可选）。
- 生产中 face crop 与 owner 关联通过 hash 抹去 owner_id 后再导出，避免数据泄露。

## 7. 推荐节奏

- **M2 上线后**：开启识别条目页的「角度标注」按钮，让运维顺手标。
- **M3-M5**：积累 ≥3000 张 face crop。
- **M8**：在研发机训第一版 angle_cls，导出 INT8，上线。
- **后续**：每月或每季度重训一次。

## 8. 我能在这台生产机上训吗？

- **不推荐**。生产机 24G/无 GPU/10C20T，跑训练会拖慢线上推理。
- 如果非要在生产机训：等到夜间停服务窗口，且仅训 mobilenetv3_small 这种小模型。
- 默认部署脚本 *不* 安装训练依赖；研发机自己装 `tools/training/requirements.txt`。
