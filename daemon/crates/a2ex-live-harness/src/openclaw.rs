use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Stdio,
};

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};

use crate::evidence_bundle::{
    OPENCLAW_ACTION_SUMMARY_FILE_NAME, OpenClawActionPhase, OpenClawActionSummary,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenClawLaunchMode {
    Spawn,
    Attach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawLaunchRequest {
    pub run_id: String,
    pub install_url: String,
    pub goal: String,
    pub runtime_command: String,
    pub image_ref: Option<String>,
    pub launch_mode: OpenClawLaunchMode,
    pub mcp_spawn_command: Vec<String>,
    pub guidance_contract: String,
    pub work_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawLaunchArtifacts {
    pub request_path: PathBuf,
    pub guidance_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug)]
pub struct OpenClawLaunchHandle {
    pub pid: Option<u32>,
    pub mode: OpenClawLaunchMode,
    pub artifacts: OpenClawLaunchArtifacts,
    child: Option<Child>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawExit {
    pub status_code: Option<i32>,
    pub success: bool,
}

impl OpenClawLaunchRequest {
    pub fn guidance_contract_markdown(&self) -> String {
        format!(
            "# A2EX OpenClaw install→approval handoff contract\n\n- run_id: `{}`\n- install_url: `{}`\n- goal: `{}`\n- OpenClaw is the real external consumer for install→approval.\n- `a2ex-mcp` remains the shipped stdio boundary.\n- WAIaaS remains the wallet/session/policy authority.\n- After `strategy_selection.approve`, exit cleanly and let `a2ex-live-harness` continue into the checked-in `across-mainnet-to-base-usdc-smoke` route under the same run_id.\n- The harness will persist `waiaas-authority.json` and `live-route-evidence.json` in the same run directory; do not create a second orchestration stack or alternate truth store.\n\n## Required choreography\n\n1. `onboarding.bootstrap_install`\n2. `skills.load_bundle`\n3. `skills.generate_proposal_packet`\n4. `readiness.evaluate_route`\n5. `strategy_selection.approve`\n6. hand off to the checked-in S02 live route under WAIaaS authority\n\n## MCP spawn command\n\n```json\n{}\n```\n\n## Guidance\n\n{}\n",
            self.run_id,
            self.install_url,
            self.goal,
            serde_json::to_string_pretty(&self.mcp_spawn_command)
                .expect("mcp spawn command serializes"),
            self.guidance_contract,
        )
    }

    pub fn persisted_artifacts(&self, run_dir: &Path) -> std::io::Result<OpenClawLaunchArtifacts> {
        fs::create_dir_all(run_dir)?;
        let request_path = run_dir.join("openclaw-request.json");
        let guidance_path = run_dir.join("openclaw-guidance.md");
        let stdout_path = run_dir.join("openclaw.stdout.log");
        let stderr_path = run_dir.join("openclaw.stderr.log");
        fs::write(
            &request_path,
            serde_json::to_vec_pretty(self).expect("launch request serializes"),
        )?;
        fs::write(&guidance_path, self.guidance_contract_markdown())?;
        Ok(OpenClawLaunchArtifacts {
            request_path,
            guidance_path,
            stdout_path,
            stderr_path,
        })
    }

    pub async fn launch(self, run_dir: &Path) -> std::io::Result<OpenClawLaunchHandle> {
        let artifacts = self.persisted_artifacts(run_dir)?;
        match self.launch_mode {
            OpenClawLaunchMode::Attach => Ok(OpenClawLaunchHandle {
                pid: None,
                mode: OpenClawLaunchMode::Attach,
                artifacts,
                child: None,
            }),
            OpenClawLaunchMode::Spawn => {
                let stdout = File::create(&artifacts.stdout_path)?;
                let stderr = File::create(&artifacts.stderr_path)?;
                let mut command = Command::new("/bin/sh");
                command
                    .arg("-lc")
                    .arg(&self.runtime_command)
                    .current_dir(&self.work_dir)
                    .envs(self.launch_env(&artifacts))
                    .stdout(Stdio::from(stdout))
                    .stderr(Stdio::from(stderr));
                let child = command.spawn()?;
                Ok(OpenClawLaunchHandle {
                    pid: child.id(),
                    mode: OpenClawLaunchMode::Spawn,
                    artifacts,
                    child: Some(child),
                })
            }
        }
    }

    fn launch_env(&self, artifacts: &OpenClawLaunchArtifacts) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            (
                "A2EX_OPENCLAW_INSTALL_URL".to_owned(),
                self.install_url.clone(),
            ),
            ("A2EX_OPENCLAW_GOAL".to_owned(), self.goal.clone()),
            ("A2EX_OPENCLAW_RUN_ID".to_owned(), self.run_id.clone()),
            (
                "A2EX_OPENCLAW_REQUEST_JSON".to_owned(),
                artifacts.request_path.display().to_string(),
            ),
            (
                "A2EX_OPENCLAW_GUIDANCE_PATH".to_owned(),
                artifacts.guidance_path.display().to_string(),
            ),
            (
                "A2EX_MCP_STDIO_COMMAND_JSON".to_owned(),
                serde_json::to_string(&self.mcp_spawn_command)
                    .expect("mcp spawn command serializes"),
            ),
        ]);
        if let Some(image_ref) = &self.image_ref {
            env.insert("A2EX_OPENCLAW_IMAGE_REF".to_owned(), image_ref.clone());
        }
        env
    }
}

