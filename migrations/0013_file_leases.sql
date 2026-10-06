-- M4-002: file lease per attempt.
-- Tabel dibuat di 0001 tetapi belum pernah ditulis kode mana pun. Tambah pemilik (attempt) supaya attempt
-- lama yang masih hidup tidak bisa memperbarui atau melepas lease milik attempt pengganti, plus constraint
-- dan index untuk pencarian per task dan pembersihan lease kedaluwarsa.
ALTER TABLE file_leases
    ADD COLUMN owner uuid NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN acquired_at timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN renewed_at timestamptz NOT NULL DEFAULT now(),
    ADD CONSTRAINT file_leases_pattern_nonempty CHECK (btrim(path_pattern) <> '');
ALTER TABLE file_leases ALTER COLUMN owner DROP DEFAULT;

CREATE INDEX file_leases_task_idx ON file_leases (task_id);
CREATE INDEX file_leases_expiry_idx ON file_leases (project_run_id, expires_at);
