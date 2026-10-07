-- M5-002: jejak audit pembersihan retensi. Satu baris per aksi (termasuk dry-run), tanpa isi artifact atau path absolut.
CREATE TABLE retention_actions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now(),
    dry_run boolean NOT NULL,
    kind text NOT NULL CHECK (kind IN ('orphan_artifact','integration_worktree')),
    target text NOT NULL CHECK (btrim(target) <> '' AND length(target) <= 255),
    outcome text NOT NULL CHECK (outcome IN ('deleted','would_delete','failed')),
    detail text CHECK (detail IS NULL OR length(detail) <= 500)
);

CREATE INDEX retention_actions_created_idx ON retention_actions (created_at DESC, id DESC);
