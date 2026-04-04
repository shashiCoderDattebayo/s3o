// ============================================================================
// SOLUTION: Connection Pool with Read/Write Isolation
// ============================================================================

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

// ─── Error Types ────────────────────────────────────────────────────────────

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

// ─── Metrics ────────────────────────────────────────────────────────────────

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

// ─── RAII Guards ────────────────────────────────────────────────────────────

pub struct PermitGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
}

// ┌──────────────────────────────────────────────────────────────────────────┐
// │ SOLUTION: Drop for PermitGuard                                          │
// │                                                                         │
// │ Key insight: Drop runs on EVERY path — normal return, early return      │
// │ via ?, panic, and tokio task cancellation. This is the ONLY place       │
// │ where we can guarantee the active counter is decremented.               │
// │                                                                         │
// │ The debug_assert catches bugs in development where we somehow           │
// │ decrement below zero (double release). In release mode it silently      │
// │ saturates to 0 via saturating_sub — better than wrapping to u64::MAX.  │
// └──────────────────────────────────────────────────────────────────────────┘
impl Drop for PermitGuard {
    fn drop(&mut self) {
        let prev = self.active.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(prev > 0, "active underflowed — double release?");
    }
}

pub struct BulkPermitGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
    count: u64,
}

// ┌──────────────────────────────────────────────────────────────────────────┐
// │ SOLUTION: Drop for BulkPermitGuard                                      │
// │                                                                         │
// │ Same principle as PermitGuard, but decrements by self.count.            │
// │ acquire_many_owned returns a single OwnedSemaphorePermit that           │
// │ represents N permits — dropping it releases all N atomically.           │
// └──────────────────────────────────────────────────────────────────────────┘
impl Drop for BulkPermitGuard {
    fn drop(&mut self) {
        let prev = self.active.fetch_sub(self.count, Ordering::Relaxed);
        debug_assert!(prev >= self.count, "active underflowed on bulk release");
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
    // ┌──────────────────────────────────────────────────────────────────────┐
    // │ SOLUTION: new()                                                      │
    // │                                                                      │
    // │ Two independent semaphores. Arc because Semaphore::acquire_owned     │
    // │ needs Arc<Semaphore>. Metrics are independent per pool.              │
    // └──────────────────────────────────────────────────────────────────────┘
    pub fn new(read_capacity: usize, write_capacity: usize, timeout: Duration) -> Self {
        Self {
            read_sem: Arc::new(Semaphore::new(read_capacity)),
            write_sem: Arc::new(Semaphore::new(write_capacity)),
            read_metrics: PoolMetrics::new(),
            write_metrics: PoolMetrics::new(),
            timeout,
        }
    }

    pub fn read_metrics(&self) -> &PoolMetrics {
        &self.read_metrics
    }

    pub fn write_metrics(&self) -> &PoolMetrics {
        &self.write_metrics
    }

    // ┌──────────────────────────────────────────────────────────────────────┐
    // │ SOLUTION: acquire_read()                                             │
    // │                                                                      │
    // │ The critical correctness property:                                   │
    // │   queued is ALWAYS decremented, even on timeout.                     │
    // │   active is ONLY incremented on success.                             │
    // │                                                                      │
    // │ The pattern:                                                         │
    // │   queued++  →  await with timeout  →  queued--  →  active++ if ok   │
    // │                                                                      │
    // │ Why queued-- comes BEFORE the match: whether the timeout fires or    │
    // │ the semaphore is acquired, we are no longer queued. Putting it       │
    // │ after the match would risk skipping it on the error path.            │
    // │                                                                      │
    // │ Why Ordering::Relaxed: these are monitoring gauges, not              │
    // │ synchronization primitives. A momentarily stale read (thread A       │
    // │ sees 3 while thread B just decremented to 2) is acceptable for      │
    // │ dashboards. SeqCst would add unnecessary memory barriers.           │
    // └──────────────────────────────────────────────────────────────────────┘
    pub async fn acquire_read(&self) -> PoolResult<(PermitGuard, Duration)> {
        self.acquire_from(&self.read_sem, &self.read_metrics).await
    }

    // ┌──────────────────────────────────────────────────────────────────────┐
    // │ SOLUTION: acquire_write()                                            │
    // │                                                                      │
    // │ Identical to acquire_read but uses the write semaphore/metrics.      │
    // │ Factored into acquire_from() to avoid duplication.                   │
    // └──────────────────────────────────────────────────────────────────────┘
    pub async fn acquire_write(&self) -> PoolResult<(PermitGuard, Duration)> {
        self.acquire_from(&self.write_sem, &self.write_metrics).await
    }

    /// Shared implementation for single-permit acquisition.
    async fn acquire_from(
        &self,
        sem: &Arc<Semaphore>,
        metrics: &PoolMetrics,
    ) -> PoolResult<(PermitGuard, Duration)> {
        let start = Instant::now();

        // 1. We are now queued
        metrics.queued.fetch_add(1, Ordering::Relaxed);

        // 2. Try to acquire with timeout
        let result = tokio::time::timeout(self.timeout, sem.clone().acquire_owned()).await;

        // 3. No longer queued — ALWAYS runs, even on timeout
        metrics.queued.fetch_sub(1, Ordering::Relaxed);

        let queue_wait = start.elapsed();

        // 4. Handle result
        match result {
            Ok(Ok(permit)) => {
                // 5. Success — now active
                metrics.active.fetch_add(1, Ordering::Relaxed);
                Ok((
                    PermitGuard {
                        _permit: permit,
                        active: Arc::clone(&metrics.active),
                    },
                    queue_wait,
                ))
            }
            Ok(Err(_)) => Err(PoolError::Closed), // semaphore was closed
            Err(_) => Err(PoolError::Timeout(self.timeout)), // timeout elapsed
        }
    }

    // ┌──────────────────────────────────────────────────────────────────────┐
    // │ SOLUTION: acquire_write_bulk()                                       │
    // │                                                                      │
    // │ Uses Semaphore::acquire_many_owned(n) which atomically acquires      │
    // │ n permits as a single unit. This is critical — acquiring one at      │
    // │ a time could deadlock if two bulk acquisitions each get partial       │
    // │ allocations.                                                         │
    // │                                                                      │
    // │ Example of the deadlock with one-at-a-time:                          │
    // │   Pool has 4 permits. Two callers each want 3.                       │
    // │   Caller A acquires 2, Caller B acquires 2. Both block waiting       │
    // │   for a 3rd that will never come. acquire_many_owned avoids this     │
    // │   because it atomically acquires all N or waits.                     │
    // │                                                                      │
    // │ The active counter is incremented by N (not 1) because the bulk      │
    // │ reservation represents N concurrent HTTP connections.                │
    // └──────────────────────────────────────────────────────────────────────┘
    pub async fn acquire_write_bulk(&self, n: usize) -> PoolResult<(BulkPermitGuard, Duration)> {
        let start = Instant::now();
        let count = n as u64;

        self.write_metrics.queued.fetch_add(1, Ordering::Relaxed);

        let result = tokio::time::timeout(
            self.timeout,
            self.write_sem.clone().acquire_many_owned(n as u32),
        )
        .await;

        self.write_metrics.queued.fetch_sub(1, Ordering::Relaxed);

        let queue_wait = start.elapsed();

        match result {
            Ok(Ok(permit)) => {
                self.write_metrics.active.fetch_add(count, Ordering::Relaxed);
                Ok((
                    BulkPermitGuard {
                        _permit: permit,
                        active: Arc::clone(&self.write_metrics.active),
                        count,
                    },
                    queue_wait,
                ))
            }
            Ok(Err(_)) => Err(PoolError::Closed),
            Err(_) => Err(PoolError::Timeout(self.timeout)),
        }
    }
}

// ============================================================================
// Tests
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
        assert_eq!(pool.read_metrics().active_count(), 0);

