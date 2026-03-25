pub const BOOTSTRAP_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS strategy_runtime_states (
    strategy_id TEXT PRIMARY KEY,
    runtime_state TEXT NOT NULL,
    last_transition_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_states (
    execution_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_plans (
    plan_id TEXT PRIMARY KEY,
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_plan_steps (
    plan_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    step_type TEXT NOT NULL,
    adapter TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    last_error TEXT,
    metadata_json TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (plan_id, step_id),
    FOREIGN KEY(plan_id) REFERENCES execution_plans(plan_id)
);

CREATE TABLE IF NOT EXISTS reconciliation_states (
    execution_id TEXT PRIMARY KEY,
    residual_exposure_usd INTEGER NOT NULL,
    rebalance_required INTEGER NOT NULL CHECK (rebalance_required IN (0, 1)),
    updated_at TEXT NOT NULL,
    FOREIGN KEY(execution_id) REFERENCES execution_states(execution_id)
);

CREATE TABLE IF NOT EXISTS capital_reservations (
    reservation_id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL,
    asset TEXT NOT NULL,
    amount INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('held', 'consumed', 'released')),
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_requests (
    request_id TEXT PRIMARY KEY,
    request_kind TEXT NOT NULL,
    source_agent_id TEXT NOT NULL,
    submitted_at TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    rationale_json TEXT NOT NULL,
    execution_prefs_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS intents (
    intent_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    source_agent_id TEXT NOT NULL,
    intent_type TEXT NOT NULL,
    objective_json TEXT NOT NULL,
    constraints_json TEXT NOT NULL,
    funding_json TEXT NOT NULL,
    post_actions_json TEXT NOT NULL,
    rationale_json TEXT NOT NULL,
    execution_prefs_json TEXT NOT NULL,
    submitted_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(request_id) REFERENCES agent_requests(request_id)
);

CREATE TABLE IF NOT EXISTS event_journal (
    event_id TEXT PRIMARY KEY,
    stream_type TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS strategy_runtime_recovery (
    strategy_id TEXT PRIMARY KEY,
    runtime_state TEXT NOT NULL,
    next_tick_at TEXT,
    last_event_id TEXT,
    metrics_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_control (
    scope_key TEXT PRIMARY KEY,
    control_mode TEXT NOT NULL,
    transition_reason TEXT NOT NULL,
    transition_source TEXT NOT NULL,
    transitioned_at TEXT NOT NULL,
    last_cleared_at TEXT,
    last_cleared_reason TEXT,
    last_cleared_source TEXT,
    last_rejection_code TEXT,
    last_rejection_message TEXT,
    last_rejection_operation TEXT,
    last_rejection_at TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS strategy_watcher_states (
    strategy_id TEXT NOT NULL,
    watcher_key TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    cursor TEXT NOT NULL,
    sampled_at TEXT NOT NULL,
    PRIMARY KEY (strategy_id, watcher_key)
);

CREATE TABLE IF NOT EXISTS strategy_trigger_memory (
    strategy_id TEXT NOT NULL,
    trigger_key TEXT NOT NULL,
    cooldown_until TEXT,
    last_fired_at TEXT,
    hysteresis_armed INTEGER NOT NULL CHECK (hysteresis_armed IN (0, 1)),
    PRIMARY KEY (strategy_id, trigger_key)
);

CREATE TABLE IF NOT EXISTS strategy_pending_hedges (
    strategy_id TEXT PRIMARY KEY,
    venue TEXT NOT NULL,
    instrument TEXT NOT NULL,
    client_order_id TEXT NOT NULL,
    signer_address TEXT NOT NULL DEFAULT '',
    account_address TEXT NOT NULL DEFAULT '',
    order_id INTEGER,
    nonce INTEGER NOT NULL,
    status TEXT NOT NULL,
    last_synced_at TEXT
);

CREATE TABLE IF NOT EXISTS onboarding_workspaces (
    workspace_id TEXT PRIMARY KEY,
    canonical_workspace_root TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS onboarding_installs (
    install_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    install_url TEXT NOT NULL,
    attached_bundle_url TEXT NOT NULL,
    claim_disposition TEXT NOT NULL CHECK (claim_disposition IN ('claimed', 'reopened')),
    bootstrap_source TEXT NOT NULL,
    bootstrap_path TEXT NOT NULL,
    state_db_path TEXT NOT NULL,
    analytics_db_path TEXT NOT NULL,
    used_remote_control_plane INTEGER NOT NULL CHECK (used_remote_control_plane IN (0, 1)),
    recovered_existing_state INTEGER NOT NULL CHECK (recovered_existing_state IN (0, 1)),
    bootstrap_attempt_count INTEGER NOT NULL DEFAULT 0,
    last_bootstrap_attempt_at TEXT,
    last_bootstrap_completed_at TEXT,
    last_bootstrap_failure_stage TEXT,
    last_bootstrap_failure_summary TEXT,
    readiness_status TEXT NOT NULL,
    readiness_blockers_json TEXT NOT NULL,
    readiness_diagnostics_json TEXT NOT NULL,
    onboarding_status TEXT NOT NULL DEFAULT 'blocked',
    bundle_drift_json TEXT,
    last_onboarding_rejection_code TEXT,
    last_onboarding_rejection_message TEXT,
    last_onboarding_rejection_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (workspace_id, install_url),
    FOREIGN KEY(workspace_id) REFERENCES onboarding_workspaces(workspace_id)
);

CREATE TABLE IF NOT EXISTS onboarding_checklist_items (
    install_id TEXT NOT NULL,
    checklist_key TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    blocker_reason TEXT,
    next_action TEXT,
    evidence_json TEXT NOT NULL,
    lifecycle_json TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (install_id, checklist_key),
    FOREIGN KEY(install_id) REFERENCES onboarding_installs(install_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_route_readiness (
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    route_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    status TEXT NOT NULL,
    capital_json TEXT NOT NULL,
    approvals_json TEXT NOT NULL,
    blockers_json TEXT NOT NULL,
    recommended_owner_action_json TEXT,
    ordered_steps_json TEXT NOT NULL DEFAULT '[]',
    current_step_key TEXT,
    last_route_rejection_code TEXT,
    last_route_rejection_message TEXT,
    last_route_rejection_at TEXT,
    evaluation_json TEXT,
    evaluation_fingerprint TEXT,
    stale_status TEXT NOT NULL DEFAULT 'fresh',
    stale_reason TEXT,
    stale_detected_at TEXT,
    evaluated_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (install_id, proposal_id, route_id),
    FOREIGN KEY(install_id) REFERENCES onboarding_installs(install_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_route_readiness_steps (
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    route_id TEXT NOT NULL,
    step_key TEXT NOT NULL,
    status TEXT NOT NULL,
    blocker_reason TEXT,
    recommended_action_json TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (install_id, proposal_id, route_id, step_key),
    FOREIGN KEY(install_id, proposal_id, route_id)
        REFERENCES onboarding_route_readiness(install_id, proposal_id, route_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_strategy_selections (
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    selection_id TEXT PRIMARY KEY,
    selection_revision INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    reopened_from_revision INTEGER,
    proposal_revision INTEGER NOT NULL,
    proposal_uri TEXT NOT NULL,
    proposal_snapshot_json TEXT NOT NULL,
    recommendation_basis_json TEXT NOT NULL,
    readiness_sensitivity_summary_json TEXT NOT NULL,
    approval_json TEXT NOT NULL,
    approval_stale INTEGER NOT NULL DEFAULT 0 CHECK (approval_stale IN (0, 1)),
    approval_stale_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (install_id, proposal_id),
    FOREIGN KEY(install_id) REFERENCES onboarding_installs(install_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_strategy_selection_overrides (
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    selection_id TEXT NOT NULL,
    selection_revision INTEGER NOT NULL,
    override_key TEXT NOT NULL,
    previous_value_json TEXT NOT NULL,
    new_value_json TEXT NOT NULL,
    rationale TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    sensitivity_class TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (selection_id, selection_revision, override_key),
    FOREIGN KEY(selection_id) REFERENCES onboarding_strategy_selections(selection_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_strategy_selection_approval_history (
    history_id INTEGER PRIMARY KEY AUTOINCREMENT,
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    selection_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    selection_revision INTEGER NOT NULL,
    approved_revision INTEGER,
    reopened_from_revision INTEGER,
    approved_by TEXT,
    note TEXT,
    reason TEXT,
    provenance_json TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY(selection_id) REFERENCES onboarding_strategy_selections(selection_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS onboarding_strategy_runtime_handoffs (
    install_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    selection_id TEXT NOT NULL,
    approved_selection_revision INTEGER NOT NULL,
    route_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    route_readiness_fingerprint TEXT NOT NULL,
    route_readiness_status TEXT NOT NULL,
    route_readiness_evaluated_at TEXT NOT NULL,
    eligibility_status TEXT NOT NULL,
    hold_reason TEXT,
    runtime_control_mode TEXT NOT NULL,
    strategy_id TEXT,
    runtime_identity_refreshed_at TEXT,
    runtime_identity_source TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (install_id, proposal_id, selection_id),
    FOREIGN KEY(selection_id) REFERENCES onboarding_strategy_selections(selection_id) ON DELETE CASCADE
);
"#;
