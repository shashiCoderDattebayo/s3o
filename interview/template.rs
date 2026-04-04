// ============================================================================
// Interview Template: Connection Pool with Read/Write Isolation
//
// Implement the four TODO functions. Everything else is provided.
// Run with: cargo test -- --nocapture
// ============================================================================

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

// ─── Error Types (provided) ─────────────────────────────────────────────────

#[derive(Debug)]
pub enum PoolError {
    Timeout(Duration),
    Closed,
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(d) => write!(f, "permit acquire timed out after {:?}", d),
            Self::Closed => write!(f, "pool is closed"),
        }
    }
}

pub type PoolResult<T> = Result<T, PoolError>;

// ─── Metrics (provided) ─────────────────────────────────────────────────────

/// Tracks active permits and queue depth for one pool (read or write).
#[derive(Clone)]
pub struct PoolMetrics {
    pub active: Arc<AtomicU64>,
    pub queued: Arc<AtomicU64>,
}

impl PoolMetrics {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicU64::new(0)),
            queued: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn active_count(&self) -> u64 {
        self.active.load(Ordering::Relaxed)
    }

    pub fn queued_count(&self) -> u64 {
        self.queued.load(Ordering::Relaxed)
    }
}

// ─── RAII Guards (you implement the Drop) ───────────────────────────────────

/// Holds a single permit. Must decrement `active` on drop.
pub struct PermitGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
}

// TODO 1: Implement Drop for PermitGuard
//   - Decrement the active counter
//   - Use debug_assert to catch underflow
impl Drop for PermitGuard {
    fn drop(&mut self) {
        // ─── YOUR CODE HERE ───
        todo!("Decrement active counter safely")
    }
}

/// Holds N permits for a compound operation (e.g., multipart upload).
pub struct BulkPermitGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
    count: u64,
}

// TODO 2: Implement Drop for BulkPermitGuard
//   - Decrement the active counter by self.count
//   - Use debug_assert to catch underflow
impl Drop for BulkPermitGuard {
    fn drop(&mut self) {
        // ─── YOUR CODE HERE ───
        todo!("Decrement active counter by count safely")
    }
}

// ─── Connection Pool ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ConnectionPool {
    read_sem: Arc<Semaphore>,
    write_sem: Arc<Semaphore>,
    read_metrics: PoolMetrics,
    write_metrics: PoolMetrics,
    timeout: Duration,
}

impl ConnectionPool {
    // TODO 3: Implement new()
    //   - Create two semaphores: one for reads, one for writes
    //   - Initialize metrics for both pools
    pub fn new(read_capacity: usize, write_capacity: usize, timeout: Duration) -> Self {
        // ─── YOUR CODE HERE ───
        todo!("Construct the pool with two separate semaphores")
    }

    pub fn read_metrics(&self) -> &PoolMetrics {
        &self.read_metrics
    }

    pub fn write_metrics(&self) -> &PoolMetrics {
        &self.write_metrics
    }

    // TODO 4: Implement acquire_read()
    //
    // Steps:
    //   1. Increment read_metrics.queued (caller is now waiting)
    //   2. Try to acquire a permit from read_sem with timeout
    //   3. Decrement read_metrics.queued (no longer waiting, whether success or timeout)
    //   4. On timeout → return PoolError::Timeout
    //   5. On success → increment read_metrics.active, return (PermitGuard, queue_wait_duration)
    //
    // Critical: queued must be decremented even if timeout fires.
    // Critical: active must be incremented ONLY on success.
    pub async fn acquire_read(&self) -> PoolResult<(PermitGuard, Duration)> {
        // ─── YOUR CODE HERE ───
        todo!("Acquire a single read permit with timeout and metrics")
    }

    // TODO 5: Implement acquire_write()
    //   Same as acquire_read() but uses write_sem and write_metrics.
    pub async fn acquire_write(&self) -> PoolResult<(PermitGuard, Duration)> {
        // ─── YOUR CODE HERE ───
        todo!("Acquire a single write permit with timeout and metrics")
    }

    // TODO 6: Implement acquire_write_bulk()
    //
    // Reserves N permits at once for a compound operation.
    // Uses Semaphore::acquire_many_owned(n) to atomically acquire N permits.
    // Returns a BulkPermitGuard that releases all N on drop.
    //
    // Same metrics pattern as acquire_write, but:
    //   - active is incremented by N (not 1)
    //   - BulkPermitGuard stores the count for proper decrement
    pub async fn acquire_write_bulk(&self, n: usize) -> PoolResult<(BulkPermitGuard, Duration)> {
        // ─── YOUR CODE HERE ───
        todo!("Acquire N write permits atomically with timeout and metrics")
    }
}

