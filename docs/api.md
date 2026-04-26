# HTTP API

> 默认 base path: `/api`。除 `/api/auth/login`、`/api/app/latest`、`/healthz`、`/readyz` 外均需 `Authorization: Bearer <jwt>`。

> v3：人员 / 工具 / 设备为**全局主数据**接口（`/api/persons` 等）；工单 / 照片 / 识别条目为**项目作用域**接口（`/api/projects/{pid}/...`）；admin 通过 `/api/admin/*` 跨项目检索。详见 [permissions.md](permissions.md)。

## 0. 约定

- 请求 / 响应默认 `application/json; charset=utf-8`，上传接口除外。
- 时间一律 ISO-8601 带时区。
- 分页参数 `page` (从 1)、`page_size` (默认 20，最大 100)。
- 错误响应统一为：
  ```json
  { "error": { "code": "INVALID_INPUT", "message": "...", "details": {} } }
  ```
- 成功响应直接返回资源体或 `{ "data": ..., "page": ..., "total": ... }`。
- 主要错误码：
  - `UNAUTHENTICATED` 401
  - `FORBIDDEN` 403（普通账号访问 admin 资源）
  - `PROJECT_FORBIDDEN` 403（项目权限位不足）
  - `NOT_FOUND` 404
  - `INVALID_INPUT` 422
  - `CONFLICT` 409（如 `employee_no` 撞库）

## 1. 健康检查

| Method | Path | 说明 |
|---|---|---|
| GET | `/healthz` | 活体，返 200 `{ ok: true }` |
| GET | `/readyz`  | 可用性：DB + 模型加载 |

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
- `POST   /api/users`             `{ username, password, role }` `role ∈ {admin, member}`
- `PATCH  /api/users/{id}`        `{ role?, enabled?, password? }`
- `DELETE /api/users/{id}`

> 开放注册已关闭，只能由 admin 创建账号。

## 3. 项目与成员

### 3.1 项目 CRUD

| Method | Path | 权限 | 说明 |
|---|---|---|---|
| GET    | `/api/projects`             | 登录即可 | 仅返回当前用户 `can_view=1` 的项目；admin 返全部 |
| GET    | `/api/projects/{pid}`       | view     | 项目详情 |
| POST   | `/api/projects`             | admin    | `{ code, name, description? }` |
| PATCH  | `/api/projects/{pid}`       | manage 或 admin | `{ name?, description?, overrides? }` |
| DELETE | `/api/projects/{pid}`       | admin    | 软删 + 级联删数据；二次确认 |

响应示例（列表）：
```json
{
  "data": [
    { "id": "...", "code": "site-A", "name": "A 站点",
      "my_perms": { "view": true, "upload": true, "delete": false, "manage": false } }
  ]
}
```

### 3.2 成员 CRUD

| Method | Path | 权限 |
|---|---|---|
| GET    | `/api/projects/{pid}/members`            | view |
| POST   | `/api/projects/{pid}/members`            | manage 或 admin |
| PATCH  | `/api/projects/{pid}/members/{user_id}`  | manage 或 admin |
| DELETE | `/api/projects/{pid}/members/{user_id}`  | manage 或 admin |

POST / PATCH 体：
```json
{ "user_id": "...", "can_view": true, "can_upload": true, "can_delete": false, "can_manage": false }
```

## 4. 全局主数据：人员 / 工具 / 设备

> **全局接口**，不带 `project_id`。写仅限 admin；读所有登录账号可用（用于上传时填员工号 / 识别后展示姓名）。

### 4.1 人员 `/api/persons`

| Method | Path | 权限 |
|---|---|---|
| GET    | `/api/persons?q=&page=&page_size=` | 登录即可 |
| GET    | `/api/persons/{id}`                | 登录即可 |
| POST   | `/api/persons`                     | admin |
| PATCH  | `/api/persons/{id}`                | admin |
| DELETE | `/api/persons/{id}`                | admin |

POST 体：
```json
{ "employee_no": "E001", "name": "张三", "notes": "" }
```
冲突：`employee_no` 已存在 → 409 `CONFLICT { existing_id }`。

