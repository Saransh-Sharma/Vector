//! Canonical append-only run artifacts and rebuildable SQLite projections.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use vector_core::PortableRunSpec;

#[derive(Clone)]
pub struct VectorDatabase {
    pool: SqlitePool,
}

impl VectorDatabase {
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePool::connect_with(options).await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<(), DbError> {
        for statement in MIGRATION.split("-- statement") {
            let statement = statement.trim();
            if !statement.is_empty() {
                sqlx::query(statement).execute(&self.pool).await?;
            }
        }
        Ok(())
    }

    pub async fn project_run(&self, manifest: &RunManifest) -> Result<(), DbError> {
        sqlx::query("INSERT INTO runs (id, profile, harness, model, status, fingerprint, started_at, updated_at, root) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET status=excluded.status, updated_at=excluded.updated_at")
            .bind(manifest.id.to_string()).bind(&manifest.profile).bind(&manifest.harness).bind(&manifest.model).bind(&manifest.status).bind(&manifest.fingerprint).bind(manifest.started_at.to_rfc3339()).bind(manifest.updated_at.to_rfc3339()).bind(manifest.root.to_string_lossy().to_string()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn project_event(&self, run_id: Uuid, event: &PortableEvent) -> Result<(), DbError> {
        sqlx::query("INSERT OR IGNORE INTO events (run_id, sequence, occurred_at, kind, payload) VALUES (?, ?, ?, ?, ?)")
            .bind(run_id.to_string()).bind(event.sequence as i64).bind(event.occurred_at.to_rfc3339()).bind(&event.kind).bind(serde_json::to_string(&event.payload)?).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn recent_runs(&self, limit: u32) -> Result<Vec<RunSummary>, DbError> {
        let rows = sqlx::query("SELECT id, profile, harness, model, status, fingerprint, started_at, root FROM runs ORDER BY started_at DESC LIMIT ?").bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(RunSummary {
                    id: row.try_get("id")?,
                    profile: row.try_get("profile")?,
                    harness: row.try_get("harness")?,
                    model: row.try_get("model")?,
                    status: row.try_get("status")?,
                    fingerprint: row.try_get("fingerprint")?,
                    started_at: row.try_get("started_at")?,
                    root: row.try_get("root")?,
                })
            })
            .collect()
    }
}

const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY,
  profile TEXT NOT NULL,
  harness TEXT NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  root TEXT NOT NULL
);
-- statement
CREATE TABLE IF NOT EXISTS events (
  run_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  occurred_at TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  PRIMARY KEY (run_id, sequence),
  FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
);
-- statement
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('queued','running','waiting_for_input','cancelling','succeeded','failed','interrupted')),
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunManifest {
    pub id: Uuid,
    pub profile: String,
    pub harness: String,
    pub model: String,
    pub status: String,
    pub fingerprint: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableEvent {
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub kind: String,
    pub payload: Value,
    #[serde(default)]
    pub native_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub profile: String,
    pub harness: String,
    pub model: String,
    pub status: String,
    pub fingerprint: String,
    pub started_at: String,
    pub root: String,
}

pub struct RunLedger {
    pub dir: PathBuf,
    pub manifest: RunManifest,
    next_sequence: u64,
}

impl RunLedger {
    pub async fn create(base: &Path, spec: &PortableRunSpec) -> Result<Self, DbError> {
        let dir = base.join(spec.run_id.to_string());
        tokio::fs::create_dir_all(dir.join("artifacts")).await?;
        tokio::fs::create_dir_all(dir.join("native")).await?;
        let now = Utc::now();
        let manifest = RunManifest {
            id: spec.run_id,
            profile: spec.profile.clone(),
            harness: spec.harness.harness.to_string(),
            model: spec.models.primary.clone(),
            status: "prepared".into(),
            fingerprint: spec.fingerprint()?,
            started_at: now,
            updated_at: now,
            root: spec.workspace.root.clone(),
        };
        write_json_atomic(&dir.join("run.json"), &manifest).await?;
        write_json_atomic(&dir.join("runspec.sanitized.json"), spec).await?;
        write_json_atomic(&dir.join("metrics.json"), &serde_json::json!({})).await?;
        write_json_atomic(
            &dir.join("verification.json"),
            &serde_json::json!({"status":"not-run"}),
        )
        .await?;
        Ok(Self {
            dir,
            manifest,
            next_sequence: 1,
        })
    }

    pub async fn append(
        &mut self,
        kind: impl Into<String>,
        payload: Value,
    ) -> Result<PortableEvent, DbError> {
        let event = PortableEvent {
            sequence: self.next_sequence,
            occurred_at: Utc::now(),
            kind: kind.into(),
            payload,
            native_ref: None,
        };
        self.next_sequence += 1;
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("events.jsonl"))
            .await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        Ok(event)
    }

    pub async fn set_status(&mut self, status: &str) -> Result<(), DbError> {
        self.manifest.status = status.into();
        self.manifest.updated_at = Utc::now();
        write_json_atomic(&self.dir.join("run.json"), &self.manifest).await
    }
}

async fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), DbError> {
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    tokio::fs::write(&temp, bytes).await?;
    tokio::fs::rename(temp, path).await?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error("VCTR_RUN_FAILED: filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VCTR_RUN_FAILED: database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("VCTR_RUN_FAILED: serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("VCTR_RUN_FAILED: fingerprint error: {0}")]
    Core(#[from] vector_core::CoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sqlite_projection_is_idempotent() {
        let dir = tempdir().unwrap();
        let db = VectorDatabase::open(&dir.path().join("vector.db"))
            .await
            .unwrap();
        let now = Utc::now();
        let run = RunManifest {
            id: Uuid::now_v7(),
            profile: "pi-safe".into(),
            harness: "pi".into(),
            model: "qwen".into(),
            status: "running".into(),
            fingerprint: "abc".into(),
            started_at: now,
            updated_at: now,
            root: dir.path().into(),
        };
        db.project_run(&run).await.unwrap();
        db.project_run(&run).await.unwrap();
        assert_eq!(db.recent_runs(10).await.unwrap().len(), 1);
    }
}
