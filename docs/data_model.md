# 数据模型

> PostgreSQL 16 + pgvector 0.7。本文是会随实现迭代的权威 schema 初稿，实际迁移在 `server/migrations/`。

> v3：项目仅作**访问控制 + 组织维度**。人员 / 工具 / 设备 + 特征值库为**全局主数据**，跨项目共享。详见 [permissions.md](permissions.md)。

## 1. 扩展与枚举

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TYPE owner_type       AS ENUM ('person', 'tool', 'device', 'wo_raw');
CREATE TYPE photo_status     AS ENUM ('pending', 'processing', 'matched', 'unmatched', 'learning', 'failed');
CREATE TYPE detect_target    AS ENUM ('face', 'tool', 'device');
CREATE TYPE match_status     AS ENUM ('matched', 'learning', 'unmatched', 'manual_corrected');
CREATE TYPE angle_kind       AS ENUM ('front', 'side', 'back', 'unknown');
CREATE TYPE user_role        AS ENUM ('admin', 'member');
CREATE TYPE embedding_source AS ENUM ('initial', 'incremental', 'manual');
```

## 2. 账号与审计

```sql
CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,                    -- argon2id
    role          user_role NOT NULL DEFAULT 'member',
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ
);

CREATE TABLE audit_log (
    id         BIGSERIAL PRIMARY KEY,
    actor_id   UUID REFERENCES users(id),
    project_id UUID,                                -- 全局操作可为 NULL（如改主数据 / 改全局 settings）
    action     TEXT NOT NULL,
    target     TEXT,
    payload    JSONB,
    ts         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_project ON audit_log(project_id, ts DESC);
CREATE INDEX audit_log_actor   ON audit_log(actor_id, ts DESC);
```

## 3. 项目与成员

```sql
CREATE TABLE projects (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    code        TEXT NOT NULL UNIQUE,              -- 短码，用于归档路径前缀
    name        TEXT NOT NULL,
    description TEXT,
    overrides   JSONB NOT NULL DEFAULT '{}'::jsonb,-- 覆盖全局 settings 的部分键
    created_by  UUID REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_members (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    can_view   BOOLEAN NOT NULL DEFAULT TRUE,
    can_upload BOOLEAN NOT NULL DEFAULT FALSE,
    can_delete BOOLEAN NOT NULL DEFAULT FALSE,
    can_manage BOOLEAN NOT NULL DEFAULT FALSE,
    granted_by UUID REFERENCES users(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX pm_user ON project_members(user_id);
```

初始迁移建一个固定 ID 的 `default` 项目，把所有 admin 加成员（4 位全开），用于历史数据回填。

## 4. 全局主数据：人员 / 工具 / 设备

> 这三张表**不带** `project_id`。一个员工 / 一把工具 / 一台设备就是一条记录，跨工单、跨项目都是同一行，识别会自动复用同一份特征值。

```sql
CREATE TABLE persons (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    employee_no TEXT NOT NULL UNIQUE,           -- 全局唯一
    name        TEXT NOT NULL,                  -- 同名允许（不同 employee_no 即不同人）
    notes       TEXT,
    created_by  UUID REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX persons_name_trgm ON persons USING gin (name gin_trgm_ops);

CREATE TABLE tools (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    sn         TEXT NOT NULL UNIQUE,            -- 序列号全局唯一
    name       TEXT NOT NULL,
    category   TEXT,
    notes      TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX tools_name_trgm ON tools USING gin (name gin_trgm_ops);

CREATE TABLE devices (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    sn         TEXT NOT NULL UNIQUE,            -- 设备 SN 全局唯一
    name       TEXT NOT NULL,
    category   TEXT,
    notes      TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX devices_name_trgm ON devices USING gin (name gin_trgm_ops);
```

维护权限：仅 `admin`（写）、所有登录账号（读）。详见 [permissions.md §3](permissions.md)。

## 5. 业务表（按项目划分）

```sql
CREATE TABLE work_orders (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    code       TEXT NOT NULL,
    title      TEXT,
    status     TEXT NOT NULL DEFAULT 'open',
    notes      TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, code)                    -- 工单号项目内唯一，跨项目允许同号
);
CREATE INDEX work_orders_project ON work_orders(project_id);

CREATE TABLE photos (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id    UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    wo_id         UUID REFERENCES work_orders(id) ON DELETE SET NULL,
    owner_type    owner_type NOT NULL,
    owner_id      UUID,                          -- 指向全局 persons/tools/devices.id
    angle         angle_kind NOT NULL DEFAULT 'unknown',
    path          TEXT NOT NULL,
    archive_path  TEXT,
    thumb_path    TEXT NOT NULL,
    hash          TEXT NOT NULL,
    bytes         BIGINT NOT NULL,
    mime          TEXT NOT NULL,
    width         INT,
    height        INT,
    status        photo_status NOT NULL DEFAULT 'pending',
    uploaded_by   UUID REFERENCES users(id),
    uploaded_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at  TIMESTAMPTZ,
    UNIQUE (project_id, hash)                    -- 项目内哈希去重；跨项目允许同图
);
CREATE INDEX photos_project_wo     ON photos(project_id, wo_id);
CREATE INDEX photos_project_owner  ON photos(project_id, owner_type, owner_id);
CREATE INDEX photos_project_status ON photos(project_id, status);
CREATE INDEX photos_owner_global   ON photos(owner_type, owner_id);  -- admin 跨项目「某人/某工具的所有照片」
CREATE INDEX photos_hash_global    ON photos(hash);                  -- 文件下载接口反查

CREATE TABLE detections (
    id               UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id       UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE, -- denorm，便于按项目筛
    photo_id         UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    target_type      detect_target NOT NULL,
    bbox             JSONB NOT NULL,
    score            REAL NOT NULL,
    embedding        vector(512),
    angle            angle_kind,
    matched_owner_id UUID,                       -- 全局 persons/tools/devices.id
    matched_score    REAL,
    match_status     match_status NOT NULL DEFAULT 'unmatched',
    reviewed_by      UUID REFERENCES users(id),
    reviewed_at      TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX detections_project ON detections(project_id, match_status);
CREATE INDEX detections_photo   ON detections(photo_id);

CREATE TABLE recognition_items (
    id                   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id           UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    photo_id             UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    detection_id         UUID REFERENCES detections(id) ON DELETE SET NULL,
    target_type          detect_target NOT NULL,
    suggested_owner_type owner_type,
    suggested_owner_id   UUID,                   -- 全局主数据 id
    suggested_score      REAL,
    status               match_status NOT NULL DEFAULT 'unmatched',
    resolved_by          UUID REFERENCES users(id),
    resolved_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ri_project_status ON recognition_items(project_id, status);
CREATE INDEX ri_photo          ON recognition_items(photo_id);

CREATE TABLE recognition_queue (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE, -- denorm
    photo_id   UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    attempts   INT NOT NULL DEFAULT 0,
    last_error TEXT,
    locked_at  TIMESTAMPTZ,
    locked_by  TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX rq_pending ON recognition_queue(created_at) WHERE locked_at IS NULL;
```

## 6. 全局特征值库

> **不带 `project_id`**。同一员工 / 工具 / 设备的所有 embedding（不论照片来自哪个项目）都聚合在这里。kNN 跨项目工作。

```sql
CREATE TABLE identity_embeddings (
    id             UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    owner_type     owner_type NOT NULL,
    owner_id       UUID NOT NULL,                -- 指向全局 persons/tools/devices.id
    embedding      vector(512) NOT NULL,
    source         embedding_source NOT NULL DEFAULT 'initial',
    source_photo   UUID REFERENCES photos(id)    ON DELETE SET NULL, -- 来源照片，用于追溯（不参与 kNN 过滤）
    source_project UUID REFERENCES projects(id)  ON DELETE SET NULL, -- denorm，仅供审计
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ide_owner ON identity_embeddings(owner_type, owner_id);
CREATE INDEX ide_embedding_hnsw ON identity_embeddings
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 200);
```

kNN 查询模板（**不带** `project_id` 过滤）：

```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE owner_type = $2
ORDER BY embedding <=> $1
LIMIT 5;
```

## 7. 全局设置与 APK 版本

```sql
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      JSONB NOT NULL,
    updated_by UUID REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE app_versions (
    id           UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    version_code INT NOT NULL,                   -- Android versionCode
    version_name TEXT NOT NULL UNIQUE,
    channel      TEXT NOT NULL DEFAULT 'stable',
    apk_path     TEXT NOT NULL,
    apk_bytes    BIGINT NOT NULL,
    apk_sha256   TEXT NOT NULL,
    notes        TEXT,
    is_latest    BOOLEAN NOT NULL DEFAULT FALSE,
    published_by UUID REFERENCES users(id),
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (channel, version_code)
);
CREATE INDEX app_versions_latest ON app_versions(channel, is_latest);
```

默认 settings 键：

| key | value | 说明 |
|---|---|---|
| `platform.name` | `"F1-Photo"` | 平台名称 |
| `match.threshold` | `0.62` | 匹配阈值 |
| `match.low_threshold` | `0.50` | 学习阈值下限 |
| `match.augment_upper` | `0.95` | 增量学习上限（[threshold, upper) 区间额外存一条） |
| `upload.max_mb` | `10` | 单文件上传上限 MB |
| `upload.allow_auto_create_person` | `false` | 上传填新员工号是否自动建主数据条目 |
| `recognition.face.enabled` | `true` | 启用人脸识别 |
| `recognition.tool.enabled` | `true` | 启用工具识别 |
| `recognition.device.enabled` | `true` | 启用设备识别 |
| `archive.naming` | 见下 | 归档命名模板 |

项目级覆盖通过 `projects.overrides JSONB` 存（同 key 即覆盖全局值）。

## 8. 常用查询示例

### 8.1 项目内可见照片列表（member）

```sql
SELECT p.id, p.hash, p.owner_type, p.owner_id, p.status, p.uploaded_at
FROM photos p
WHERE p.project_id = $1
ORDER BY p.uploaded_at DESC
LIMIT 50;
```

### 8.2 全局 kNN（识别匹配，跨项目）

```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE owner_type = $2
ORDER BY embedding <=> $1
LIMIT 5;
```

### 8.3 admin 跨项目检索照片

```sql
SELECT p.*, pj.code AS project_code, pj.name AS project_name
FROM photos p JOIN projects pj ON pj.id = p.project_id
WHERE ($1::uuid IS NULL OR p.project_id = $1)
  AND ($2::owner_type IS NULL OR p.owner_type = $2)
  AND ($3::uuid IS NULL OR p.owner_id = $3)
ORDER BY p.uploaded_at DESC
LIMIT 100;
```

### 8.4 某员工跨项目所有照片

```sql
SELECT p.*, pj.code AS project_code, pj.name AS project_name
FROM photos p
JOIN projects pj ON pj.id = p.project_id
WHERE p.owner_type = 'person' AND p.owner_id = $1
ORDER BY p.uploaded_at DESC;
```

### 8.5 当前用户可见项目集合

```sql
-- admin 看全部；member 看 can_view=TRUE 的项目
SELECT pj.id, pj.code, pj.name
FROM projects pj
WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = $1 AND u.role = 'admin')
   OR EXISTS (
      SELECT 1 FROM project_members pm
      WHERE pm.project_id = pj.id AND pm.user_id = $1 AND pm.can_view = TRUE
   );
```

### 8.6 hash 反查照片所属项目（文件下载鉴权用）

```sql
SELECT project_id
FROM photos
WHERE hash = $1
LIMIT 1;
-- 跨项目同 hash 时按当前用户能 view 的第一条放行；admin 直接放行任意一条
```

## 9. M1 初始迁移关键步骤

```sql
-- 1) 默认项目
INSERT INTO projects (id, code, name, description)
VALUES ('00000000-0000-0000-0000-000000000001', 'default', '默认项目', '系统初始化项目');

-- 2) admin 加入默认项目（4 权限位全开）
INSERT INTO project_members (project_id, user_id, can_view, can_upload, can_delete, can_manage)
SELECT '00000000-0000-0000-0000-000000000001', id, TRUE, TRUE, TRUE, TRUE
FROM users WHERE role = 'admin';

-- 3) 全局 settings 默认键
INSERT INTO settings (key, value) VALUES
  ('platform.name',                    '"F1-Photo"'::jsonb),
  ('match.threshold',                  '0.62'::jsonb),
  ('match.low_threshold',              '0.50'::jsonb),
  ('match.augment_upper',              '0.95'::jsonb),
  ('upload.max_mb',                    '10'::jsonb),
  ('upload.allow_auto_create_person',  'false'::jsonb),
  ('recognition.face.enabled',         'true'::jsonb),
  ('recognition.tool.enabled',         'true'::jsonb),
  ('recognition.device.enabled',       'true'::jsonb)
ON CONFLICT (key) DO NOTHING;
```
