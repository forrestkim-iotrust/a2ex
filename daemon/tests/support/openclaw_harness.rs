use std::{fs, path::PathBuf};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

pub fn entrypoint_path() -> PathBuf {
    repo_root().join("scripts/run-m008-s01.sh")
}

pub fn runbook_text() -> String {
    let full_path = repo_root().join("docs/runbooks/m008-s01-openclaw-harness.md");
    fs::read_to_string(&full_path).unwrap_or_else(|error| {
        panic!(
            "required S01 runbook missing at {}: {error}",
            full_path.display()
        )
    })
}

pub fn assert_runbook_phrases(mut gaps: Vec<String>) -> Vec<String> {
    let runbook = runbook_text();
    for phrase in [
        "install_url",
        "goal",
        "OpenClaw",
        "a2ex-mcp",
        "onboarding.bootstrap_install",
        "skills.load_bundle",
        "skills.generate_proposal_packet",
        "readiness.evaluate_route",
        "strategy_selection.approve",
        "run_id",
        "A2EX_OPENCLAW_INSTALL_URL",
        "A2EX_OPENCLAW_GOAL",
    ] {
        if !runbook.contains(phrase) {
            gaps.push(format!(
                "runbook must mention `{phrase}` so the ignored live acceptance test targets the shipped install→proposal→approval contract"
            ));
        }
    }
    gaps
}
