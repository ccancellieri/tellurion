CREATE TABLE IF NOT EXISTS {{schema}}.control_schema (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    version BIGINT NOT NULL
);
INSERT INTO {{schema}}.control_schema (singleton, version)
VALUES (TRUE, 1)
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE IF NOT EXISTS {{schema}}.control_revisions (
    revision BIGINT PRIMARY KEY CHECK (revision > 0),
    snapshot_json JSONB NOT NULL,
    recorded_at_unix_ms BIGINT NOT NULL CHECK (recorded_at_unix_ms >= 0)
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_state (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    current_revision BIGINT NOT NULL REFERENCES {{schema}}.control_revisions(revision)
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_resources (
    resource_key TEXT PRIMARY KEY,
    resource_kind TEXT NOT NULL,
    resource_json JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_role_bindings (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    role TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    binding_json JSONB NOT NULL,
    PRIMARY KEY (issuer, subject, role, scope_key)
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_path_policies (
    policy_id TEXT PRIMARY KEY,
    policy_json JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_tombstones (
    scope_key TEXT PRIMARY KEY,
    scope_json JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_entity_versions (
    resource_key TEXT PRIMARY KEY,
    entity_version TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_audit (
    revision BIGINT PRIMARY KEY REFERENCES {{schema}}.control_revisions(revision),
    actor_json JSONB NOT NULL,
    request_json JSONB NOT NULL,
    idempotency_key TEXT,
    changed_resources_json JSONB NOT NULL,
    recorded_at_unix_ms BIGINT NOT NULL CHECK (recorded_at_unix_ms >= 0),
    applying_instance TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_outbox (
    revision BIGINT NOT NULL REFERENCES {{schema}}.control_revisions(revision),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    changed_resources_json JSONB NOT NULL,
    PRIMARY KEY (revision, ordinal)
);

CREATE TABLE IF NOT EXISTS {{schema}}.control_idempotency (
    idempotency_key TEXT PRIMARY KEY,
    changeset_json JSONB NOT NULL,
    commit_json JSONB NOT NULL
);
