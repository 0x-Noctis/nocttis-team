CREATE TABLE providers (
    id text PRIMARY KEY CHECK (btrim(id) <> ''),
    base_url text NOT NULL CHECK (
        base_url ~ '^https?://[^/@:[:space:]]+([:]([0-9]+))?([/?#][^[:space:]]*)?$'
        AND base_url !~ '^https?://[^/]*@'
    ),
    api_key_env text NOT NULL CHECK (api_key_env ~ '^[A-Za-z_][A-Za-z0-9_]*$'),
    request_timeout_seconds bigint NOT NULL CHECK (request_timeout_seconds > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE models (
    id text PRIMARY KEY CHECK (btrim(id) <> ''),
    provider_id text NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    remote_name text NOT NULL CHECK (btrim(remote_name) <> ''),
    class text NOT NULL CHECK (btrim(class) <> ''),
    context_window bigint NOT NULL CHECK (context_window > 0),
    max_output_tokens bigint NOT NULL CHECK (max_output_tokens > 0),
    claimed_capabilities jsonb NOT NULL DEFAULT
        '{"chat": false, "streaming": false, "tools": false, "parallel_tools": false}',
    verified_capabilities jsonb NOT NULL DEFAULT
        '{"chat": "unknown", "streaming": "unknown", "tools": "unknown", "parallel_tools": "unknown"}',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider_id, id),
    UNIQUE (provider_id, remote_name),
    CHECK (
        jsonb_typeof(claimed_capabilities) = 'object'
        AND claimed_capabilities ?& ARRAY['chat', 'streaming', 'tools', 'parallel_tools']
        AND claimed_capabilities - ARRAY['chat', 'streaming', 'tools', 'parallel_tools']::text[] = '{}'
        AND jsonb_typeof(claimed_capabilities->'chat') = 'boolean'
        AND jsonb_typeof(claimed_capabilities->'streaming') = 'boolean'
        AND jsonb_typeof(claimed_capabilities->'tools') = 'boolean'
        AND jsonb_typeof(claimed_capabilities->'parallel_tools') = 'boolean'
    ),
    CHECK (
        jsonb_typeof(verified_capabilities) = 'object'
        AND verified_capabilities ?& ARRAY['chat', 'streaming', 'tools', 'parallel_tools']
        AND verified_capabilities - ARRAY['chat', 'streaming', 'tools', 'parallel_tools']::text[] = '{}'
        AND verified_capabilities->>'chat' IN ('unknown', 'supported', 'unsupported')
        AND verified_capabilities->>'streaming' IN ('unknown', 'supported', 'unsupported')
        AND verified_capabilities->>'tools' IN ('unknown', 'supported', 'unsupported')
        AND verified_capabilities->>'parallel_tools' IN ('unknown', 'supported', 'unsupported')
    )
);

CREATE TABLE provider_probes (
    id uuid PRIMARY KEY,
    provider_id text NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id text NOT NULL,
    probe_kind text NOT NULL CHECK (probe_kind IN ('chat', 'streaming', 'tools')),
    status text NOT NULL CHECK (status IN ('succeeded', 'failed')),
    capability_status text NOT NULL DEFAULT 'unknown'
        CHECK (capability_status IN ('unknown', 'supported', 'unsupported')),
    latency_ms bigint NOT NULL CHECK (latency_ms >= 0),
    error_code text CHECK (error_code IN (
        'authentication_failed',
        'rate_limited',
        'timeout',
        'invalid_response',
        'provider_unavailable',
        'context_too_large'
    )),
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (provider_id, model_id)
        REFERENCES models(provider_id, id) ON DELETE CASCADE
);

CREATE INDEX provider_probes_provider_created_idx
    ON provider_probes (provider_id, created_at DESC);

CREATE INDEX provider_probes_model_created_idx
    ON provider_probes (model_id, created_at DESC);
