mod support;

use std::path::Path;

use a2ex_onboarding::{
    ClaimDisposition, InstallBootstrapError, InstallBootstrapRequest, bootstrap_install,
};
use reqwest::Url;
use rusqlite::Connection;
use tempfile::tempdir;

fn analytics_bootstrap_event_count(path: &Path) -> i64 {
    let connection =
        Connection::open(path).expect("analytics db should open for bootstrap evidence checks");
    connection
        .query_row("SELECT COUNT(*) FROM daemon_bootstrap_events", [], |row| {
            row.get(0)
        })
        .expect("daemon bootstrap events table should exist in analytics.db")
}

fn onboarding_install_count(path: &Path) -> i64 {
    let connection = Connection::open(path).expect("state db should open for install count checks");
    connection
        .query_row("SELECT COUNT(*) FROM onboarding_installs", [], |row| {
            row.get(0)
        })
        .expect("onboarding_installs table should exist in state.db")
}

#[tokio::test]
async fn explicit_workspace_claim_persists_identity_and_reopens_same_url() {
    let workspace_root = tempdir().expect("workspace tempdir");
    let install_url = Url::parse("http://127.0.0.1:65530/install/official-bundle/skill.md")
        .expect("install url parses");

    let first = bootstrap_install(InstallBootstrapRequest {
        install_url: install_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("bootstrap entrypoint should return structured claim output");

    assert_eq!(
        first.claim_disposition,
        ClaimDisposition::Claimed,
        "first bootstrap must report claim_disposition=claimed for a never-before-seen workspace root"
    );
    assert_eq!(
        first.claimed_workspace_root,
        workspace_root.path(),
        "bootstrap must echo the claimed workspace root instead of silently deriving a different location"
    );
    assert!(
        !first.workspace_id.is_empty(),
        "bootstrap must return a durable workspace_id on first claim"
    );
    assert!(
        !first.install_id.is_empty(),
        "bootstrap must return a durable install_id on first claim"
    );
    assert!(
        first.bootstrap.state_db_path.exists(),
        "bootstrap must create the claimed workspace state.db on first claim"
    );
    assert!(
        first.bootstrap.analytics_db_path.exists(),
        "bootstrap must create the claimed workspace analytics.db on first claim"
    );
    assert!(
        !first.bootstrap.used_remote_control_plane,
        "local install bootstrap must prove local authority with used_remote_control_plane=false"
    );

    let second = bootstrap_install(InstallBootstrapRequest {
        install_url: install_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect(
        "same install URL should reopen the claimed workspace instead of failing as a new install",
    );

    assert_eq!(
        second.claim_disposition,
        ClaimDisposition::Reopened,
        "same install URL plus same workspace root must reopen instead of claiming a duplicate install"
    );
    assert_eq!(
        second.workspace_id, first.workspace_id,
        "reopen path must keep the canonical workspace_id stable"
    );
    assert_eq!(
        second.install_id, first.install_id,
        "reopen path must keep the canonical install_id stable"
    );
    assert!(
        second.bootstrap.recovered_existing_state,
        "reopen path must report recovered_existing_state=true after the first bootstrap created local runtime state"
    );
    assert_eq!(
        onboarding_install_count(&first.bootstrap.state_db_path),
        1,
        "same install URL reopen must reuse the persisted install row instead of creating a duplicate local install"
    );
}

#[tokio::test]
async fn mismatch_rejection_happens_before_workspace_state_mutates() {
    let workspace_root = tempdir().expect("workspace tempdir");
    let install_url = Url::parse("http://127.0.0.1:65530/install/official-bundle/skill.md")
        .expect("install url parses");

    let initial = bootstrap_install(InstallBootstrapRequest {
        install_url: install_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("initial claim should succeed before mismatch coverage runs");

    assert!(
        initial.bootstrap.analytics_db_path.exists(),
        "first claim must create analytics.db before mismatch protection can prove no extra bootstrap evidence was written"
    );
    let bootstrap_events_before =
        analytics_bootstrap_event_count(&initial.bootstrap.analytics_db_path);

    let mismatch = bootstrap_install(InstallBootstrapRequest {
        install_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some("workspace-does-not-match".to_owned()),
        expected_install_id: Some(initial.install_id.clone()),
    })
    .await;

    match mismatch {
        Err(InstallBootstrapError::WorkspaceIdentityMismatch { .. }) => {}
        other => panic!(
            "workspace identity mismatches must return WorkspaceIdentityMismatch before mutating persisted install state, got {other:?}"
        ),
    }

    let bootstrap_events_after =
        analytics_bootstrap_event_count(&initial.bootstrap.analytics_db_path);
    assert_eq!(
        bootstrap_events_after, bootstrap_events_before,
        "mismatch rejection must not append daemon bootstrap evidence after the canonical workspace/install identity check fails"
    );
}

#[tokio::test]
async fn repeated_bootstrap_reuses_claimed_workspace_runtime_path_idempotently() {
    let workspace_root = tempdir().expect("workspace tempdir");
    let install_url = Url::parse("http://127.0.0.1:65530/install/official-bundle/skill.md")
        .expect("install url parses");

    let first = bootstrap_install(InstallBootstrapRequest {
        install_url: install_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("first bootstrap should produce structured bootstrap paths");
    let second = bootstrap_install(InstallBootstrapRequest {
        install_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect("second bootstrap should reuse the same claimed runtime");

    assert_eq!(
        first.bootstrap.state_db_path,
        workspace_root.path().join(".a2ex-daemon/state.db"),
        "bootstrap must place state.db under the explicit claimed workspace root"
    );
    assert_eq!(
        second.bootstrap.state_db_path, first.bootstrap.state_db_path,
        "reopen path must reuse the same state.db path rather than deriving a second local runtime"
    );
    assert_eq!(
        second.bootstrap.analytics_db_path, first.bootstrap.analytics_db_path,
        "reopen path must reuse the same analytics.db path rather than deriving a second local runtime"
    );
    assert!(
        second.bootstrap.analytics_db_path.exists(),
        "bootstrap must create analytics.db in the claimed workspace before idempotent bootstrap evidence can be inspected"
    );
    assert_eq!(
        analytics_bootstrap_event_count(&second.bootstrap.analytics_db_path),
        2,
        "repeated bootstrap should leave durable idempotent evidence by appending a second daemon bootstrap event in the claimed analytics.db"
    );
}