impl OpenClawLaunchHandle {
    pub async fn wait(&mut self) -> std::io::Result<OpenClawExit> {
        match self.child.as_mut() {
            Some(child) => {
                let status = child.wait().await?;
                Ok(OpenClawExit {
                    status_code: status.code(),
                    success: status.success(),
                })
            }
            None => Ok(OpenClawExit {
                status_code: None,
                success: true,
            }),
        }
    }
}

pub fn persist_action_summary(
    request: &OpenClawLaunchRequest,
    artifacts: &OpenClawLaunchArtifacts,
    mode: &OpenClawLaunchMode,
    exit: &OpenClawExit,
    run_dir: &Path,
) -> std::io::Result<OpenClawActionSummary> {
    let summary = build_action_summary(request, artifacts, mode, exit)?;
    fs::write(
        run_dir.join(OPENCLAW_ACTION_SUMMARY_FILE_NAME),
        serde_json::to_vec_pretty(&summary).expect("OpenClaw action summary serializes"),
    )?;
    Ok(summary)
}

pub fn build_action_summary(
    request: &OpenClawLaunchRequest,
    artifacts: &OpenClawLaunchArtifacts,
    mode: &OpenClawLaunchMode,
    exit: &OpenClawExit,
) -> std::io::Result<OpenClawActionSummary> {
    let stdout_line_count = count_lines(&artifacts.stdout_path)?;
    let stderr_line_count = count_lines(&artifacts.stderr_path)?;
    let launch_mode = match mode {
        OpenClawLaunchMode::Spawn => "spawn",
        OpenClawLaunchMode::Attach => "attach",
    }
    .to_owned();
    let overall_status = if exit.success { "completed" } else { "failed" }.to_owned();
    let stop_reason = if exit.success {
        if stderr_line_count > 0 {
            Some("completed_with_stderr_output".to_owned())
        } else {
            Some("approval_boundary_handoff_completed".to_owned())
        }
    } else {
        Some("openclaw_exit_non_zero".to_owned())
    };
    let action_summary = if exit.success {
        "OpenClaw action summary derived from request, guidance, and persisted log artifact refs without promoting raw stdout/stderr to primary truth."
    } else {
        "OpenClaw action summary records a failed launch or handoff using typed artifact refs and bounded status fields instead of raw log promotion."
    }
    .to_owned();
    let guidance_ref = artifacts.guidance_path.display().to_string();
    let request_ref = artifacts.request_path.display().to_string();
    let typed_phase_summary = vec![
        OpenClawActionPhase {
            phase_key: "onboarding.bootstrap_install".to_owned(),
            action_name: "bootstrap install".to_owned(),
            status: phase_status(exit.success).to_owned(),
            evidence_refs: vec![request_ref.clone(), guidance_ref.clone()],
        },
        OpenClawActionPhase {
            phase_key: "skills.load_bundle".to_owned(),
            action_name: "load bundle".to_owned(),
            status: phase_status(exit.success).to_owned(),
            evidence_refs: vec![guidance_ref.clone()],
        },
        OpenClawActionPhase {
            phase_key: "skills.generate_proposal_packet".to_owned(),
            action_name: "generate proposal".to_owned(),
            status: phase_status(exit.success).to_owned(),
            evidence_refs: vec![guidance_ref.clone()],
        },
        OpenClawActionPhase {
            phase_key: "readiness.evaluate_route".to_owned(),
            action_name: "evaluate route".to_owned(),
            status: phase_status(exit.success).to_owned(),
            evidence_refs: vec![guidance_ref.clone()],
        },
        OpenClawActionPhase {
            phase_key: "strategy_selection.approve".to_owned(),
            action_name: "approve strategy".to_owned(),
            status: phase_status(exit.success).to_owned(),
            evidence_refs: vec![guidance_ref],
        },
    ];

    Ok(OpenClawActionSummary {
        run_id: request.run_id.clone(),
        install_url: Some(request.install_url.clone()),
        request_source: "openclaw-request.json".to_owned(),
        request_path: request_ref,
        guidance_path: artifacts.guidance_path.display().to_string(),
        stdout_path: artifacts.stdout_path.display().to_string(),
        stderr_path: artifacts.stderr_path.display().to_string(),
        launch_mode,
        runtime_command_ref: request.runtime_command.clone(),
        image_ref: request.image_ref.clone(),
        overall_status,
        stop_reason,
        stdout_line_count,
        stderr_line_count,
        typed_phase_summary,
        summary: action_summary,
    })
}

fn phase_status(success: bool) -> &'static str {
    if success {
        "completed_or_handed_off"
    } else {
        "interrupted"
    }
}

fn count_lines(path: &Path) -> std::io::Result<usize> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents.lines().count()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

pub fn default_guidance_contract() -> String {
    [
        "Use the shipped A2EX install→proposal→approval choreography.",
        "Treat `a2ex-mcp` as the only MCP/std.io server for this run.",
        "Respect WAIaaS as the wallet/session/policy authority.",
        "Record any surfaced install_id, proposal_id, selection_id, session_id, and policy_id references back into the run record.",
        "After `strategy_selection.approve`, exit cleanly so the harness can continue into `across-mainnet-to-base-usdc-smoke` under the same run_id and persist `waiaas-authority.json` plus `live-route-evidence.json`.",
    ]
    .join(" ")
}
