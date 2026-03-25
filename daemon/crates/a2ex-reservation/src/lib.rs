use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_rusqlite::Connection;
use uuid::Uuid;

const BOOTSTRAP_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS capital_reservations (
    reservation_id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL,
    asset TEXT NOT NULL,
    amount INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('held', 'consumed', 'released')),
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS event_journal (
    event_id TEXT PRIMARY KEY,
    stream_type TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"#;

const RESERVATION_STREAM_TYPE: &str = "reservation";
const HELD_EVENT_TYPE: &str = "reservation_held";
const CONSUMED_EVENT_TYPE: &str = "reservation_consumed";
const RELEASED_EVENT_TYPE: &str = "reservation_released";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationRequest {
    pub reservation_id: String,
    pub execution_id: String,
    pub asset: String,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationDecision {
    pub reservation_id: String,
    pub execution_id: String,
    pub asset: String,
    pub amount: u64,
    pub state: ReservationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    Held,
    Consumed,
    Released,
}

impl ReservationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Consumed => "consumed",
            Self::Released => "released",
        }
    }
}

#[derive(Debug, Error)]
pub enum ReservationError {
    #[error("failed to open reservation database at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to initialize reservation database at {path}")]
    Initialize {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to update reservation state at {path}")]
    Persist {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("reservation {reservation_id} not found")]
    NotFound { reservation_id: String },
    #[error("reservation {reservation_id} is not in state {expected}")]
    InvalidState {
        reservation_id: String,
        expected: &'static str,
    },
    #[error("reservation amount {requested} exceeds held amount {available} for {reservation_id}")]
    AmountExceeded {
        reservation_id: String,
        requested: u64,
        available: u64,
    },
}

#[async_trait]
pub trait ReservationManager: Send + Sync {
    async fn hold(&self, req: ReservationRequest) -> Result<ReservationDecision, ReservationError>;
    async fn require_held(
        &self,
        reservation_id: &str,
        amount: u64,
    ) -> Result<ReservationDecision, ReservationError>;
    async fn consume(&self, reservation_id: &str, amount: u64) -> Result<(), ReservationError>;
    async fn release(
        &self,
        reservation_id: &str,
        amount: Option<u64>,
    ) -> Result<(), ReservationError>;
}

#[derive(Debug)]
pub struct SqliteReservationManager {
    path: PathBuf,
    connection: Connection,
}

impl SqliteReservationManager {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, ReservationError> {
        let path = path.as_ref().to_path_buf();
        let connection =
            Connection::open(&path)
                .await
                .map_err(|source| ReservationError::Open {
                    path: path.clone(),
                    source,
                })?;

        connection
            .call(|conn| {
                conn.execute_batch(BOOTSTRAP_SQL)?;
                Ok(())
            })
            .await
            .map_err(|source| ReservationError::Initialize {
                path: path.clone(),
                source,
            })?;

        Ok(Self { path, connection })
    }

    async fn transition(
        &self,
        reservation_id: &str,
        amount: Option<u64>,
        expected_state: ReservationState,
        next_state: ReservationState,
        event_type: &'static str,
    ) -> Result<(), ReservationError> {
        let path = self.path.clone();
        let reservation_id = reservation_id.to_owned();
        let reservation_id_for_call = reservation_id.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                let reservation = load_reservation(&tx, &reservation_id_for_call)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;

                if reservation.state != expected_state.as_str() {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "reservation {} must be {} before transition",
                        reservation_id_for_call,
                        expected_state.as_str(),
                    )));
                }

                let transition_amount = amount.unwrap_or(reservation.amount);
                if transition_amount > reservation.amount {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "reservation {} exceeds held amount {}",
                        reservation_id_for_call, reservation.amount,
                    )));
                }

                tx.execute(
                    "UPDATE capital_reservations
                     SET state = ?2, updated_at = CURRENT_TIMESTAMP
                     WHERE reservation_id = ?1",
                    params![reservation_id_for_call, next_state.as_str()],
                )?;

                append_journal_entry(
                    &tx,
                    &reservation.reservation_id,
                    event_type,
                    reservation.execution_id,
                    reservation.asset,
                    transition_amount,
                    next_state,
                )?;

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| match &source {
                tokio_rusqlite::Error::Error(rusqlite::Error::QueryReturnedNoRows) => {
                    ReservationError::NotFound { reservation_id }
                }
                tokio_rusqlite::Error::Error(rusqlite::Error::InvalidParameterName(_)) => {
                    ReservationError::InvalidState {
                        reservation_id,
                        expected: expected_state.as_str(),
                    }
                }
                _ => ReservationError::Persist { path, source },
            })
    }
}

