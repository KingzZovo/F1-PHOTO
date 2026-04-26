# 权限与项目

## 1. 总览

业务层级：

```
项目 (Project)               ← 仅作访问控制 + 组织维度
  └── 工单 (Work Order)
        └── 关联的 人员 / 工具 / 设备  ← 全局主数据，跨项目共享
              └── 照片
```

**核心原则：**

1. **项目不是数据隔离边界**，只是「**谁能看哪些工单 / 照片，能在里头做什么**」的访问控制单元。
2. **人员 / 工具 / 设备是全局主数据**，不属于具体项目。一个员工跨工单、跨项目重复出现是正常被支持的场景。
3. **特征值库 (`identity_embeddings`) 全局共享**：识别匹配跨项目工作；同一员工在任意项目里被拍到都会自动识别为同一人。
4. **管理员 (admin) 拥有全局视角**，可以跨项目检索 / 操作所有工单、照片、识别条目、主数据。
5. **普通账号 (member) 只能看到被加进的项目**，未加入的项目完全不可见。

## 2. 角色模型

### 2.1 全局账号角色

| 角色 | 说明 |
|---|---|
| `admin`  | 超级管理员。跨项目检索/操作所有工单、照片、识别条目；维护全局人员/工具/设备主数据；管理用户、APK、全局设置。 |
| `member` | 普通账号。只能访问被加进的项目；可读取全局主数据（用于绑定 / 查看识别结果对应的姓名）。 |

### 2.2 项目内权限位

每条 `project_members(project_id, user_id, ...)` 记录含 4 个独立的布尔权限位：

| 权限位 | 含义 |
|---|---|
| `can_view`   | 看到此项目内的工单、照片、识别条目。 |
| `can_upload` | 在此项目内上传照片、创建/修改工单、对识别条目做绑定 / 纠错（含「在工单上引用现有员工号 / SN」）。 |
| `can_delete` | 删除此项目内的工单、照片、识别条目。 |
| `can_manage` | 改项目元数据；增删成员；调本项目级阈值 overrides。 |

约束：

- 4 个权限位**独立**，可任意组合。
- 全部为 0 等价于「不是成员」，不应入库。
- `can_manage` 不自动包含 `can_delete`：可以管成员但不能直接删数据。
- 项目内权限**不允许写全局主数据**（人员 / 工具 / 设备的增删改）；那是 admin 专属。

## 3. 全局主数据：人员 / 工具 / 设备

### 3.1 唯一标定

| 实体 | 唯一字段 | 说明 |
|---|---|---|
| 人员 person | `employee_no` | 员工号全局唯一。`employee_no + name` 是这个人的完整标定。 |
| 工具 tool | `sn` | 序列号全局唯一。 |
| 设备 device | `sn` | 设备 SN 全局唯一。 |

- **同名允许**：两个员工都叫「张三」+ 不同 `employee_no` 视为不同人；同 `employee_no` 必同人。
- **跨工单 / 跨项目同员工是常态**：识别会自动复用其全局特征值。

### 3.2 谁可以维护？

- **创建 / 编辑 / 删除**：仅 `admin`。避免普通账号造重复 / 错员工号污染主数据。
- **读取（列表 / 检索）**：所有登录账号都可读，用于上传时填员工号、识别后展示姓名。
- **「上传时自动建档」可选开关**（全局 settings `upload.allow_auto_create_person`，默认关闭）：开启后 `can_upload` 用户填新员工号时后端自动 `INSERT` 一条 `name='待补全'` 的人员，事后 admin 补名字。默认关闭以保证主数据清洁。

## 4. 哪些表按项目划分

**带 `project_id` 的表（按项目划分）：**

- `work_orders`
- `photos`
- `recognition_items`
- `recognition_queue`
- `detections`（denorm `project_id`，便于按项目筛）
- `audit_log`（可选 `project_id`，全局操作时为 NULL）

**全局表（不带 `project_id`）：**

- `users`、`projects`、`project_members`
- `persons`、`tools`、`devices`（主数据）
- `identity_embeddings`（特征值库，跨项目共享）
- `app_versions`、`settings`

## 5. 唯一约束总览

- `users.username` UNIQUE
- `persons.employee_no` UNIQUE（全局）
- `tools.sn` UNIQUE（全局）
- `devices.sn` UNIQUE（全局）
- `projects.code` UNIQUE
- `project_members (project_id, user_id)` PRIMARY KEY
- `work_orders (project_id, code)` UNIQUE — 工单号在项目内唯一，跨项目允许同号
- `photos (project_id, hash)` UNIQUE — 项目内同图去重；同一物理图在不同项目可以独立存档

## 6. 鉴权流程

```mermaid
flowchart TD
    A[请求带 JWT] --> B[解析 user_id / role]
    B --> C{路由分类}
    C -->|/api/auth/* /healthz /readyz /api/projects 列表| G[登录即可]
    C -->|/api/persons|tools|devices 读| RR[登录即可]
    C -->|/api/persons|tools|devices 写| AdminOnly{role = admin?}
    C -->|/api/admin/*| AdminOnly
    C -->|/api/projects/pid/*| Scope{role = admin?}
    Scope -->|是| P[放行]
    Scope -->|否| M[查 project_members 对应权限位]
    M -->|权限位 = 1| P
    M -->|权限位 = 0 或 非成员| F[403 PROJECT_FORBIDDEN]
    AdminOnly -->|是| P
    AdminOnly -->|否| F2[403 FORBIDDEN]
    C -->|/api/files/*| FH[hash → photos.project_id\n再校验 view 权限]
```