// ============================================================================
// Tests — these must all pass
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_read_acquire_release() {
        let pool = ConnectionPool::new(5, 5, Duration::from_secs(1));

        assert_eq!(pool.read_metrics().active_count(), 0);

        let (guard, _wait) = pool.acquire_read().await.unwrap();
        assert_eq!(pool.read_metrics().active_count(), 1);

        drop(guard);
        assert_eq!(pool.read_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_basic_write_acquire_release() {
        let pool = ConnectionPool::new(5, 5, Duration::from_secs(1));

        let (guard, _wait) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 1);
        assert_eq!(pool.read_metrics().active_count(), 0); // reads unaffected

        drop(guard);
        assert_eq!(pool.write_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_read_write_isolation() {
        // Fill up the write pool completely
        let pool = ConnectionPool::new(5, 2, Duration::from_secs(1));

        let (_w1, _) = pool.acquire_write().await.unwrap();
        let (_w2, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 2);

        // Reads should still work — they use a separate pool
        let (_r1, _) = pool.acquire_read().await.unwrap();
        assert_eq!(pool.read_metrics().active_count(), 1);
    }

    #[tokio::test]
    async fn test_timeout_when_pool_exhausted() {
        let pool = ConnectionPool::new(1, 1, Duration::from_millis(50));

        // Exhaust the read pool
        let _guard = pool.acquire_read().await.unwrap();

        // Second acquire should timeout
        let result = pool.acquire_read().await;
        assert!(matches!(result, Err(PoolError::Timeout(_))));

        // Metrics should be clean after timeout (queued back to 0)
        assert_eq!(pool.read_metrics().queued_count(), 0);
    }

    #[tokio::test]
    async fn test_raii_on_error_path() {
        let pool = ConnectionPool::new(2, 2, Duration::from_secs(1));

        // Simulate an operation that acquires a permit then fails
        let result: Result<(), &str> = async {
            let (_guard, _) = pool.acquire_read().await.unwrap();
            assert_eq!(pool.read_metrics().active_count(), 1);
            Err("simulated error") // guard dropped here via ?-like pattern
        }
        .await;

        assert!(result.is_err());
        // Guard must have been dropped, releasing the permit
        assert_eq!(pool.read_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_bulk_acquire_and_release() {
        let pool = ConnectionPool::new(5, 10, Duration::from_secs(1));

        let (bulk, _wait) = pool.acquire_write_bulk(4).await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 4);

        // Regular writes can still use remaining 6 slots
        let (_w1, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 5);

        drop(bulk);
        assert_eq!(pool.write_metrics().active_count(), 1); // only w1 remains

        drop(_w1);
        assert_eq!(pool.write_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_queue_depth_visible_during_contention() {
        let pool = ConnectionPool::new(1, 1, Duration::from_secs(5));

        // Fill the pool
        let (_guard, _) = pool.acquire_read().await.unwrap();

        // Spawn a waiter — it should be queued
        let pool2 = pool.clone();
        let waiter = tokio::spawn(async move {
            let (_g, wait) = pool2.acquire_read().await.unwrap();
            wait // return how long we waited
        });

        // Give the waiter time to enter the queue
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(pool.read_metrics().queued_count(), 1);

        // Release the first permit — waiter should proceed
        drop(_guard);
        let wait_time = waiter.await.unwrap();
        assert!(wait_time >= Duration::from_millis(5)); // waited at least a bit
        assert_eq!(pool.read_metrics().queued_count(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_reads_and_writes_no_interference() {
        let pool = ConnectionPool::new(5, 5, Duration::from_secs(5));

        // 10 concurrent reads + 10 concurrent writes
        let mut handles = vec![];

        for _ in 0..10 {
            let p = pool.clone();
            handles.push(tokio::spawn(async move {
                let (_g, _) = p.acquire_read().await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }
        for _ in 0..10 {
            let p = pool.clone();
            handles.push(tokio::spawn(async move {
                let (_g, _) = p.acquire_write().await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }

        // All should complete without deadlock
        let timeout = tokio::time::timeout(Duration::from_secs(5), async {
            for h in handles {
                h.await.unwrap();
            }
        })
        .await;

        assert!(timeout.is_ok(), "must not deadlock");
        assert_eq!(pool.read_metrics().active_count(), 0);
        assert_eq!(pool.write_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_bulk_simulates_multipart_upload() {
        let pool = ConnectionPool::new(5, 6, Duration::from_secs(5));

        // Simulate: multipart upload reserves 3 write slots, uploads 20 parts
        let (bulk, _) = pool.acquire_write_bulk(3).await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 3);

        // Meanwhile, 3 more write slots are available for other ops
        let (_w1, _) = pool.acquire_write().await.unwrap();
        let (_w2, _) = pool.acquire_write().await.unwrap();
        let (_w3, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 6);

        // 7th write should timeout (pool full)
        let result = ConnectionPool::new(5, 6, Duration::from_millis(50));
        // (we already have 6 active, pool is 6)

        drop(bulk); // release 3
        assert_eq!(pool.write_metrics().active_count(), 3);
    }
}
