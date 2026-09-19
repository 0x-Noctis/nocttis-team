CREATE TABLE providers (
    id uuid PRIMARY KEY,
    name text NOT NULL UNIQUE CHECK (btrim(name) <> ''),
    base_url text NOT NULL CHECK (base_url ~ '^https?://'),
    api_key_env text NOT NULL CHECK (api_key_env ~ '^[A-Za-z_][A-Za-z0-9_]*$'),
    request_timeout_seconds integer NOT NULL CHECK (request_timeout_seconds > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE models (
    id uuid PRIMARY KEY,
    provider_id uuid NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    name text NOT NULL CHECK (btrim(name) <> ''),
    remote_name text NOT NULL CHECK (btrim(remote_name) <> ''),
    class text NOT NULL CHECK (btrim(class) <> ''),
    context_window integer NOT NULL CHECK (context_window > 0),
    max_output_tokens integer NOT NULL CHECK (max_output_tokens > 0),
    claimed_capabilities jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(claimed_capabilities) = 'object'),
    verified_capabilities jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(verified_capabilities) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider_id, id),
    UNIQUE (provider_id, remote_name)
);

CREATE TABLE provider_probes (
    id uuid PRIMARY KEY,
    provider_id uuid NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id uuid NOT NULL,
    probe_type text NOT NULL CHECK (btrim(probe_type) <> ''),
    status text NOT NULL CHECK (btrim(status) <> ''),
    verified_capabilities jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(verified_capabilities) = 'object'),
    latency_ms integer CHECK (latency_ms >= 0),
    error_code text,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (provider_id, model_id)
        REFERENCES models(provider_id, id) ON DELETE CASCADE
);

CREATE INDEX provider_probes_provider_created_idx
    ON provider_probes (provider_id, created_at DESC);

CREATE INDEX provider_probes_model_created_idx
    ON provider_probes (model_id, created_at DESC);