实现：axum extractor `RequireProjectPerm(perm)`，`RequireAdmin`。admin 在 `RequireProjectPerm` 中直接 bypass。

## 7. 识别匹配的作用域

**识别匹配是全局的，不按项目分割。**

```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE owner_type = $2
ORDER BY embedding <=> $1
LIMIT 5;
```

效果（即你提的需求）：

- 员工 E001「张三」在项目 A 的工单里被多次拍照训练特征值。
- 同一员工到项目 B 上传新照片 → 自动识别为 E001 张三，绑定 `photos.owner_id` 指向全局 `persons.id`。
- 增量学习也写进全局 `identity_embeddings`（带 `source_photo` / `source_project` 用于追溯，但不参与 kNN 过滤）。
- 人工纠错可以绑定**任意**全局 person/tool/device，不受项目限制。
- 照片本身的可见性仍受项目控制：项目 B 的成员若没加项目 A，看不见项目 A 里的张三照片，但可以在「人员主数据」里查到 E001 张三这个人。

## 8. 管理员的全局检索

新增 `/api/admin/*` 命名空间，仅 admin 可访问，不带项目过滤（可选 `?project_id=` 缩小范围）：

| 接口 | 说明 |
|---|---|
| `GET /api/admin/work_orders` | 跨项目工单检索；过滤 `q` / `project_id` / `owner` / `from` / `to` |
| `GET /api/admin/photos` | 跨项目照片检索；过滤 `q` / `hash` / `owner_type+owner_id` / `project_id` / `status` / `date` |
| `GET /api/admin/recognition_items` | 跨项目识别条目（unmatched / manual_corrected 等）|
| `GET /api/admin/persons/{id}/photos` | 此员工在所有项目内出现过的照片汇总 |
| `GET /api/admin/tools/{id}/photos` | 同上 |
| `GET /api/admin/devices/{id}/photos` | 同上 |
| `GET /api/admin/stats/dashboard` | 全局汇总仪表盘 |
| `GET /api/admin/audit_log` | 跨项目审计 |

普通账号调用 `/api/admin/*` → 403 `FORBIDDEN`。

## 9. 文件归档与下载

- 归档路径仍以 `project_code` 开头：`data/archive/{project_code}/{wo_code_prefix3}/{YYYYMM}/{wo_code}_{owner_name}_{angle}_{seq:03}.{ext}`。
  - 这是「物理组织」+「打包下载效率」考虑，跟数据隔离无关。
  - 同一员工的照片散布在多个 `{project_code}/` 目录下属正常。
- 文件下载接口：`/api/files/orig/{hash}.{ext}` 等，服务端用 hash 反查 `photos` 找到 `project_id`，再校验当前用户在该项目的 `view` 权限；admin 直接放行。

## 10. UI 影响

- 顶栏「项目切换器」：member 必选，列表来自 `GET /api/projects`；admin 多一个「**所有项目（全局）**」选项，进入 admin 控制台。
- 主数据页（**人员 / 工具 / 设备**）：
  - admin：可增删改 + 看到这个员工 / 工具 / 设备出现过的所有项目和照片。
  - member：只读列表 + 检索，无编辑按钮。
- 识别条目人工纠错：选 owner 时是**全局**搜索 person/tool/device，不限项目。
- 无项目权限的 member 登录后展示空状态 + 联系管理员提示。

## 11. 升级与回填

M1 初始迁移：

```sql
-- 默认项目（用于历史数据回填和首次安装）
INSERT INTO projects (id, code, name, description)
VALUES ('00000000-0000-0000-0000-000000000001', 'default', '默认项目', '系统初始化项目');

-- admin 自动加入 default 项目（4 权限全开）
INSERT INTO project_members (project_id, user_id, can_view, can_upload, can_delete, can_manage)
SELECT '00000000-0000-0000-0000-000000000001', id, TRUE, TRUE, TRUE, TRUE
FROM users WHERE role = 'admin';
```

后续不再需要任何「按项目复制特征值」之类的兜底操作。

## 12. 决策记录

- **ADR-006**（修订）：引入「项目」层，仅作**访问控制 + 组织维度**，**不**做数据物理隔离；admin 跨项目可达。
- **ADR-007**（反转）：人员 / 工具 / 设备 + `identity_embeddings` **全局共享**。`employee_no` / `sn` 全局唯一即标定；跨项目同身份**自动识别**为期望行为。
- **ADR-008**：弃用 `operator / viewer` 全局角色；项目内 4 个布尔权限位。
- **ADR-009**：全局主数据写入仅限 `admin`；普通账号只读，可选「上传时自动建档」开关由 admin 在全局 settings 中切换。
- **ADR-010**：`/api/admin/*` 命名空间专用于 admin 全局检索 / 跨项目操作；普通账号 403。
