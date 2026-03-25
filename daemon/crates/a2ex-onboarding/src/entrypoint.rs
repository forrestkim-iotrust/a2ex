use reqwest::Client;

use crate::bootstrap::{attach_bundle_readiness, bootstrap_claimed_workspace};
use crate::model::{
    ApplyStrategySelectionOverrideRequest, ApproveStrategySelectionRequest,
    GuidedOnboardingActionRequest, GuidedOnboardingActionResult, GuidedOnboardingError,
    GuidedOnboardingInspection, GuidedOnboardingInspectionRequest, GuidedOnboardingState,
    GuidedOnboardingStateRequest, InspectStrategyReportWindowRequest,
    InspectStrategyRuntimeRequest, InspectStrategySelectionRequest, InstallBootstrapError,
    InstallBootstrapRequest, InstallBootstrapResult, MaterializeStrategySelectionRequest,
    ReopenStrategySelectionRequest, RouteReadinessActionRequest, RouteReadinessActionResult,
    RouteReadinessError, RouteReadinessEvaluationRequest, RouteReadinessInspectionRequest,
    RouteReadinessRecord, StrategyExceptionRollup, StrategyOperatorReport, StrategyReportWindow,
    StrategyRuntimeHandoffError, StrategyRuntimeHandoffRecord, StrategyRuntimeMonitoringSummary,
    StrategySelectionError, StrategySelectionInspection, StrategySelectionRecord,
};
use crate::store::OnboardingStore;

pub async fn bootstrap_install(
    request: InstallBootstrapRequest,
) -> Result<InstallBootstrapResult, InstallBootstrapError> {
    let state_db_path = request.workspace_root.join(".a2ex-daemon/state.db");
    let store = OnboardingStore::open(&state_db_path).await?;
    let claim = store
        .claim_workspace_install(
            &request.workspace_root,
            &request.install_url,
            request.expected_workspace_id.as_deref(),
            request.expected_install_id.as_deref(),
        )
        .await?;

    let bootstrap = bootstrap_claimed_workspace(&claim.canonical_workspace_root).await?;
    let (attached_bundle_url, readiness, interpretation, load_outcome) =
        attach_bundle_readiness(&Client::new(), claim.attached_bundle_url.clone()).await?;
    let persisted = store
        .persist_bootstrap_success(&claim, &bootstrap, &attached_bundle_url, &readiness)
        .await?;
    let onboarding = store
        .persist_interpreted_onboarding(&persisted.install_id, &interpretation, &load_outcome)
        .await?;
    let inspection = store
        .inspect_guided_onboarding(&persisted.install_id)
        .await
        .map_err(|error| InstallBootstrapError::Persistence {
            message: error.to_string(),
        })?;

    Ok(InstallBootstrapResult {
        workspace_id: persisted.workspace_id,
        install_id: persisted.install_id,
        claim_disposition: persisted.claim_disposition,
        claimed_workspace_root: persisted.canonical_workspace_root,
        attached_bundle_url: persisted.attached_bundle_url,
        bootstrap,
        onboarding,
        readiness: persisted.readiness,
        proposal_handoff: inspection.proposal_handoff,
    })
}

pub async fn read_guided_onboarding(
    request: GuidedOnboardingStateRequest,
) -> Result<GuidedOnboardingState, GuidedOnboardingError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| GuidedOnboardingError::Persistence {
            message: error.to_string(),
        })?;
    store.read_guided_onboarding(&request.install_id).await
}

pub async fn inspect_guided_onboarding(
    request: GuidedOnboardingInspectionRequest,
) -> Result<GuidedOnboardingInspection, GuidedOnboardingError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| GuidedOnboardingError::Persistence {
            message: error.to_string(),
        })?;
    store.inspect_guided_onboarding(&request.install_id).await
}

