-- Milestone #5-skel: prepare for #7 detector retrain.
--
-- 1. model_versions: audit trail of detector ONNX models. The active
--    model is the row with `kind='object_detect'` and the latest
--    `promoted_at`; earlier rows are history kept for rollback / audit.
--    Columns:
--      * sha256:               file hash of the promoted ONNX (matches
--                              the dist tarball's models/<kind>.onnx)
--      * file_size:            bytes
--      * corrections_consumed: count of v_training_corrections rows
--                              folded into this version's training set
--      * eval_deltas:          JSON shadow-eval report
--                              (P/R deltas vs the previous version)
--      * promoted_by:          user who ran `f1photo retrain-detector
--                              --promote` (NULL for cron)
--
-- 2. v_training_corrections: read-only view that joins manual
--    recognition corrections to detection bboxes + photo paths.
--    Milestone #7's `f1photo retrain-detector` CLI reads this view to
--    materialise YOLO training tuples (image path + bbox + class label).

CREATE TABLE model_versions (
    id                   bigserial PRIMARY KEY,
    kind                 text NOT NULL DEFAULT 'object_detect',
    sha256               text NOT NULL,
    file_size            bigint,
    corrections_consumed integer,
    eval_deltas          jsonb,
    promoted_at          timestamptz NOT NULL DEFAULT now(),
    promoted_by          uuid REFERENCES users(id) ON DELETE SET NULL,
    notes                text
);

CREATE INDEX model_versions_kind_promoted_at_idx
    ON model_versions(kind, promoted_at DESC);

CREATE OR REPLACE VIEW v_training_corrections AS
SELECT
    ri.id                         AS recognition_item_id,
    ri.project_id,
    ri.photo_id,
    ri.detection_id,
    ri.corrected_owner_type::text AS corrected_owner_type,
    ri.corrected_owner_id,
    ri.corrected_at,
    d.bbox,
    d.score                       AS detection_score,
    d.target_type::text           AS target_type,
    p.path                        AS photo_path,
    p.hash                        AS photo_hash,
    p.width                       AS photo_width,
    p.height                      AS photo_height
FROM recognition_items ri
JOIN detections d ON d.id = ri.detection_id
JOIN photos     p ON p.id = ri.photo_id
WHERE ri.status            = 'manual_corrected'
  AND ri.corrected_at      IS NOT NULL
  AND ri.corrected_owner_id IS NOT NULL
  AND ri.detection_id      IS NOT NULL;