### 4.2 工具 `/api/tools`、设备 `/api/devices`

同构。POST 体：
```json
{ "sn": "T-AAA-001", "name": "扭力扳手", "category": "扳手", "notes": "" }
```

### 4.3 「快速建档」批量上传

- `POST /api/persons/{id}/photos:bulk_register`  权限 admin
- `POST /api/tools/{id}/photos:bulk_register`    权限 admin
- `POST /api/devices/{id}/photos:bulk_register`  权限 admin

上传多张同一实体照 → 后端跑检测 / embedding → 直接写入全局 `identity_embeddings`（不绑定具体 project，`source_project = NULL`）。

## 5. 工单 `/api/projects/{pid}/work_orders`

| Method | Path | 权限 |
|---|---|---|
| GET    | `/api/projects/{pid}/work_orders?q=&from=&to=&status=`  | view |
| GET    | `/api/projects/{pid}/work_orders/{id}`                  | view |
| POST   | `/api/projects/{pid}/work_orders`                       | upload |
| PATCH  | `/api/projects/{pid}/work_orders/{id}`                  | upload |
| DELETE | `/api/projects/{pid}/work_orders/{id}`                  | delete |
| GET    | `/api/projects/{pid}/work_orders/{id}/photos`           | view |

## 6. 照片 `/api/projects/{pid}/photos`

### POST `/api/projects/{pid}/photos`  （权限 upload）
`multipart/form-data`：
- `file` (必填)
- `wo_code` 或 `wo_id` (二选一)
- `owner_type` (必填：`person|tool|device|wo_raw`)
- `owner_id` (可选；不填则进入识别流程)
- `employee_no` 或 `sn` (可选；当 `owner_type=person` 且 `owner_id` 缺省，可用员工号反查全局主数据)
- `angle` (可选：`front|side|back|unknown`)

响应 202：
```json
{ "id": "...", "hash": "...", "thumb_url": "/api/files/thumb/abc.webp", "status": "pending" }
```

语义补充：
- 上传时仅引用全局主数据，不创建主数据。
- 如填了 `employee_no` / `sn` 但全局无对应记录：
  - 若全局 `upload.allow_auto_create_person=true`：自动建一条 `name='待补全'` 的主数据并绑定。
  - 否则返 422 `INVALID_INPUT { code: "unknown_employee_no" }`，提示联系 admin 建档。

### 其他照片接口
- `GET    /api/projects/{pid}/photos?wo=&owner_type=&owner_id=&status=&q=`  view
- `GET    /api/projects/{pid}/photos/{id}`                                  view
- `PATCH  /api/projects/{pid}/photos/{id}`   `{ angle?, owner_type?, owner_id? }`  upload
- `DELETE /api/projects/{pid}/photos/{id}`   delete

### 文件下载（鉴权）
- `GET /api/files/orig/{hash}.{ext}`     hash 反查 `photos.project_id` 后校验 view
- `GET /api/files/thumb/{hash}.webp`     同上
- `GET /api/files/archive/...`           同上
- `GET /api/files/annotated/{hash}.jpg`  同上

admin 跨项目可读任意文件，无需指定项目。

## 7. 识别条目（人工纠错）

- `GET   /api/projects/{pid}/recognition_items?status=unmatched&page=1`         view
- `GET   /api/projects/{pid}/recognition_items/{id}`                             view
- `POST  /api/projects/{pid}/recognition_items/{id}:resolve`                     upload
  - body: `{ "action": "bind|create_and_bind|ignore", "owner_type": "...", "owner_id": "...", "employee_no": "...", "name": "..." }`
  - `action=bind`：绑定到指定全局 owner_id（任意项目都可）。
  - `action=create_and_bind`：仅 `admin` 可用，创建全局主数据并绑定（普通账号若需新建联系管理员）。
  - `action=ignore`：标记 `manual_corrected` 但不绑定。
- `POST  /api/projects/{pid}/recognition_items/{id}:correct`                     upload
  - 已绑定后纠正到另一全局 owner，写一条 manual embedding。

