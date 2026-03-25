use a2ex_daemon::{DaemonConfig, bootstrap_local_runtime};
use a2ex_skill_bundle::{
    BundleLoadOutcome, SkillBundleInterpretation, interpret_bundle_load_outcome,
    load_skill_bundle_from_url,
};
use reqwest::{Client, Url};

use crate::model::{BootstrapReport, BootstrapSource, InstallBootstrapError, InstallReadiness};

pub async fn bootstrap_claimed_workspace(
    workspace_root: &std::path::Path,
) -> Result<BootstrapReport, InstallBootstrapError> {
    let daemon_root = workspace_root.join(".a2ex-daemon");
    let bootstrap = bootstrap_local_runtime(&DaemonConfig::for_data_dir(&daemon_root))
        .await
        .map_err(|error| InstallBootstrapError::DaemonBootstrap {
            message: error.to_string(),
        })?;

    Ok(BootstrapReport {
        source: match bootstrap.source {
            a2ex_daemon::BootstrapSource::LocalRuntime => BootstrapSource::LocalRuntime,
        },
        bootstrap_path: bootstrap.bootstrap_path,
        state_db_path: bootstrap.state_db_path,
        analytics_db_path: bootstrap.analytics_db_path,
        used_remote_control_plane: bootstrap.used_remote_control_plane,
        recovered_existing_state: bootstrap.recovered_existing_state,
    })
}

pub async fn attach_bundle_readiness(
    client: &Client,
    install_url: Url,
) -> Result<
    (
        Url,
        InstallReadiness,
        SkillBundleInterpretation,
        BundleLoadOutcome,
    ),
    InstallBootstrapError,
> {
    let load_outcome = load_skill_bundle_from_url(client, install_url.clone())
        .await
        .map_err(|error| InstallBootstrapError::BundleLoad {
            attached_bundle_url: install_url.clone(),
            message: error.to_string(),
        })?;
    let interpretation = interpret_bundle_load_outcome(&load_outcome).map_err(|error| {
        InstallBootstrapError::BundleInterpretation {
            attached_bundle_url: install_url.clone(),
            message: error.to_string(),
        }
    })?;

    let readiness = readiness_from_interpretation(&load_outcome, &interpretation);
    Ok((
        attached_bundle_identity(&install_url, &load_outcome),
        readiness,
        interpretation,
        load_outcome,
    ))
}

fn readiness_from_interpretation(
    load_outcome: &BundleLoadOutcome,
    interpretation: &SkillBundleInterpretation,
) -> InstallReadiness {
    InstallReadiness {
        status: interpretation.status.clone(),
        blockers: interpretation.blockers.clone(),
        diagnostics: load_outcome.diagnostics.clone(),
    }
}

fn attached_bundle_identity(fallback_url: &Url, load_outcome: &BundleLoadOutcome) -> Url {
    load_outcome
        .bundle
        .as_ref()
        .and_then(|bundle| {
            bundle
                .documents
                .iter()
                .find(|document| document.document_id == bundle.entry_document_id)
                .map(|document| document.source_url.clone())
        })
        .unwrap_or_else(|| fallback_url.clone())
}
