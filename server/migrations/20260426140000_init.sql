-- F1-Photo initial schema (v3)
--
-- Layout principle:
--   * GLOBAL master data: users, projects, project_members, persons, tools,
--     devices, identity_embeddings, settings, app_versions.
--   * PROJECT-SCOPED operational data: work_orders, photos, detections,
--     recognition_items, recognition_queue.
--   * audit_log spans both (project_id nullable).

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto; -- gen_random_uuid()

-- =====================================================================
-- Enums
-- =====================================================================
CREATE TYPE owner_type AS ENUM ('person', 'tool', 'device', 'wo_raw');
CREATE TYPE photo_status AS ENUM ('pending', 'processing', 'matched', 'unmatched', 'learning', 'failed');
CREATE TYPE detect_target AS ENUM ('face', 'tool', 'device');
CREATE TYPE match_status AS ENUM ('matched', 'learning', 'unmatched', 'manual_corrected');
CREATE TYPE angle_kind AS ENUM ('front', 'side', 'back', 'unknown');
CREATE TYPE user_role AS ENUM ('admin', 'member');
CREATE TYPE embedding_source AS ENUM ('initial', 'incremental', 'manual');

-- =====================================================================
-- Global tables
-- =====================================================================

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username      text UNIQUE NOT NULL,
    password_hash text NOT NULL,
    role          user_role NOT NULL DEFAULT 'member',
    full_name     text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    disabled_at   timestamptz
);

CREATE TABLE projects (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    code        text UNIQUE NOT NULL,
    name        text NOT NULL,
    icon        text,
    description text,
    overrides   jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);