## 8. 后台调参

### 8.1 全局（admin）
- `GET   /api/settings`
- `PATCH /api/settings`     `{ "match.threshold": 0.6, "upload.max_mb": 12, ... }`
- `GET   /api/platform`
- `PATCH /api/platform`     `{ "name": "..." }`

### 8.2 项目级（manage 或 admin）
- `GET   /api/projects/{pid}/settings`            合并后的有效配置 = 全局 + projects.overrides
- `PATCH /api/projects/{pid}/settings`            体：要写入 `projects.overrides` 的键值，传 `null` 表示清除覆盖

## 9. 打包下载

- `POST /api/projects/{pid}/packaging`        权限 view
  ```json
  { "by": "work_order|person|tool|device", "id": "...", "include": ["orig", "archive"] }
  ```
  - `by=person` 时 `id` 是全局 `persons.id`，但只打包**本项目**内此员工的照片。admin 可改用 `/api/admin/...` 跨项目打包（见 §11）。
  - 响应 202：`{ "task_id": "..." }`
- `GET  /api/projects/{pid}/packaging/{task_id}`    进度 + 完成后 zip URL
- `GET  /api/files/zips/{task_id}.zip`              二进制；按 task 反查项目鉴权

## 10. SSE 实时事件

- `GET /api/events?token=<jwt>`
  - 服务端按用户「可见项目集合」过滤事件；admin 默认收所有项目事件，可加 `?project_id=<pid>` 缩小订阅。
  - 事件 payload 都带 `project_id`：
    - `recognition.matched   { project_id, photo_id, detection_id, owner_type, owner_id, score }`
    - `recognition.learning  { project_id, ... }`
    - `recognition.unmatched { project_id, ... }`
    - `recognition.failed    { project_id, ... }`
    - `settings.updated      { scope: "global|project", project_id? }`
    - `system.notice`

## 11. 管理员全局检索（仅 admin）

| Method | Path | 说明 |
|---|---|---|
| GET | `/api/admin/work_orders?project_id=&q=&from=&to=`             | 跨项目工单检索 |
| GET | `/api/admin/photos?project_id=&hash=&owner_type=&owner_id=&status=&from=&to=` | 跨项目照片检索 |
| GET | `/api/admin/recognition_items?project_id=&status=&from=&to=`  | 跨项目识别条目 |
| GET | `/api/admin/persons/{id}/photos?project_id=`                  | 此员工出现过的所有照片（带项目信息）|
| GET | `/api/admin/tools/{id}/photos?project_id=`                    | 同上 |
| GET | `/api/admin/devices/{id}/photos?project_id=`                  | 同上 |
| GET | `/api/admin/audit_log?project_id=&actor=&action=&from=&to=`   | 跨项目审计 |
| GET | `/api/admin/stats/dashboard`                                  | 全局汇总仪表盘 |
| POST| `/api/admin/packaging`                                        | 跨项目打包；body 同 §9，但可不带 project_id |

非 admin 调用 → 403 `FORBIDDEN`。

## 12. APK 版本接口

- `GET    /api/app/latest?channel=stable`         公开
- `GET    /api/app/versions`                       admin
- `POST   /api/app/versions`                       admin（`multipart`：apk + meta）
- `DELETE /api/app/versions/{id}`                  admin
- `GET    /api/files/apks/{version}.apk`           登录即可

## 13. 项目级仪表盘

- `GET /api/projects/{pid}/stats/dashboard`       view ｜ 项目维度的今日上传 / 推理吞吐 / unmatched 积压

## 14. Android 客户端快路

| 场景 | 接口 |
|---|---|
| 登录 | `POST /api/auth/login` |
| 拿可见项目 | `GET /api/projects` |
| 拍照上传 | `POST /api/projects/{pid}/photos` |
| 工单查询 | `GET /api/projects/{pid}/work_orders` |
| 识别结果 | `GET /api/projects/{pid}/recognition_items` |
| 主数据查询（填员工号） | `GET /api/persons?q=张三` |
| 版本检查 | `GET /api/app/latest` |
