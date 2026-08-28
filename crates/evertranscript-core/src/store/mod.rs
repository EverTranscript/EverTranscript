//! The record's storage: one writer, several readers.
//!
//! ADR-0026 makes the Core the record's only writer. This module makes that
//! executable rather than aspirational: exactly one thread owns exactly one
//! writable connection, and every mutation in the process queues through it.
//! Reads take a separate pool so History search never waits behind the live
//! transcript writer — the shape Granola's storage process ships.

pub mod meetings;
pub mod schema;
pub mod watchlist;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use rusqlite::Connection;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// How many concurrent readers. Reads are short; this is about never
/// blocking a Client's query behind a write, not about throughput.
const READER_COUNT: usize = 4;

type WriteJob = Box<dyn FnOnce(&mut Connection) + Send>;

/// A handle to the record. Cheap to clone; the connections live behind it.
#[derive(Clone)]
pub struct Store {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    write_tx: mpsc::Sender<WriteJob>,
    readers: Mutex<Vec<Connection>>,
    reader_permits: Arc<Semaphore>,
    database_path: PathBuf,
}

impl Store {
    /// Opens (creating if needed) the database and starts the writer thread.
    pub fn open(database_path: &Path) -> Result<Self> {
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let mut writer = Connection::open(database_path)
            .with_context(|| format!("opening {}", database_path.display()))?;
        schema::configure(&writer)?;
        schema::migrate(&mut writer).context("applying migrations")?;

        let (write_tx, mut write_rx) = mpsc::channel::<WriteJob>(256);
        std::thread::Builder::new()
            .name("evertranscript-writer".to_string())
            .spawn(move || {
                // One thread, one connection, for the life of the Core: the
                // single-writer rule with nothing left to enforce.
                while let Some(job) = write_rx.blocking_recv() {
                    job(&mut writer);
                }
                tracing::debug!("writer thread finished");
            })
            .context("spawning the writer thread")?;

        Ok(Self {
            inner: Arc::new(StoreInner {
                write_tx,
                readers: Mutex::new(Vec::new()),
                reader_permits: Arc::new(Semaphore::new(READER_COUNT)),
                database_path: database_path.to_path_buf(),
            }),
        })
    }

    /// Opens an in-memory store. Tests only — a shared cache keeps every
    /// handle looking at the same database.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "evertranscript-test-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&directory)?;
        Self::open(&directory.join("test.db"))
    }

    pub fn database_path(&self) -> &Path {
        &self.inner.database_path
    }

    /// Runs a mutation on the single writer connection.
    pub async fn write<T, F>(&self, job: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (result_tx, result_rx) = oneshot::channel();
        self.inner
            .write_tx
            .send(Box::new(move |connection| {
                let _ = result_tx.send(job(connection));
            }))
            .await
            .map_err(|_| anyhow::anyhow!("the writer thread is gone"))?;
        result_rx
            .await
            .map_err(|_| anyhow::anyhow!("the write was dropped without a result"))?
    }

    /// Runs a query on a pooled read-only connection.
    pub async fn read<T, F>(&self, job: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let _permit = Arc::clone(&self.inner.reader_permits)
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("the reader pool is closed"))?;

        let pooled = self.inner.readers.lock().await.pop();
        let connection = match pooled {
            Some(connection) => connection,
            None => {
                let connection = Connection::open(&self.inner.database_path)?;
                schema::configure(&connection)?;
                connection.pragma_update(None, "query_only", true)?;
                connection
            }
        };

        let (result, connection) = tokio::task::spawn_blocking(move || {
            let result = job(&connection);
            (result, connection)
        })
        .await
        .context("the read task panicked")?;

        self.inner.readers.lock().await.push(connection);
        result
    }
}

/// Now, in the format the record stores timestamps in.
pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_are_visible_to_readers() {
        let store = Store::open_in_memory().expect("open");
        store
            .write(|connection| {
                connection.execute(
                    "INSERT INTO meetings (id, started_at, created_at, updated_at)
                     VALUES ('m', 'now', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("write");

        let count: i64 = store
            .read(|connection| {
                Ok(connection.query_row("SELECT count(*) FROM meetings", [], |row| row.get(0))?)
            })
            .await
            .expect("read");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn concurrent_reads_do_not_deadlock() {
        let store = Store::open_in_memory().expect("open");
        let mut handles = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store
                    .read(|connection| {
                        Ok(
                            connection.query_row("SELECT count(*) FROM meetings", [], |row| {
                                row.get::<_, i64>(0)
                            })?,
                        )
                    })
                    .await
            }));
        }
        for handle in handles {
            handle.await.expect("join").expect("read");
        }
    }

    #[tokio::test]
    async fn a_failed_write_does_not_poison_the_writer() {
        let store = Store::open_in_memory().expect("open");
        let failure = store
            .write(|connection| {
                connection.execute("INSERT INTO nope VALUES (1)", [])?;
                Ok(())
            })
            .await;
        assert!(failure.is_err(), "the bad write should fail");

        store
            .write(|connection| {
                connection.execute(
                    "INSERT INTO meetings (id, started_at, created_at, updated_at)
                     VALUES ('m', 'now', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("the writer must still work");
    }
}
