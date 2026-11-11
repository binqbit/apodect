CREATE TABLE projects (
    id text PRIMARY KEY,
    deployment jsonb NOT NULL
);

-- These are observed contract states, never a second editable budget ledger.
CREATE TABLE project_views (
    project text PRIMARY KEY REFERENCES projects(id),
    height numeric(20, 0) NOT NULL CHECK (height >= 0),
    block_hash text NOT NULL,
    state jsonb NOT NULL,
    domains jsonb NOT NULL,
    plans jsonb NOT NULL,
    commands jsonb NOT NULL,
    invoices jsonb NOT NULL,
    requests jsonb NOT NULL,
    queue_head jsonb NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE vault_views (
    project text NOT NULL REFERENCES projects(id),
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    height numeric(20, 0) NOT NULL CHECK (height >= 0),
    block_hash text NOT NULL,
    state jsonb NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project, chain)
);

-- A financial operation retains its original signed bytes across process death
-- and ambiguous RPC responses. No automatic replacement transaction is inferred.
CREATE TABLE effects (
    project text NOT NULL REFERENCES projects(id),
    id text NOT NULL,
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    network text NOT NULL,
    intent jsonb NOT NULL,
    attempt integer NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    transaction jsonb,
    state text NOT NULL DEFAULT 'planned'
        CHECK (state IN ('planned', 'prepared', 'submitted', 'confirmed', 'failed', 'attention')),
    result jsonb,
    error text,
    PRIMARY KEY (project, id),
    CHECK (state = 'planned' OR transaction IS NOT NULL)
);

-- Only definitively finished attempts can be retried. Their signed bytes and
-- outcomes remain inspectable after the current effect returns to planned.
CREATE TABLE effect_attempts (
    project text NOT NULL,
    effect text NOT NULL,
    attempt integer NOT NULL,
    transaction jsonb NOT NULL,
    state text NOT NULL CHECK (state IN ('confirmed', 'failed')),
    result jsonb,
    error text,
    PRIMARY KEY (project, effect, attempt),
    FOREIGN KEY (project, effect) REFERENCES effects(project, id)
);

CREATE TABLE checkpoints (
    project text NOT NULL REFERENCES projects(id),
    observation text NOT NULL,
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    selectors jsonb NOT NULL,
    evidence jsonb,
    PRIMARY KEY (project, observation)
);

CREATE TABLE observations (
    project text NOT NULL REFERENCES projects(id),
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    subject text NOT NULL,
    fingerprint text NOT NULL,
    observation text NOT NULL,
    PRIMARY KEY (project, chain, subject)
);

CREATE TABLE funding_quotes (
    project text NOT NULL REFERENCES projects(id),
    route text NOT NULL,
    response jsonb NOT NULL,
    PRIMARY KEY (project, route)
);

CREATE TABLE outbox (
    id uuid PRIMARY KEY,
    sequence bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    project text NOT NULL REFERENCES projects(id),
    kind text NOT NULL,
    payload jsonb NOT NULL
);

CREATE TABLE profiles (
    id uuid PRIMARY KEY,
    display_name text CHECK (display_name IS NULL OR char_length(display_name) BETWEEN 1 AND 100)
);

CREATE TABLE wallets (
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    network text NOT NULL,
    address text NOT NULL,
    profile_id uuid NOT NULL REFERENCES profiles(id),
    near_public_key text,
    CHECK ((chain = 'near') = (near_public_key IS NOT NULL)),
    PRIMARY KEY (chain, network, address)
);
CREATE INDEX wallets_profile ON wallets(profile_id);

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    chain text NOT NULL,
    network text NOT NULL,
    address text NOT NULL,
    proof jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    FOREIGN KEY (chain, network, address) REFERENCES wallets(chain, network, address)
);

CREATE TABLE login_challenges (
    id uuid PRIMARY KEY,
    chain text NOT NULL CHECK (chain IN ('ethereum', 'solana', 'near')),
    network text NOT NULL,
    address text NOT NULL,
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 32),
    message text NOT NULL,
    callback_url text,
    expires_at timestamptz NOT NULL,
    link_session bytea REFERENCES sessions(token_hash) ON DELETE CASCADE
);