CREATE TABLE project_members (
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    can_view   boolean NOT NULL DEFAULT true,
    can_upload boolean NOT NULL DEFAULT false,
    can_delete boolean NOT NULL DEFAULT false,
    can_manage boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON project_members(user_id);

-- Master data: persons / tools / devices
-- employee_no / sn are GLOBALLY UNIQUE; same name is allowed.
CREATE TABLE persons (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    employee_no text UNIQUE NOT NULL,
    name        text NOT NULL,
    department  text,
    phone       text,
    photo_count integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    deleted_at  timestamptz
);
CREATE INDEX persons_name_idx ON persons(name);

CREATE TABLE tools (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sn          text UNIQUE NOT NULL,
    name        text NOT NULL,
    category    text,
    photo_count integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    deleted_at  timestamptz
);
CREATE INDEX tools_name_idx ON tools(name);

CREATE TABLE devices (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sn          text UNIQUE NOT NULL,
    name        text NOT NULL,
    model       text,
    photo_count integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    deleted_at  timestamptz
);
CREATE INDEX devices_name_idx ON devices(name);

-- Global embeddings; kNN filters only on owner_type, never project_id.
CREATE TABLE identity_embeddings (
    id             bigserial PRIMARY KEY,
    owner_type     owner_type NOT NULL,
    owner_id       uuid NOT NULL,
    embedding      vector(512) NOT NULL,
    source         embedding_source NOT NULL,
    source_photo   uuid,
    source_project uuid,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX identity_embeddings_owner_idx ON identity_embeddings(owner_type, owner_id);
CREATE INDEX identity_embeddings_hnsw_idx
    ON identity_embeddings USING hnsw (embedding vector_cosine_ops);

CREATE TABLE settings (
    key        text PRIMARY KEY,
    value      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid REFERENCES users(id) ON DELETE SET NULL
);

CREATE TABLE app_versions (
    id           bigserial PRIMARY KEY,
    channel      text NOT NULL,
    version_code integer NOT NULL,
    version_name text NOT NULL,
    file_path    text NOT NULL,
    sha256       text NOT NULL,
    notes        text,
    mandatory    boolean NOT NULL DEFAULT false,
    published_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(channel, version_code)
);

-- =====================================================================
-- Project-scoped tables
-- =====================================================================

CREATE TABLE work_orders (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    code       text NOT NULL,
    title      text,
    status     text NOT NULL DEFAULT 'open',
    created_by uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(project_id, code)
);
CREATE INDEX work_orders_project_idx ON work_orders(project_id);
CREATE INDEX work_orders_code_idx ON work_orders(code);

CREATE TABLE photos (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id     uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_order_id  uuid REFERENCES work_orders(id) ON DELETE SET NULL,
    owner_type     owner_type,
    owner_id       uuid,
    hash           text NOT NULL,
    path           text NOT NULL,
    thumb_path     text,
    archive_path   text,
    annotated_path text,
    angle          angle_kind NOT NULL DEFAULT 'unknown',
    width          integer,
    height         integer,
    bytes          bigint,
    status         photo_status NOT NULL DEFAULT 'pending',
    exif           jsonb NOT NULL DEFAULT '{}'::jsonb,
    uploaded_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE(project_id, hash)
);
CREATE INDEX photos_project_wo_idx ON photos(project_id, work_order_id);
CREATE INDEX photos_owner_idx ON photos(owner_type, owner_id);
CREATE INDEX photos_status_idx ON photos(status);

CREATE TABLE detections (
    id                 bigserial PRIMARY KEY,
    project_id         uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    photo_id           uuid NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    target_type        detect_target NOT NULL,
    bbox               jsonb NOT NULL,
    score              real NOT NULL,
    embedding          vector(512),
    angle              angle_kind NOT NULL DEFAULT 'unknown',
    match_status       match_status NOT NULL DEFAULT 'unmatched',
    matched_owner_type owner_type,
    matched_owner_id   uuid,
    matched_score      real,
    created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX detections_photo_idx ON detections(photo_id);
CREATE INDEX detections_project_idx ON detections(project_id);

CREATE TABLE recognition_items (
    id                   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id           uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    photo_id             uuid NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    detection_id         bigint REFERENCES detections(id) ON DELETE SET NULL,
    status               match_status NOT NULL,
    suggested_owner_type owner_type,
    suggested_owner_id   uuid,
    suggested_score      real,
    corrected_owner_type owner_type,
    corrected_owner_id   uuid,
    corrected_by         uuid REFERENCES users(id) ON DELETE SET NULL,
    corrected_at         timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX recognition_items_project_status_idx
    ON recognition_items(project_id, status);
CREATE INDEX recognition_items_photo_idx ON recognition_items(photo_id);

CREATE TABLE recognition_queue (
    id           bigserial PRIMARY KEY,
    project_id   uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    photo_id     uuid NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    attempts     integer NOT NULL DEFAULT 0,
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX recognition_queue_pickup_idx
    ON recognition_queue(locked_until NULLS FIRST, created_at);

-- =====================================================================
-- Audit log (project_id nullable for global actions)
-- =====================================================================
CREATE TABLE audit_log (
    id          bigserial PRIMARY KEY,
    user_id     uuid REFERENCES users(id) ON DELETE SET NULL,
    project_id  uuid REFERENCES projects(id) ON DELETE SET NULL,
    action      text NOT NULL,
    target_type text NOT NULL,
    target_id   text,
    before      jsonb,
    after       jsonb,
    ip          inet,
    ua          text,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_project_time_idx ON audit_log(project_id, created_at DESC);
CREATE INDEX audit_log_user_time_idx ON audit_log(user_id, created_at DESC);

-- =====================================================================
-- Seeds: default project + default settings
-- =====================================================================

INSERT INTO projects (id, code, name, description)
VALUES (
    '00000000-0000-0000-0000-000000000001'::uuid,
    'default',
    '默认项目',
    '系统默认项目，可在后台修改或删除（删除前先迁移工单）'
)
ON CONFLICT (code) DO NOTHING;

INSERT INTO settings (key, value) VALUES
    ('platform.name', '"F1-Photo"'::jsonb),
    ('match.threshold', '0.62'::jsonb),
    ('match.low_threshold', '0.50'::jsonb),
    ('match.augment_upper', '0.95'::jsonb),
    ('upload.max_mb', '10'::jsonb),
    ('upload.allow_auto_create_person', 'false'::jsonb),
    ('recognition.face.enabled', 'true'::jsonb),
    ('recognition.tool.enabled', 'true'::jsonb),
    ('recognition.device.enabled', 'true'::jsonb),
    ('recognition.angle.enabled', 'false'::jsonb)
ON CONFLICT (key) DO NOTHING;
