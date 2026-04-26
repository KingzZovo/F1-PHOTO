# 数据模型

> PostgreSQL 16 + pgvector 0.7。本文是会随实现迭代的权威 schema 初稿，实际迁移在 `server/migrations/`。

> v2 引入「项目」层与项目级 RBAC，详见 [permissions.md](permissions.md)。所有业务表都加了 `project_id NOT NULL`。

## 1. 扩展与枚举

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TYPE owner_type AS ENUM ('person', 'tool', 'device', 'wo_raw');
CREATE TYPE photo_status AS ENUM ('pending', 'processing', 'matched', 'unmatched', 'learning', 'failed');
CREATE TYPE detect_target AS ENUM ('face', 'tool', 'device');
CREATE TYPE match_status AS ENUM ('matched', 'learning', 'unmatched', 'manual_corrected');
CREATE TYPE angle_kind AS ENUM ('front', 'side', 'back', 'unknown');
CREATE TYPE user_role AS ENUM ('admin', 'member');
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
    project_id UUID,                                -- 跨项目操作可为空
    action     TEXT NOT NULL,
    target     TEXT,
    payload    JSONB,
    ts         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_project ON audit_log(project_id, ts DESC);
```

## 3. 项目与成员（v2 新增）

```sql
CREATE TABLE projects (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    code        TEXT NOT NULL UNIQUE,              -- 短码，用于归档路径前缀
    name        TEXT NOT NULL,
    description TEXT,
    overrides   JSONB NOT NULL DEFAULT '{}'::jsonb,-- 覆盖全局 settings 的部分键（阈值 / 上传上限等）
    created_by  UUID REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 成员资格 + 4 个独立的权限位
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

初始迁移会插入一个固定 ID 的 `default` 项目并把所有 `admin` 加成员，用于历史数据回填。

## 4. 业务主体（全部 `project_id NOT NULL`）

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
    UNIQUE (project_id, code)
);
CREATE INDEX work_orders_project ON work_orders(project_id);

CREATE TABLE persons (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    employee_no TEXT,
    notes       TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, employee_no)
);
CREATE INDEX persons_project ON persons(project_id);
CREATE INDEX persons_name_trgm ON persons USING gin (name gin_trgm_ops);

CREATE TABLE tools (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    sn         TEXT,
    category   TEXT,
    notes      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, sn)
);
CREATE INDEX tools_project ON tools(project_id);

CREATE TABLE devices (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    sn         TEXT,
    category   TEXT,
    notes      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, sn)
);
CREATE INDEX devices_project ON devices(project_id);
```

> 跨项目同名是允许的：项目 A 的「张三」和项目 B 的「张三」是两条不同记录，互相独立。

## 5. 照片与识别

```sql
CREATE TABLE photos (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id    UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    wo_id         UUID REFERENCES work_orders(id) ON DELETE SET NULL,
    owner_type    owner_type NOT NULL,
    owner_id      UUID,
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
    UNIQUE (project_id, hash)                       -- 同一项目内哈希去重；跨项目允许同图
);
CREATE INDEX photos_project_wo ON photos(project_id, wo_id);
CREATE INDEX photos_project_owner ON photos(project_id, owner_type, owner_id);
CREATE INDEX photos_project_status ON photos(project_id, status);

CREATE TABLE detections (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE, -- denorm，便于过滤
    photo_id        UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    target_type     detect_target NOT NULL,
    bbox            JSONB NOT NULL,
    score           REAL NOT NULL,
    embedding       vector(512),
    angle           angle_kind,
    matched_owner_id UUID,
    matched_score   REAL,
    match_status    match_status NOT NULL DEFAULT 'unmatched',
    reviewed_by     UUID REFERENCES users(id),
    reviewed_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX detections_project ON detections(project_id, match_status);
CREATE INDEX detections_photo ON detections(photo_id);

-- 同身份可保留多条 embedding（初始 / 增量 / 人工纠正）；按项目隔离
CREATE TABLE identity_embeddings (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    owner_type  owner_type NOT NULL,
    owner_id    UUID NOT NULL,
    embedding   vector(512) NOT NULL,
    source      embedding_source NOT NULL DEFAULT 'initial',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ide_project_owner ON identity_embeddings(project_id, owner_type, owner_id);
-- HNSW 全局，但所有 kNN 查询都强制 WHERE project_id = ? AND owner_type = ?
CREATE INDEX ide_embedding_hnsw ON identity_embeddings
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 200);
```

## 6. 识别条目（后台人工纠错面板）

```sql
CREATE TABLE recognition_items (
    id                    UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id            UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    detection_id          UUID NOT NULL REFERENCES detections(id) ON DELETE CASCADE,
    photo_id              UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    annotated_image_path  TEXT NOT NULL,
    suggested_owner_id    UUID,
    manual_owner_id       UUID,
    status                match_status NOT NULL,
    notes                 TEXT,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ri_project_status ON recognition_items(project_id, status);
```

## 7. 后台设置与队列

```sql
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- 预置项 (迁移脚本插入)
-- match_threshold       0.62
-- match_low_threshold   0.50
-- upload_max_mb         10
-- platform_name         'F1-Photo'
-- listen_port           (仅展示)
-- 项目级覆盖：projects.overrides 的同名键覆盖以上默认

-- 轻量队列表；project_id denorm 便于按项目积压统计
CREATE TABLE recognition_queue (
    id          BIGSERIAL PRIMARY KEY,
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    photo_id    UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    enqueued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    attempts    INT NOT NULL DEFAULT 0,
    last_error  TEXT
);
CREATE INDEX rq_pending ON recognition_queue(started_at) WHERE finished_at IS NULL;
CREATE INDEX rq_project ON recognition_queue(project_id);
-- 出队：SELECT ... FOR UPDATE SKIP LOCKED
```

## 8. APK 版本控制

```sql
CREATE TABLE app_versions (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    channel     TEXT NOT NULL DEFAULT 'stable',
    version     TEXT NOT NULL,
    apk_path    TEXT NOT NULL,
    sha256      TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    notes       TEXT,
    is_force    BOOLEAN NOT NULL DEFAULT FALSE,
    released_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(channel, version)
);
CREATE INDEX app_versions_latest ON app_versions(channel, released_at DESC);
```

APK 不分项目；版本接口对所有登录设备公开。

## 9. 常用查询

### kNN 查找身份（强制项目过滤）
```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE project_id = $2
  AND owner_type = $3
ORDER BY embedding <=> $1
LIMIT 5;
```

### 拉取某项目未匹配列表
```sql
SELECT ri.*, p.thumb_path
FROM recognition_items ri
JOIN photos p ON p.id = ri.photo_id
WHERE ri.project_id = $1 AND ri.status = 'unmatched'
ORDER BY ri.updated_at DESC
LIMIT 50 OFFSET $2;
```

### 当前用户能访问的项目列表
```sql
SELECT p.*, pm.can_view, pm.can_upload, pm.can_delete, pm.can_manage
FROM projects p
JOIN project_members pm ON pm.project_id = p.id
WHERE pm.user_id = $1 AND pm.can_view = TRUE
ORDER BY p.name;
```

### 根据工单拉人员 / 工具 / 设备
```sql
SELECT DISTINCT ON (d.matched_owner_id)
       d.matched_owner_id, d.target_type, d.matched_score, p.thumb_path
FROM detections d
JOIN photos p ON p.id = d.photo_id
WHERE p.project_id = $1 AND p.wo_id = $2 AND d.match_status = 'matched';
```
