# HTTP API

> 默认 base path: `/api`。除 `/api/auth/login`、`/api/app/latest`、`/healthz`、`/readyz` 外均需 `Authorization: Bearer <jwt>`。

> v2：业务接口全部加 `project_id` 路径前缀，详见 [permissions.md](permissions.md)。

## 0. 约定

- 请求 / 响应默认 `application/json; charset=utf-8`，上传接口除外。
- 时间一律 ISO-8601 带时区。
- 分页参数 `page` (从 1)、`page_size` (默认 20，最大 100)。
- 错误响应统一为：
  ```json
  { "error": { "code": "INVALID_INPUT", "message": "...", "details": {} } }
  ```
- 成功响应直接返回资源体或 `{ "data": ..., "page": ..., "total": ... }`。
- 项目作用域接口路径形如 `/api/projects/{project_id}/{kind}`，权限校验在 axum extractor `RequireProjectPerm(perm)` 中完成。

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
- `POST   /api/users`             `{ username, password, role }`  `role ∈ {admin, member}`
- `PATCH  /api/users/{id}`        `{ role?, enabled?, password? }`
- `DELETE /api/users/{id}`

> 开放注册已关闭，只能由 admin 创建。

## 3. 项目与权限（v2 新增）

### 3.1 项目

| Method | Path | 权限 | 说明 |
|---|---|---|---|
| GET    | `/api/projects`                 | 登录即可（admin 可加 `?all=1`） | 仅返回 `can_view=1` 的项目；带本人权限位 |
| GET    | `/api/projects/{pid}`           | view  | 项目详情 |
| POST   | `/api/projects`                 | admin | `{ code, name, description? }` |
| PATCH  | `/api/projects/{pid}`           | manage / admin | `{ name?, description?, overrides? }` |
| DELETE | `/api/projects/{pid}`           | admin | 软删 + 删数据，需二次确认 |

响应示例（列表）：
```json
{
  "data": [
    { "id": "...", "code": "site-A", "name": "A 站点",
      "my_perms": { "view": true, "upload": true, "delete": false, "manage": false } }
  ]
}
```

### 3.2 成员

| Method | Path | 权限 |
|---|---|---|
| GET    | `/api/projects/{pid}/members`            | view |
| POST   | `/api/projects/{pid}/members`            | manage / admin |
| PATCH  | `/api/projects/{pid}/members/{user_id}`  | manage / admin |
| DELETE | `/api/projects/{pid}/members/{user_id}`  | manage / admin |

POST / PATCH 体：
```json
{ "user_id": "...", "can_view": true, "can_upload": true, "can_delete": false, "can_manage": false }
```

## 4. 工单 / 人员 / 工具 / 设备 CRUD（项目作用域）

同构：`/api/projects/{pid}/work_orders | persons | tools | devices`。

| Method | Path | 权限 |
|---|---|---|
| GET    | `/api/projects/{pid}/{kind}`                            | view |
| GET    | `/api/projects/{pid}/{kind}/{id}`                       | view |
| POST   | `/api/projects/{pid}/{kind}`                            | upload |
| PATCH  | `/api/projects/{pid}/{kind}/{id}`                       | upload |
| DELETE | `/api/projects/{pid}/{kind}/{id}`                       | delete |
| GET    | `/api/projects/{pid}/{kind}/{id}/photos`                | view |
| POST   | `/api/projects/{pid}/{kind}/{id}/photos:bulk_upload`    | upload |

示例：
```http
POST /api/projects/{pid}/persons
{ "name": "张三", "employee_no": "E001" }
```

## 5. 照片

### POST `/api/projects/{pid}/photos`  （权限 upload）
`multipart/form-data`：
- `file` (必填)
- `wo_code` 或 `wo_id` (二选一)
- `owner_type` (必填：`person|tool|device|wo_raw`)
- `owner_id` (可选)
- `angle` (可选：`front|side|back|unknown`)

响应 202：
```json
{ "id": "...", "hash": "...", "thumb_url": "/api/files/thumb/abc.webp", "status": "pending" }
```

### 其他照片接口（均带 `/api/projects/{pid}` 前缀）
- `GET    /api/projects/{pid}/photos`                 列表，参数 `wo`/`owner_type`/`owner_id`/`status`/`q`（view）
- `GET    /api/projects/{pid}/photos/{id}`            详情含 detections（view）
- `PATCH  /api/projects/{pid}/photos/{id}`            `{ angle?, owner_type?, owner_id? }`（upload）
- `DELETE /api/projects/{pid}/photos/{id}`            （delete）
- `GET    /api/files/orig/{hash}.{ext}`               鉴权 + 服务端校验调用方至少在某个项目能访问该 hash
- `GET    /api/files/thumb/{hash}.webp`               同上
- `GET    /api/files/archive/...`                     同上

