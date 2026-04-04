# Interview Question: Connection Pool with Read/Write Isolation

**Duration**: 45 minutes
**Level**: Senior / Staff Engineer
**Topics**: Concurrency, async Rust, RAII, backpressure, resource scheduling

---

## Problem Statement

You are building a connection pool for a storage client that talks to a remote
backend (like S3). The client handles two types of operations:

- **Reads**: GET, HEAD, LIST — latency-sensitive, high volume
- **Writes**: PUT, DELETE — throughput-sensitive, sometimes large (multipart)

Your team has decided on a **read/write split pool** design:

```
┌─────────────────────────────────────────────┐
│              ConnectionPool                  │
│                                             │
│  read_pool:  Semaphore(read_capacity)       │
│  write_pool: Semaphore(write_capacity)      │
│                                             │
│  Reads acquire from read_pool               │
│  Writes acquire from write_pool             │
│  Reads never block writes, writes never     │
│  block reads                                │
└─────────────────────────────────────────────┘
```

### Requirements

1. **Read/write isolation**: reads and writes have separate permit pools
2. **RAII safety**: permits must be released on ALL code paths (success, error, panic, task cancellation)
3. **Timeout**: if a permit isn't available within a deadline, fail fast with an error
4. **Metrics**: track active permits and queue depth per pool, so operators can detect saturation
5. **Bulk reservation**: compound operations (e.g., a multipart upload with 100 parts) should reserve N permits upfront and hold them for the entire operation, so parts don't re-queue between batches

### What You Need to Implement

The template provides the struct definitions, metric helpers, and test harness.
You need to implement **four functions**:

1. `ConnectionPool::new()` — construct the pool
2. `ConnectionPool::acquire_read()` — acquire one read permit (returns RAII guard + wait time)
3. `ConnectionPool::acquire_write()` — acquire one write permit
4. `ConnectionPool::acquire_write_bulk()` — reserve N write permits at once for compound ops

### Evaluation Criteria

- **Correctness**: permits always released, gauges always accurate, no deadlocks
- **Safety**: RAII guard handles all code paths including `?` and task abort
- **Timeout handling**: fail fast, don't leave gauges in inconsistent state
- **Metrics accuracy**: active count and queue depth are always consistent
- **Code clarity**: clean, idiomatic async Rust

### Bonus (if time permits)

- Explain why `Ordering::Relaxed` is sufficient for the atomic counters
- What happens if `acquire_write_bulk(n)` is called where `n > write_capacity`?
- How would you add work-stealing (reads borrow idle write capacity)?

---

## API Reference (provided in template)

```rust
// You will use these types:
tokio::sync::Semaphore           // FIFO semaphore
tokio::sync::OwnedSemaphorePermit // permit that can be moved into structs
tokio::time::timeout              // wraps a future with a deadline
std::sync::atomic::AtomicU64     // lock-free counter
std::time::{Duration, Instant}   // timing
```
