-- Replace the unconditional UNIQUE on projects.code with a partial unique
-- index that only applies to active (non-archived) projects. This lets us
-- archive a project, then later create a new project that re-uses the
-- archived project's code without hitting a 23505 unique_violation.
--
-- See docs/permissions.md for the project model: archive == soft delete.

ALTER TABLE projects DROP CONSTRAINT IF EXISTS projects_code_key;

CREATE UNIQUE INDEX IF NOT EXISTS projects_code_active_uniq
    ON projects (code)
    WHERE archived_at IS NULL;
