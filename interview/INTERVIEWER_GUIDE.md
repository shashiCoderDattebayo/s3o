# Interviewer Guide

## Setup (2 min)

Give the candidate:
- `PROBLEM.md` — read time
- `template.rs` — their working file
- `Cargo.toml` — so they can run `cargo test`

Do NOT give them `solution.rs`.

## Timeline (45 min)

| Time | Phase | What to look for |
|---|---|---|
| 0-5 | Read problem statement | Do they ask clarifying questions? |
| 5-10 | Implement `Drop` for both guards | Do they understand RAII? Use debug_assert? |
| 10-15 | Implement `new()` | Trivial — should be fast |
| 15-30 | Implement `acquire_read/write` | **Core of the interview** — see rubric below |
| 30-40 | Implement `acquire_write_bulk` | Extension of the pattern with acquire_many |
| 40-45 | Discussion / bonus questions | Architecture thinking |

## What Makes This Hard

The `acquire_read/write` function looks simple but has a subtle correctness requirement:

```
queued++ → await semaphore (may timeout) → queued-- → active++ (only on success)
```

**Common mistakes**:
1. Forgetting to decrement `queued` on the timeout path → gauge drifts forever
2. Incrementing `active` before checking if the acquire succeeded → gauge inflated
3. Using `?` on the timeout result before decrementing `queued` → same as #1
4. Not handling the `Err` from `acquire_owned()` (semaphore closed) → panic at runtime

The correct pattern requires the `queued` decrement to be **unconditional** — it must run whether the semaphore was acquired or the timeout fired. This is the same pattern as a `try/finally` in other languages.

## Scoring Rubric

### Drop implementations (10 points)
| Points | Criteria |
|---|---|
| 10 | Correct `fetch_sub`, `debug_assert` for underflow, bulk decrements by `count` |
| 7 | Correct `fetch_sub` but missing debug_assert |
| 3 | Uses some manual mechanism instead of atomic |
| 0 | Forgets to decrement |

### acquire_read/write (35 points)
| Points | Criteria |
|---|---|
| 35 | Correct ordering (queued++ → await → queued-- → active++ on success only), timeout handled, returns queue_wait duration |
| 25 | Correct but doesn't factor into shared `acquire_from()` (duplication is ok for interview) |
| 15 | Queued decrement happens but on wrong path (e.g., only on success) |
| 5 | Acquires permit but no metrics tracking |
| 0 | Doesn't compile or deadlocks |

### acquire_write_bulk (15 points)
| Points | Criteria |
|---|---|
| 15 | Uses `acquire_many_owned`, increments active by N, BulkPermitGuard stores count |
| 10 | Works but acquires permits one at a time (potential deadlock) |
| 5 | Correct concept but wrong API usage |
| 0 | Not attempted |

### Code quality (10 points)
| Points | Criteria |
|---|---|
| 10 | Clean, idiomatic Rust, proper error handling, factored code |
| 7 | Works but verbose/repetitive |
| 3 | Works but hard to read |

### Tests pass (10 points)
| Points | Criteria |
|---|---|
| 10 | All 9 tests pass |
| 7 | 7-8 tests pass |
| 3 | 4-6 tests pass |
| 0 | Fewer than 4 pass |

### Bonus discussion (20 points)
| Points | Criteria |
|---|---|
| 5 | Explains why Relaxed ordering is sufficient (monitoring gauges, not synchronization) |
| 5 | Identifies that `acquire_write_bulk(n)` where `n > capacity` will wait forever (or timeout) — suggests validation |
| 5 | Proposes work-stealing design (try primary pool, fall back to secondary) |
| 5 | Discusses trade-offs: bulk reservation vs flat FIFO, per-transfer vs global pools |

**Total: 80 base + 20 bonus = 100 points**

### Hiring bar
| Score | Signal |
|---|---|
| 70+ | Strong hire — understands concurrency, RAII, async, metrics |
| 50-69 | Hire — solid fundamentals, minor gaps |
| 30-49 | Weak hire — gets the concept but implementation has bugs |
| <30 | No hire — doesn't understand the concurrency model |

## Discussion Questions (5-10 min at end)

### Q1: Why Relaxed ordering?
**Expected**: "These are counters for monitoring dashboards. A momentarily stale value (one thread sees 3 while another just decremented to 2) is fine for metrics. We're not using these atomics for synchronization between threads — the semaphore handles that. `SeqCst` or `AcqRel` would add unnecessary memory barriers."

**Strong signal**: mentions happens-before relationships, knows what a memory barrier is.

### Q2: What if `acquire_write_bulk(n)` where `n > write_capacity`?
**Expected**: "`acquire_many_owned(n)` will wait forever because the semaphore will never have `n` permits available simultaneously. With our timeout, it will eventually return `PoolError::Timeout`. We should validate `n <= write_capacity` at call time and return an error immediately."

**Strong signal**: mentions the deadlock scenario with partial allocation if we acquired one-at-a-time instead.

### Q3: How would you add work-stealing?
**Expected**: "If the read pool is full but the write pool has idle permits, allow reads to borrow from the write pool. Implementation: try `read_sem` with a very short timeout, if it fails try `write_sem`. But this adds complexity — the borrowed permit needs to be returned to the right pool. An alternative: one shared overflow pool that both sides can borrow from."

**Strong signal**: discusses fairness implications, return-to-correct-pool problem, mentions that this breaks isolation guarantees.

### Q4 (Architecture): How does this compare to boto3's concurrency model?
**Expected**: "boto3 uses a single global thread pool (`max_concurrency=10`) shared by all operations. There's no read/write separation. A multipart upload's parts compete with small GETs. Our model isolates reads and writes — a flood of uploads can't delay GETs."

**Strong signal**: knows that per-transfer concurrency (Go model) vs global pool (boto3 model) is a fundamental design choice, can articulate trade-offs.