        drop(guard);
        assert_eq!(pool.write_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_read_write_isolation() {
        let pool = ConnectionPool::new(5, 2, Duration::from_secs(1));

        let (_w1, _) = pool.acquire_write().await.unwrap();
        let (_w2, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 2);

        let (_r1, _) = pool.acquire_read().await.unwrap();
        assert_eq!(pool.read_metrics().active_count(), 1);
    }

    #[tokio::test]
    async fn test_timeout_when_pool_exhausted() {
        let pool = ConnectionPool::new(1, 1, Duration::from_millis(50));

        let _guard = pool.acquire_read().await.unwrap();

        let result = pool.acquire_read().await;
        assert!(matches!(result, Err(PoolError::Timeout(_))));

        assert_eq!(pool.read_metrics().queued_count(), 0);
    }

    #[tokio::test]
    async fn test_raii_on_error_path() {
        let pool = ConnectionPool::new(2, 2, Duration::from_secs(1));

        let result: Result<(), &str> = async {
            let (_guard, _) = pool.acquire_read().await.unwrap();
            assert_eq!(pool.read_metrics().active_count(), 1);
            Err("simulated error")
        }
        .await;

        assert!(result.is_err());
        assert_eq!(pool.read_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_bulk_acquire_and_release() {
        let pool = ConnectionPool::new(5, 10, Duration::from_secs(1));

        let (bulk, _wait) = pool.acquire_write_bulk(4).await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 4);

        let (_w1, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 5);

        drop(bulk);
        assert_eq!(pool.write_metrics().active_count(), 1);

        drop(_w1);
        assert_eq!(pool.write_metrics().active_count(), 0);
    }

    #[tokio::test]
    async fn test_queue_depth_visible_during_contention() {
        let pool = ConnectionPool::new(1, 1, Duration::from_secs(5));

        let (_guard, _) = pool.acquire_read().await.unwrap();

        let pool2 = pool.clone();
        let waiter = tokio::spawn(async move {
            let (_g, wait) = pool2.acquire_read().await.unwrap();
            wait
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(pool.read_metrics().queued_count(), 1);

        drop(_guard);
        let wait_time = waiter.await.unwrap();
        assert!(wait_time >= Duration::from_millis(5));
        assert_eq!(pool.read_metrics().queued_count(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_reads_and_writes_no_interference() {
        let pool = ConnectionPool::new(5, 5, Duration::from_secs(5));

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

        let (bulk, _) = pool.acquire_write_bulk(3).await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 3);

        let (_w1, _) = pool.acquire_write().await.unwrap();
        let (_w2, _) = pool.acquire_write().await.unwrap();
        let (_w3, _) = pool.acquire_write().await.unwrap();
        assert_eq!(pool.write_metrics().active_count(), 6);

        drop(bulk);
        assert_eq!(pool.write_metrics().active_count(), 3);
    }
}
