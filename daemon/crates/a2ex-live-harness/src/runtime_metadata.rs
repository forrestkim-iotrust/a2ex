use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeMetadata {
    pub openclaw: OpenClawRuntimeMetadata,
    pub a2ex: A2exRuntimeMetadata,
    pub waiaas: WaiaasRuntimeMetadata,
    pub env: Vec<EnvVarStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OpenClawRuntimeMetadata {
    pub runtime_command: String,
    pub version: Option<String>,
    pub image_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct A2exRuntimeMetadata {
    pub harness_version: String,
    pub rust_version_requirement: String,
    pub run_entrypoint: String,
    pub report_command_ref: String,
    pub report_command_template: Vec<String>,
    pub mcp_spawn_command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WaiaasRuntimeMetadata {
    pub base_url: Option<String>,
    pub health_url: Option<String>,
    pub session_url: Option<String>,
    pub policy_url: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnvVarStatus {
    pub key: String,
    pub present: bool,
    pub redacted_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMetadataInput {
    pub openclaw_runtime_command: String,
    pub openclaw_version: Option<String>,
    pub openclaw_image_ref: Option<String>,
    pub waiaas_base_url: Option<String>,
    pub waiaas_version: Option<String>,
    pub env_keys: Vec<String>,
}

impl RuntimeMetadataInput {
    pub fn capture(&self) -> RuntimeMetadata {
        let run_entrypoint = std::env::var("A2EX_OPENCLAW_RUN_ENTRYPOINT")
            .unwrap_or_else(|_| "scripts/run-m008-s03.sh".to_owned());
        let report_command_ref = std::env::var("A2EX_OPENCLAW_REPORT_COMMAND_REF")
            .unwrap_or_else(|_| "scripts/report-m008-s03.sh".to_owned());
        let report_command_path = if report_command_ref.starts_with("./") {
            report_command_ref.clone()
        } else {
            format!("./{report_command_ref}")
        };

        RuntimeMetadata {
            openclaw: OpenClawRuntimeMetadata {
                runtime_command: self.openclaw_runtime_command.clone(),
                version: self.openclaw_version.clone(),
                image_ref: self.openclaw_image_ref.clone(),
            },
            a2ex: A2exRuntimeMetadata {
                harness_version: env!("CARGO_PKG_VERSION").to_owned(),
                rust_version_requirement: env!("CARGO_PKG_RUST_VERSION").to_owned(),
                run_entrypoint,
                report_command_ref,
                report_command_template: vec![
                    "bash".to_owned(),
                    report_command_path,
                    ".a2ex-openclaw-harness/runs/<run_id>".to_owned(),
                ],
                mcp_spawn_command: vec![
                    "cargo".to_owned(),
                    "run".to_owned(),
                    "--quiet".to_owned(),
                    "--manifest-path".to_owned(),
                    "daemon/Cargo.toml".to_owned(),
                    "-p".to_owned(),
                    "a2ex-mcp".to_owned(),
                    "--bin".to_owned(),
                    "a2ex-mcp".to_owned(),
                ],
            },
            waiaas: WaiaasRuntimeMetadata {
                health_url: self
                    .waiaas_base_url
                    .as_ref()
                    .map(|base| format!("{}/health", base.trim_end_matches('/'))),
                session_url: self
                    .waiaas_base_url
                    .as_ref()
                    .map(|base| format!("{}/sessions", base.trim_end_matches('/'))),
                policy_url: self
                    .waiaas_base_url
                    .as_ref()
                    .map(|base| format!("{}/policies", base.trim_end_matches('/'))),
                base_url: self.waiaas_base_url.clone(),
                version: self.waiaas_version.clone(),
            },
            env: self
                .env_keys
                .iter()
                .map(|key| EnvVarStatus {
                    key: key.clone(),
                    present: std::env::var_os(key).is_some(),
                    redacted_status: if std::env::var_os(key).is_some() {
                        "present".to_owned()
                    } else {
                        "missing".to_owned()
                    },
                })
                .collect(),
        }
    }
}