> 文件下载接口的鉴权：服务端用 `hash` 反查 `photos` 找出 `project_id`，再校验当前用户在该项目的 view 权限；多个项目同 hash 时按用户最先匹配到的项目放行。

## 6. 识别条目（后台人工纠错）

- `GET   /api/projects/{pid}/recognition_items?status=unmatched&page=1`   view
- `GET   /api/projects/{pid}/recognition_items/{id}`                       view
- `POST  /api/projects/{pid}/recognition_items/{id}:resolve`               upload
  - body: `{ "action": "bind|create_and_bind|ignore", "owner_type": "...", "owner_id": "..." }`
- `POST  /api/projects/{pid}/recognition_items/{id}:correct`               upload
- `GET   /api/files/annotated/{hash}.jpg`                                  同照片下载，按 hash 反查项目鉴权

## 7. 后台调参

### 全局（admin）
- `GET   /api/settings`
- `PATCH /api/settings`     `{ "match_threshold": 0.6, "upload_max_mb": 12, ... }`
- `GET   /api/platform`
- `PATCH /api/platform`     `{ "name": "..." }`

### 项目级（manage）
- `GET   /api/projects/{pid}/settings`            合并后的有效配置 = 全局 + projects.overrides
- `PATCH /api/projects/{pid}/settings`            体：要写入 `projects.overrides` 的键值
- `GET   /api/projects/{pid}/recognition_projects`           识别项目列表（人脸 / 工具 / 设备 / 角度等）
- `PATCH /api/projects/{pid}/recognition_projects/{key}`     阈值、是否启用、限定类

## 8. 打包下载

- `POST /api/projects/{pid}/packaging`        权限 view
  ```json
  { "by": "work_order|person|tool|device", "id": "...", "include": ["orig", "archive"] }
  ```
  响应 202：`{ "task_id": "..." }`
- `GET  /api/projects/{pid}/packaging/{task_id}`    进度 + 完成后 zip URL
- `GET  /api/files/zips/{task_id}.zip`              二进制；按 task 反查项目鉴权

## 9. SSE 实时事件

- `GET /api/events?token=<jwt>`
  - 服务端按用户「可见项目集合」过滤事件，永远不会推送非成员项目的内容。
  - 也可加 `?project_id=<pid>` 只订阅一个项目。
  - 事件 payload 都带 `project_id`：
    - `recognition.matched   { project_id, photo_id, detection_id, owner, score }`
    - `recognition.learning  { project_id, ... }`
    - `recognition.unmatched { project_id, ... }`
    - `recognition.failed    { project_id, ... }`
    - `settings.updated      { scope: "global|project", project_id? }`
    - `system.notice`

## 10. APK 版本接口

- `GET    /api/app/latest?channel=stable`         公开
- `GET    /api/app/versions`                       admin
- `POST   /api/app/versions`                       admin
- `DELETE /api/app/versions/{id}`                  admin
- `GET    /api/files/apks/{version}.apk`           登录即可

## 11. 管理辅助

- `GET /api/audit_log?actor=...&project_id=...&action=...&from=...&to=...`   admin 全量；member 仅自己加成员的项目
- `GET /api/projects/{pid}/stats/dashboard`       项目维度的今日上传 / 推理吞吐 / unmatched 积压
- `GET /api/stats/dashboard`                       admin 全局视图

## 12. Android 客户端快路

| 场景 | 接口 |
|---|---|
| 登录 | `POST /api/auth/login` |
| 拿可见项目 | `GET /api/projects` |
| 选项目后查工单 | `GET /api/projects/{pid}/work_orders?q=...` |
| 上传现场照 | `POST /api/projects/{pid}/photos` |
| 检查更新 | `GET /api/app/latest` |

APP 在登录后强制要求选定一个项目，所有后续请求带该 `pid`。

## 13. 错误码一览

| code | 说明 |
|---|---|
| `UNAUTHORIZED`        | 未登录 / token 失效 |
| `FORBIDDEN`           | 全局权限不足（非 admin 调用 admin 专属接口等） |
| `PROJECT_FORBIDDEN`   | 用户不是该项目成员，或缺少所需权限位 |
| `PROJECT_NOT_FOUND`   | 项目不存在或已被删除 |
| `INVALID_INPUT`       | 请求体校验失败 |
| `NOT_FOUND`           | 资源不存在 |
| `CONFLICT`            | 唯一约束 / 状态冲突（如 `(project_id, code)` 重复） |
| `PAYLOAD_TOO_LARGE`   | 超出 upload_max_mb |
| `RATE_LIMITED`        | 调用过于频繁 |
| `MODEL_UNAVAILABLE`   | ONNX 未加载完成 |
| `INTERNAL`            | 未预期错误 |