#[async_trait]
impl ReservationManager for SqliteReservationManager {
    async fn hold(&self, req: ReservationRequest) -> Result<ReservationDecision, ReservationError> {
        let path = self.path.clone();
        let request = req.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO capital_reservations (reservation_id, execution_id, asset, amount, state, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
                     ON CONFLICT(reservation_id) DO UPDATE SET
                        execution_id = excluded.execution_id,
                        asset = excluded.asset,
                        amount = excluded.amount,
                        state = excluded.state,
                        updated_at = CURRENT_TIMESTAMP",
                    params![
                        request.reservation_id,
                        request.execution_id,
                        request.asset,
                        request.amount,
                        ReservationState::Held.as_str(),
                    ],
                )?;

                append_journal_entry(
                    &tx,
                    &request.reservation_id,
                    HELD_EVENT_TYPE,
                    request.execution_id.clone(),
                    request.asset.clone(),
                    request.amount,
                    ReservationState::Held,
                )?;

                tx.commit()?;
                Ok(ReservationDecision {
                    reservation_id: request.reservation_id,
                    execution_id: request.execution_id,
                    asset: request.asset,
                    amount: request.amount,
                    state: ReservationState::Held,
                })
            })
            .await
            .map_err(|source| ReservationError::Persist { path, source })
    }

    async fn require_held(
        &self,
        reservation_id: &str,
        amount: u64,
    ) -> Result<ReservationDecision, ReservationError> {
        let path = self.path.clone();
        let reservation_id = reservation_id.to_owned();
        let reservation_id_for_call = reservation_id.clone();

        self.connection
            .call(move |conn| {
                let reservation = conn
                    .query_row(
                        "SELECT reservation_id, execution_id, asset, amount, state
                         FROM capital_reservations
                         WHERE reservation_id = ?1",
                        [reservation_id_for_call.as_str()],
                        |row| {
                            Ok(StoredReservation {
                                reservation_id: row.get(0)?,
                                execution_id: row.get(1)?,
                                asset: row.get(2)?,
                                amount: row.get(3)?,
                                state: row.get(4)?,
                            })
                        },
                    )
                    .optional()?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;

                if reservation.state != ReservationState::Held.as_str() {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "reservation {} must be {} before dispatch",
                        reservation_id_for_call,
                        ReservationState::Held.as_str(),
                    )));
                }

                if amount > reservation.amount {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "reservation {} exceeds held amount {}",
                        reservation_id_for_call, reservation.amount,
                    )));
                }

                Ok(ReservationDecision {
                    reservation_id: reservation.reservation_id,
                    execution_id: reservation.execution_id,
                    asset: reservation.asset,
                    amount: reservation.amount,
                    state: ReservationState::Held,
                })
            })
            .await
            .map_err(|source| match &source {
                tokio_rusqlite::Error::Error(rusqlite::Error::QueryReturnedNoRows) => {
                    ReservationError::NotFound { reservation_id }
                }
                tokio_rusqlite::Error::Error(rusqlite::Error::InvalidParameterName(_)) => {
                    ReservationError::InvalidState {
                        reservation_id,
                        expected: ReservationState::Held.as_str(),
                    }
                }
                _ => ReservationError::Persist { path, source },
            })
    }

    async fn consume(&self, reservation_id: &str, amount: u64) -> Result<(), ReservationError> {
        self.transition(
            reservation_id,
            Some(amount),
            ReservationState::Held,
            ReservationState::Consumed,
            CONSUMED_EVENT_TYPE,
        )
        .await
    }

    async fn release(
        &self,
        reservation_id: &str,
        amount: Option<u64>,
    ) -> Result<(), ReservationError> {
        self.transition(
            reservation_id,
            amount,
            ReservationState::Consumed,
            ReservationState::Released,
            RELEASED_EVENT_TYPE,
        )
        .await
    }
}

#[derive(Debug)]
struct StoredReservation {
    reservation_id: String,
    execution_id: String,
    asset: String,
    amount: u64,
    state: String,
}

fn load_reservation(
    tx: &rusqlite::Transaction<'_>,
    reservation_id: &str,
) -> Result<Option<StoredReservation>, rusqlite::Error> {
    tx.query_row(
        "SELECT reservation_id, execution_id, asset, amount, state
         FROM capital_reservations
         WHERE reservation_id = ?1",
        [reservation_id],
        |row| {
            Ok(StoredReservation {
                reservation_id: row.get(0)?,
                execution_id: row.get(1)?,
                asset: row.get(2)?,
                amount: row.get(3)?,
                state: row.get(4)?,
            })
        },
    )
    .optional()
}

fn append_journal_entry(
    tx: &rusqlite::Transaction<'_>,
    reservation_id: &str,
    event_type: &str,
    execution_id: String,
    asset: String,
    amount: u64,
    state: ReservationState,
) -> rusqlite::Result<()> {
    let payload_json = serde_json::json!({
        "reservation_id": reservation_id,
        "execution_id": execution_id,
        "asset": asset,
        "amount": amount,
        "state": state,
    })
    .to_string();

    tx.execute(
        "INSERT INTO event_journal (event_id, stream_type, stream_id, event_type, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)",
        params![
            Uuid::now_v7().to_string(),
            RESERVATION_STREAM_TYPE,
            reservation_id,
            event_type,
            payload_json,
        ],
    )?;

    Ok(())
}
