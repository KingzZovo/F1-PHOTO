# HTTP API

> 默认 base path: `/api`。除 `/api/auth/login`、`/api/app/latest`、`/healthz`、`/readyz` 外均需 `Authorization: Bearer <jwt>`。

## 0. 约定

- 请求 / 响应默认 `application/json; charset=utf-8`，上传接口除外。
- 时间一律 ISO-8601 带时区。
- 分页参数 `page` (从 1)、`page_size` (默认 20，最大 100)。
- 错误响应统一为：
  ```json
  { "error": { "code": "INVALID_INPUT", "message": "...", "details": {} } }
  ```
- 成功响应直接返回资源体或 `{ "data": ..., "page": ..., "total": ... }`。

## 1. 健康检查

| Method | Path | 说明 |
|---|---|---|
| GET | `/healthz` | 活体，返 200 `{ ok: true }` |
| GET | `/readyz` | 可用性：DB + 模型加载 |

## 2. 鉴权

### POST `/api/auth/login`
请求：`{ "username": "admin", "password": "..." }`
响应：
```json
{
  "token": "<jwt>",
  "expires_at": "2026-04-27T04:42:53Z",
  "user": { "id": "...", "username": "admin", "role": "admin" }
}
```

### POST `/api/auth/logout`
作废 token（在服务端黑名单到过期）。

### POST `/api/auth/change_password`
`{ "old": "...", "new": "..." }`

### 管理员账号（admin）
- `GET    /api/users`
- `POST   /api/users`             `{ username, password, role }`
- `PATCH  /api/users/{id}`        `{ role?, enabled?, password? }`
- `DELETE /api/users/{id}`

> 开放注册已关闭，只能由 admin 创建。

## 3. 工单 / 人员 / 工具 / 设备 CRUD

同构：`/api/work_orders | /api/persons | /api/tools | /api/devices`。

| Method | Path | 说明 |
|---|---|---|
| GET    | `/api/{kind}`                   | 列表 + 搜索（`q`、分页） |
| GET    | `/api/{kind}/{id}`              | 详情 |
| POST   | `/api/{kind}`                   | 创建 |
| PATCH  | `/api/{kind}/{id}`              | 修改 |
| DELETE | `/api/{kind}/{id}`              | 删除（软删 / 需手动释放照片） |
| GET    | `/api/{kind}/{id}/photos`       | 该实体所有照片 |
| POST   | `/api/{kind}/{id}/photos:bulk_upload` | 快速建库（多张同人/同工具） |

示例：
```http
POST /api/persons
{ "name": "张三", "employee_no": "E001" }
```

## 4. 照片

### POST `/api/photos`
`multipart/form-data`。表单字段：
- `file` (必填)
- `wo_code` (可选)、`wo_id` (可选，二选一)
- `owner_type` (必填：`person|tool|device|wo_raw`)
- `owner_id` (可选，进阶手动绑定)
- `angle` (可选：`front|side|back|unknown`)

响应 202：
```json
{
  "id": "...",
  "hash": "...",
  "thumb_url": "/api/files/thumb/abc.webp",
  "status": "pending"
}
```

### 其他接口
- `GET /api/photos`                        列表，参数：`wo`, `owner_type`, `owner_id`, `status`, `q`
- `GET /api/photos/{id}`                   详情包含 detection、归档路径
- `PATCH /api/photos/{id}`                 `{ angle?, owner_type?, owner_id? }`
- `DELETE /api/photos/{id}`
- `GET /api/files/orig/{hash}.{ext}`        原图（鉴权 + Range）
- `GET /api/files/thumb/{hash}.webp`       缩略图
- `GET /api/files/archive/...`             归档后路径

## 5. 识别条目（后台人工纠错）

- `GET   /api/recognition_items?status=unmatched&page=1`
- `GET   /api/recognition_items/{id}`              含红框预览图 URL
- `POST  /api/recognition_items/{id}:resolve`     `{ "action": "bind", "owner_type": "person", "owner_id": "..." }`
  - action 可选：`bind` 绑定现有；`create_and_bind` 新建后绑（携 `payload`）；`ignore` 忽略。
- `POST  /api/recognition_items/{id}:correct`     修改 matched 项的 owner（同上，类似但用于已 matched）。
- `GET   /api/files/annotated/{hash}.jpg`         红框预绘图

## 6. 后台调参

- `GET   /api/settings`
- `PATCH /api/settings`  体：`{ "match_threshold": 0.6, "upload_max_mb": 12 }`
- `GET   /api/settings/recognition_projects`              识别项目列表（人脸 / 工具 / 设备 / 角度 / YOLO类号限定等）
- `PATCH /api/settings/recognition_projects/{key}`        含阈值、是否启用、限定类、其它参数
- `GET   /api/platform`                                   平台名 / 版本 / 端口
- `PATCH /api/platform`                                   `{ "name": "..." }`

## 7. 打包下载

- `POST /api/packaging`
  ```json
  {
    "by": "work_order|person|tool|device",
    "id": "...",
    "include": ["orig", "archive"]
  }
  ```
  响应 202：`{ "task_id": "..." }`
- `GET  /api/packaging/{task_id}`         进度 + 完成后 zip URL
- `GET  /api/files/zips/{task_id}.zip`    下载二进制

## 8. SSE 实时事件

- `GET /api/events?token=<jwt>`           连接后会发送下列事件：
  - `recognition.matched`        `{ photo_id, detection_id, owner, score }`
  - `recognition.learning`
  - `recognition.unmatched`
  - `recognition.failed`
  - `settings.updated`
  - `system.notice`
- 上量限制：每连接 50 events/s，服务端遭限后发送 `system.throttle`。

## 9. APK 版本接口

- `GET  /api/app/latest?channel=stable`           公开接口，供 APK 检查更新
  ```json
  { "version": "1.0.0", "apk_url": "/api/files/apks/1.0.0.apk", "sha256": "...", "size": 12345, "is_force": false, "notes": "...", "released_at": "..." }
  ```
- `GET  /api/app/versions`                         后台列表
- `POST /api/app/versions`                         multipart上传 APK
- `DELETE /api/app/versions/{id}`
- `GET  /api/files/apks/{version}.apk`            下载设备 APK

## 10. 管理辅助

- `GET /api/audit_log?actor=...&action=...&from=...&to=...`
- `GET /api/stats/dashboard` 返回今日上传量、识别吞吐、unmatched 积压、分类饱和度。

## 11. Android 客户端快路

| 场景 | 接口 |
|---|---|
| 登录 | `POST /api/auth/login` |
| 查工单 | `GET /api/work_orders?q=...` |
| 上传现场照 | `POST /api/photos` |
| 查该工单人员/工具 | `GET /api/work_orders/{id}/summary` (M5 加) |
| 检查更新 | `GET /api/app/latest` |

## 12. 错误码一览

| code | 说明 |
|---|---|
| `UNAUTHORIZED` | 未登录 / token 失效 |
| `FORBIDDEN` | 权限不足 |
| `INVALID_INPUT` | 请求体校验失败 |
| `NOT_FOUND` | 资源不存在 |
| `CONFLICT` | 唯一约束 / 状态冲突 |
| `PAYLOAD_TOO_LARGE` | 超出 upload_max_mb |
| `RATE_LIMITED` | 调用过于频繁 |
| `MODEL_UNAVAILABLE` | ONNX 未加载完成 |
| `INTERNAL` | 未预期错误 |