pub async fn apply_guided_onboarding_action(
    request: GuidedOnboardingActionRequest,
) -> Result<GuidedOnboardingActionResult, GuidedOnboardingError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| GuidedOnboardingError::Persistence {
            message: error.to_string(),
        })?;
    store
        .apply_guided_onboarding_action(&request.install_id, request.action)
        .await
}

pub async fn evaluate_route_readiness(
    request: RouteReadinessEvaluationRequest,
) -> Result<RouteReadinessRecord, RouteReadinessError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| RouteReadinessError::Persistence {
            message: error.to_string(),
        })?;
    store
        .evaluate_route_readiness(
            &request.install_id,
            &request.proposal_id,
            &request.route_id,
            &request.request_id,
        )
        .await
}

pub async fn inspect_route_readiness(
    request: RouteReadinessInspectionRequest,
) -> Result<RouteReadinessRecord, RouteReadinessError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| RouteReadinessError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_route_readiness(&request.install_id, &request.proposal_id, &request.route_id)
        .await
}

pub async fn apply_route_readiness_action(
    request: RouteReadinessActionRequest,
) -> Result<RouteReadinessActionResult, RouteReadinessError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| RouteReadinessError::Persistence {
            message: error.to_string(),
        })?;
    store
        .apply_route_readiness_action(
            &request.install_id,
            &request.proposal_id,
            &request.route_id,
            request.action,
        )
        .await
}

pub async fn materialize_strategy_selection(
    request: MaterializeStrategySelectionRequest,
) -> Result<StrategySelectionRecord, StrategySelectionError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;
    store.materialize_strategy_selection(request).await
}

pub async fn apply_strategy_selection_override(
    request: ApplyStrategySelectionOverrideRequest,
) -> Result<StrategySelectionInspection, StrategySelectionError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;
    store
        .apply_strategy_selection_override(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
            request.override_record,
        )
        .await
}

pub async fn reopen_strategy_selection(
    request: ReopenStrategySelectionRequest,
) -> Result<StrategySelectionRecord, StrategySelectionError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;
    store.reopen_strategy_selection(request).await
}

pub async fn approve_strategy_selection(
    request: ApproveStrategySelectionRequest,
) -> Result<StrategySelectionRecord, StrategySelectionError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;
    store.approve_strategy_selection(request).await
}

pub async fn inspect_strategy_selection(
    request: InspectStrategySelectionRequest,
) -> Result<StrategySelectionInspection, StrategySelectionError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_selection(&request.install_id, &request.proposal_id)
        .await
}

pub async fn inspect_strategy_runtime_eligibility(
    request: InspectStrategyRuntimeRequest,
) -> Result<StrategyRuntimeHandoffRecord, StrategyRuntimeHandoffError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategyRuntimeHandoffError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_runtime_eligibility(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        )
        .await
}

pub async fn inspect_strategy_operator_report(
    request: InspectStrategyRuntimeRequest,
) -> Result<StrategyOperatorReport, StrategyRuntimeHandoffError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategyRuntimeHandoffError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_operator_report(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        )
        .await
}

pub async fn inspect_strategy_exception_rollup(
    request: InspectStrategyRuntimeRequest,
) -> Result<StrategyExceptionRollup, StrategyRuntimeHandoffError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategyRuntimeHandoffError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_exception_rollup(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        )
        .await
}

pub async fn inspect_strategy_report_window(
    request: InspectStrategyReportWindowRequest,
) -> Result<StrategyReportWindow, StrategyRuntimeHandoffError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategyRuntimeHandoffError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_report_window(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
            &request.cursor,
            request.window_limit,
        )
        .await
}

pub async fn inspect_strategy_runtime_monitoring(
    request: InspectStrategyRuntimeRequest,
) -> Result<StrategyRuntimeMonitoringSummary, StrategyRuntimeHandoffError> {
    let store = OnboardingStore::open(&request.state_db_path)
        .await
        .map_err(|error| StrategyRuntimeHandoffError::Persistence {
            message: error.to_string(),
        })?;
    store
        .inspect_strategy_runtime_monitoring(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        )
        .await
}
