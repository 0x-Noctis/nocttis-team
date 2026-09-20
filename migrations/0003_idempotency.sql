CREATE TABLE idempotency_keys (
    key text PRIMARY KEY CHECK (btrim(key) <> ''),
    request_fingerprint text NOT NULL CHECK (btrim(request_fingerprint) <> ''),
    status_code smallint CHECK (status_code BETWEEN 200 AND 599),
    response_body jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK (
        (status_code IS NULL AND response_body IS NULL AND completed_at IS NULL)
        OR (status_code IS NOT NULL AND response_body IS NOT NULL AND completed_at IS NOT NULL)
    )
);
