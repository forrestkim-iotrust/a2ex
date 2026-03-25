use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio_rusqlite::Connection;

use crate::reconciliation::ExecutionStateRecord;
use crate::{JournalEntry, StateError, StateRepository};

const ANALYTICS_BOOTSTRAP_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS execution_projection (
    execution_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsProjectionReport {
    pub projected_executions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionAnalyticsRecord {
    pub execution_id: String,
    pub plan_id: String,
    pub status: String,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum AnalyticsError {
    #[error("failed to load canonical journal for analytics projection")]
    LoadCanonical(#[from] StateError),
    #[error("failed to open analytics database at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to initialize analytics database at {path}")]
    Initialize {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to persist analytics projection at {path}")]
    Persist {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to decode execution journal payload")]
    DecodeExecution(#[from] serde_json::Error),
}

pub async fn project_execution_analytics(
    state_db_path: impl AsRef<Path>,
    analytics_db_path: impl AsRef<Path>,
) -> Result<AnalyticsProjectionReport, AnalyticsError> {
    let repository = StateRepository::open(state_db_path).await?;
    let journal = repository.load_journal().await?;
    let executions = latest_execution_states(journal)?;

    let path = analytics_db_path.as_ref().to_path_buf();
    let connection = Connection::open(&path)
        .await
        .map_err(|source| AnalyticsError::Open {
            path: path.clone(),
            source,
        })?;

    connection
        .call(|conn| {
            conn.execute_batch(ANALYTICS_BOOTSTRAP_SQL)?;
            Ok(())
        })
        .await
        .map_err(|source| AnalyticsError::Initialize {
            path: path.clone(),
            source,
        })?;

    let projection_rows = executions.into_values().collect::<Vec<_>>();
    let report = AnalyticsProjectionReport {
        projected_executions: projection_rows.len(),
    };

    connection
        .call(move |conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM execution_projection", [])?;

            for execution in &projection_rows {
                tx.execute(
                    "INSERT INTO execution_projection (execution_id, plan_id, status, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    (
                        execution.execution_id.as_str(),
                        execution.plan_id.as_str(),
                        execution.status.as_str(),
                        execution.updated_at.as_str(),
                    ),
                )?;
            }

            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|source| AnalyticsError::Persist { path, source })?;

    Ok(report)
}

pub async fn load_execution_analytics(
    analytics_db_path: impl AsRef<Path>,
) -> Result<Vec<ExecutionAnalyticsRecord>, AnalyticsError> {
    let path = analytics_db_path.as_ref().to_path_buf();
    let connection = Connection::open(&path)
        .await
        .map_err(|source| AnalyticsError::Open {
            path: path.clone(),
            source,
        })?;

    connection
        .call(|conn| {
            conn.execute_batch(ANALYTICS_BOOTSTRAP_SQL)?;
            Ok(())
        })
        .await
        .map_err(|source| AnalyticsError::Initialize {
            path: path.clone(),
            source,
        })?;

    connection
        .call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT execution_id, plan_id, status, updated_at
                 FROM execution_projection
                 ORDER BY execution_id",
            )?;
            stmt.query_map([], |row| {
                Ok(ExecutionAnalyticsRecord {
                    execution_id: row.get(0)?,
                    plan_id: row.get(1)?,
                    status: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .await
        .map_err(|source| AnalyticsError::Persist { path, source })
}

fn latest_execution_states(
    journal: Vec<JournalEntry>,
) -> Result<BTreeMap<String, ExecutionStateRecord>, AnalyticsError> {
    let mut executions = BTreeMap::new();

    for entry in journal {
        if entry.event_type != "execution_state_changed" {
            continue;
        }

        let execution = serde_json::from_str::<ExecutionStateRecord>(&entry.payload_json)?;
        executions.insert(execution.execution_id.clone(), execution);
    }

    Ok(executions)
}
