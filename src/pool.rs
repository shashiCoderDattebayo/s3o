//! Read/write split connection pool.
//!
//! Two independent FIFO semaphores so reads and writes don't contend.
//! Compound operations (multipart, range) use [`acquire_bulk`] to reserve
//! N permits upfront.
use crate::error::{ErrorKind, S3Error, S3Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
fn gauge(name: &'static str, pool: &'static str, value: u64) {
    metrics::gauge!(name, "pool" => pool).set(value as f64);
}
pub(crate) struct Permit {
    _inner: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
    pool: &'static str,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let prev = self.active.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(prev > 0);
        gauge("s3.pool.active", self.pool, prev - 1);
    }
}
pub(crate) struct BulkPermit {
    _inner: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
    pool: &'static str,
    count: u64,
}
impl Drop for BulkPermit {
    fn drop(&mut self) {
        let prev = self.active.fetch_sub(self.count, Ordering::Relaxed);
        debug_assert!(prev >= self.count);
        gauge("s3.pool.active", self.pool, prev - self.count);
    }
}
#[derive(Clone)]
struct HalfPool {
    sem: Arc<Semaphore>,
    active: Arc<AtomicU64>,
    queued: Arc<AtomicU64>,
    name: &'static str,
    timeout: Duration,
}
impl HalfPool {
    fn new(capacity: usize, name: &'static str, timeout: Duration) -> Self {
        Self {
            sem: Arc::new(Semaphore::new(capacity)),
            active: Arc::new(AtomicU64::new(0)),
            queued: Arc::new(AtomicU64::new(0)),
            name,
            timeout,
        }
    }
    async fn acquire(&self) -> S3Result<(Permit, Duration)> {
        let start = Instant::now();
        self.queued.fetch_add(1, Ordering::Relaxed);
        gauge(
            "s3.pool.queued",
            self.name,
            self.queued.load(Ordering::Relaxed),
        );
        let result = tokio::time::timeout(self.timeout, self.sem.clone().acquire_owned()).await;
        let q = self.queued.fetch_sub(1, Ordering::Relaxed);
        gauge("s3.pool.queued", self.name, q.saturating_sub(1));
        let permit = result
            .map_err(|_| {
                S3Error::backend(ErrorKind::Timeout, format!("{} pool timeout", self.name))
            })?
            .map_err(|_| S3Error::config("pool closed"))?;
        let a = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        gauge("s3.pool.active", self.name, a);
        Ok((
            Permit {
                _inner: permit,
                active: Arc::clone(&self.active),
                pool: self.name,
            },
            start.elapsed(),
        ))
    }
    async fn acquire_bulk(&self, n: usize) -> S3Result<(BulkPermit, Duration)> {
        let start = Instant::now();
        self.queued.fetch_add(1, Ordering::Relaxed);
        gauge(
            "s3.pool.queued",
            self.name,
            self.queued.load(Ordering::Relaxed),
        );
        let result =
            tokio::time::timeout(self.timeout, self.sem.clone().acquire_many_owned(n as u32)).await;
        let q = self.queued.fetch_sub(1, Ordering::Relaxed);
        gauge("s3.pool.queued", self.name, q.saturating_sub(1));
        let permit = result
            .map_err(|_| {
                S3Error::backend(
                    ErrorKind::Timeout,
                    format!("{} pool bulk({n}) timeout", self.name),
                )
            })?
            .map_err(|_| S3Error::config("pool closed"))?;
        let count = n as u64;
        let a = self.active.fetch_add(count, Ordering::Relaxed) + count;
        gauge("s3.pool.active", self.name, a);
        Ok((
            BulkPermit {
                _inner: permit,
                active: Arc::clone(&self.active),
                pool: self.name,
                count,
            },
            start.elapsed(),
        ))
    }
}
#[derive(Clone)]
pub(crate) struct Pool {
    read: HalfPool,
    write: HalfPool,
}
impl Pool {
    pub fn new(read_connections: usize, write_connections: usize, timeout: Duration) -> Self {
        Self {
            read: HalfPool::new(read_connections, "read", timeout),
            write: HalfPool::new(write_connections, "write", timeout),
        }
    }
    /// Single read permit (HEAD, LIST, small GET).
    pub async fn read(&self) -> S3Result<(Permit, Duration)> {
        self.read.acquire().await
    }
    /// Single write permit (small PUT, DELETE).
    pub async fn write(&self) -> S3Result<(Permit, Duration)> {
        self.write.acquire().await
    }
    /// Reserve N read permits for a range download.
    pub async fn read_bulk(&self, n: usize) -> S3Result<(BulkPermit, Duration)> {
        self.read.acquire_bulk(n).await
    }
    /// Reserve N write permits for a multipart upload.
    pub async fn write_bulk(&self, n: usize) -> S3Result<(BulkPermit, Duration)> {
        self.write.acquire_bulk(n).await
    }
}
