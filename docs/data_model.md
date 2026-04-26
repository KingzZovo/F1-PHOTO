# 数据模型

> PostgreSQL 16 + pgvector 0.7。本文是会随实现迭代的权威 schema 初稿，实际迁移在 `server/migrations/`。

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
CREATE TYPE user_role AS ENUM ('admin', 'operator', 'viewer');
CREATE TYPE embedding_source AS ENUM ('initial', 'incremental', 'manual');
```

## 2. 账号与审计

```sql
CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,                    -- argon2id
    role          user_role NOT NULL DEFAULT 'operator',
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ
);

CREATE TABLE audit_log (
    id        BIGSERIAL PRIMARY KEY,
    actor_id  UUID REFERENCES users(id),
    action    TEXT NOT NULL,
    target    TEXT,
    payload   JSONB,
    ts        TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

## 3. 业务主体

```sql
CREATE TABLE work_orders (
    id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    code       TEXT NOT NULL UNIQUE,
    title      TEXT,
    status     TEXT NOT NULL DEFAULT 'open',
    notes      TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE persons (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name        TEXT NOT NULL,
    employee_no TEXT UNIQUE,
    notes       TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX persons_name_trgm ON persons USING gin (name gin_trgm_ops);

CREATE TABLE tools (
    id        UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name      TEXT NOT NULL,
    sn        TEXT UNIQUE,
    category  TEXT,
    notes     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE devices (
    id        UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name      TEXT NOT NULL,
    sn        TEXT UNIQUE,
    category  TEXT,
    notes     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

## 4. 照片与识别

```sql
CREATE TABLE photos (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    wo_id         UUID REFERENCES work_orders(id) ON DELETE SET NULL,
    owner_type    owner_type NOT NULL,              -- person/tool/device/wo_raw
    owner_id      UUID,                              -- 为空代表“未绑定”
    angle         angle_kind NOT NULL DEFAULT 'unknown',
    path          TEXT NOT NULL,                     -- data/orig/{hash}.{ext}
    archive_path  TEXT,                              -- 归档后的路径
    thumb_path    TEXT NOT NULL,
    hash          TEXT NOT NULL UNIQUE,              -- sha256
    bytes         BIGINT NOT NULL,
    mime          TEXT NOT NULL,
    width         INT,
    height        INT,
    status        photo_status NOT NULL DEFAULT 'pending',
    uploaded_by   UUID REFERENCES users(id),
    uploaded_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at  TIMESTAMPTZ
);
CREATE INDEX photos_wo ON photos(wo_id);
CREATE INDEX photos_owner ON photos(owner_type, owner_id);
CREATE INDEX photos_status ON photos(status);

-- 一张照片可能有多个检测目标
-- detection 的 owner 代表该检测目标被绑定到哪个业务实体
CREATE TABLE detections (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    photo_id        UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    target_type     detect_target NOT NULL,
    bbox            JSONB NOT NULL,                  -- {x,y,w,h}
    score           REAL NOT NULL,
    embedding       vector(512),                     -- pgvector
    angle           angle_kind,                      -- 仅 face
    matched_owner_id UUID,                            -- 查 persons/tools/devices
    matched_score   REAL,
    match_status    match_status NOT NULL DEFAULT 'unmatched',
    reviewed_by     UUID REFERENCES users(id),
    reviewed_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX detections_photo ON detections(photo_id);
CREATE INDEX detections_status ON detections(match_status);

-- 同身份可保留多条 embedding（初始 / 增量 / 人工纠正）
CREATE TABLE identity_embeddings (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    owner_type  owner_type NOT NULL,                 -- person/tool/device
    owner_id    UUID NOT NULL,
    embedding   vector(512) NOT NULL,
    source      embedding_source NOT NULL DEFAULT 'initial',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- 按 owner 检索 + 向量匹配
CREATE INDEX ide_owner ON identity_embeddings(owner_type, owner_id);
-- HNSW 适合在线 kNN
CREATE INDEX ide_embedding_hnsw ON identity_embeddings
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 200);
```

## 5. 识别条目（后台人工纠错面板）

```sql
CREATE TABLE recognition_items (
    id                    UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    detection_id          UUID NOT NULL REFERENCES detections(id) ON DELETE CASCADE,
    photo_id              UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    annotated_image_path  TEXT NOT NULL,            -- 预绘红框后产物
    suggested_owner_id    UUID,
    manual_owner_id       UUID,                     -- 人工选定
    status                match_status NOT NULL,
    notes                 TEXT,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ri_status ON recognition_items(status);
```

## 6. 后台设置与队列

```sql
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- 预置项 (迁移脚本插入)
-- match_threshold 默认 0.62
-- match_low_threshold 默认 0.50
-- upload_max_mb 默认 10
-- platform_name 默认 'F1-Photo'
-- listen_port (仅展示)

-- 轻量队列表
CREATE TABLE recognition_queue (
    id          BIGSERIAL PRIMARY KEY,
    photo_id    UUID NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    enqueued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    attempts    INT NOT NULL DEFAULT 0,
    last_error  TEXT
);
CREATE INDEX rq_pending ON recognition_queue(started_at) WHERE finished_at IS NULL;
-- 出队逻辑用 SELECT ... FOR UPDATE SKIP LOCKED
```

## 7. APK 版本控制

```sql
CREATE TABLE app_versions (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    channel     TEXT NOT NULL DEFAULT 'stable',     -- stable/beta
    version     TEXT NOT NULL,                       -- 1.0.0
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

## 8. 常用查询

### kNN 查找身份
```sql
SELECT owner_type, owner_id, 1 - (embedding <=> $1) AS score
FROM identity_embeddings
WHERE owner_type = $2
ORDER BY embedding <=> $1
LIMIT 5;
```

### 拉取未匹配列表
```sql
SELECT ri.*, p.thumb_path
FROM recognition_items ri
JOIN photos p ON p.id = ri.photo_id
WHERE ri.status = 'unmatched'
ORDER BY ri.updated_at DESC
LIMIT 50 OFFSET $1;
```

### 根据工单拉人员 / 工具 / 设备
```sql
SELECT DISTINCT ON (d.matched_owner_id)
       d.matched_owner_id, d.target_type, d.matched_score, p.thumb_path
FROM detections d
JOIN photos p ON p.id = d.photo_id
WHERE p.wo_id = $1 AND d.match_status = 'matched';
```
