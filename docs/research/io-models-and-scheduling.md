# I/O Models and Task Scheduling for S3 Clients

Notes on how different I/O models apply to our S3 client workload,
and what each library actually uses under the hood.

---

## The Workload: What Are We Scheduling?

An S3 client's hot path is:

```
1. Build HTTP request (CPU: sign, serialize — microseconds)
2. Send request bytes over TCP (I/O: network write)
3. Wait for response (I/O: network read — the dominant cost)
4. Parse response (CPU: deserialize — microseconds)
```

Steps 2-3 are 99%+ of wall time. This is an **I/O-bound workload** —
CPU barely matters. The bottleneck is network round-trip time and
bandwidth, not computation.

For a multipart upload (640 parts × 8 MiB):
```
Total data: 5 GB
Network RTT: ~1 ms (same-AZ)
Per-part time: ~50-100 ms (8 MiB at 10 Gbps + RTT)
Sequential: 640 × 75 ms = 48 seconds
With 16 concurrent: 640/16 × 75 ms = 3 seconds
```

The question: what's the best I/O model to drive those 16 concurrent HTTP
requests?

---

## I/O Model Comparison

### Model 1: Thread-per-request (boto3)

```
Thread pool (10 threads)
├── Thread 1: blocking HTTP send/recv for part 1
├── Thread 2: blocking HTTP send/recv for part 2
├── ...
└── Thread 10: blocking HTTP send/recv for part 10
```

**How it works:** Each thread calls `urllib3.request()` which blocks on
socket I/O. The OS kernel handles multiplexing — each thread sleeps in
`epoll_wait` until data arrives on its socket.

**Pros:**
- Simple mental model — one thread, one connection, one part
- No callback hell, no async complexity
- Python's GIL doesn't matter because threads release GIL during I/O

**Cons:**
- Thread creation/switching overhead (~10-50 µs per context switch)
- Stack memory: ~8 MB per thread × 10 threads = 80 MB for pool
- Doesn't scale to 1000s of concurrent requests (thread per connection)
- Latency: context switch adds jitter

**When it's fine:** max_concurrency ≤ 100. For S3 clients, this is typical.
boto3's model works because S3 concurrency is limited (10-50, not 10000).

### Model 2: Epoll event loop (Go, Netty, Node.js)

```
Single event loop thread
├── epoll_wait() → which sockets are ready?
├── Socket A readable → read part 1 response chunk
├── Socket B writable → write part 2 request chunk
├── Socket C readable → read part 3 response chunk
└── No more ready sockets → epoll_wait() again
```

**How it works:** One thread manages all connections via the OS's
multiplexing facility (epoll on Linux, kqueue on macOS). When data
arrives on any socket, the event loop processes it and advances the
corresponding request's state machine.

**Go's variant:** goroutines look like threads to the programmer but the
runtime schedules them onto a pool of OS threads (GOMAXPROCS, default =
CPU count). The Go runtime's netpoller uses epoll under the hood. A
goroutine doing `conn.Read()` is actually suspended on the netpoller and
resumed when data arrives — no OS thread is blocked.

**Java Netty's variant:** Event loop group (typically 2 × CPU cores threads)
each running an epoll event loop. Channels are assigned to event loops.
All I/O for a channel happens on its assigned event loop thread — no locking
needed. AWS Java SDK v2's async client uses Netty (`NettyNioAsyncHttpClient`).

**Node.js variant:** Single-threaded event loop (libuv) using epoll/kqueue.
All I/O is non-blocking. JavaScript callbacks fire when data arrives.
`@aws-sdk/client-s3` runs in this model.

**Pros:**
- Scales to 100K+ concurrent connections on one machine
- No per-connection thread overhead
- Low latency — no context switch, just function dispatch

**Cons:**
- Must not block the event loop — a slow callback delays ALL connections
- More complex programming model (callbacks, promises, async/await)
- Harder to debug (stack traces across async boundaries)

### Model 3: Async/await with work-stealing (Tokio — what we use)

```
Tokio runtime (multi-threaded, N worker threads)
├── Worker 1: polls futures from its local queue
│   ├── Future A: upload_part() awaiting send()
│   └── Future B: range_get() awaiting recv()
├── Worker 2: polls futures from its local queue
│   └── Future C: stat() awaiting recv()
├── Worker 3: idle — steals Future D from Worker 1's queue
└── Global queue: overflow futures that no worker has claimed
```

**How it works:** Tokio's scheduler maintains a per-thread local FIFO queue
and a shared global injection queue. When a future calls `.await`, it yields
control. Tokio's reactor (backed by mio, which uses epoll/kqueue) resumes the
future when I/O is ready. If a worker thread's queue is empty, it steals from
another worker's queue — this is the work-stealing scheduler.

Under the hood:
```
tokio::spawn(async { client.put_object(...).await })
  → puts a Task on the scheduler
  → a worker thread polls the task's future
  → future calls hyper::conn::send_request()
  → hyper internally calls mio::Poll (epoll_wait)
  → kernel notifies when socket is writable/readable
  → mio wakes the future
  → worker thread resumes polling
```

**This is what our crate uses** — via `aws-sdk-s3` (which uses hyper, which
uses tokio, which uses mio, which uses epoll/kqueue).

**Pros:**
- Best of both worlds: epoll efficiency + goroutine-like programming model
- Work-stealing balances load across CPU cores automatically
- Zero-cost futures: no heap allocation until `.await` suspends
- Scales to millions of concurrent tasks on a handful of threads
- Async/await syntax is close to synchronous code

**Cons:**
- `Send + Sync` bounds everywhere (can't hold non-Send types across .await)
- Colored function problem (async infects the call chain)
- Debugging: async stack traces are fragmented

### Model 4: io_uring (Linux 5.1+, bleeding edge)

```
Submission Queue (SQ) ←── user space pushes I/O requests
        ↕ shared memory ring buffer
Completion Queue (CQ) ──→ user space polls completed I/O

No system calls for submission or completion (when using SQPOLL).
Kernel processes I/O requests asynchronously from its own thread.
```

**How it works:** Instead of `read(fd, buf, len)` syscalls, the application
writes I/O descriptors into a memory-mapped ring buffer (SQ). The kernel
picks them up asynchronously, performs the I/O, and puts completions into
another ring buffer (CQ). The application polls CQ without syscalls.

**For S3 clients specifically:**
- io_uring's value is in reducing syscall overhead for high-IOPS workloads
  (storage, not network)
- For network I/O, the gain over epoll is marginal because network
  operations already have high latency (~1 ms RTT) that dominates syscall
  overhead (~1 µs)
- The main benefit would be batching: submit 16 send() + 16 recv() as one
  batch instead of 32 individual syscalls

**Current state in Rust S3 ecosystem:**
- **tokio-uring**: exists but experimental, single-threaded only
- **monoio**: io_uring-based runtime, used by some storage systems
- **glommio**: io_uring-based, focus on direct I/O for storage
- **No S3 client uses io_uring today** — hyper/reqwest/opendal all use
  epoll via mio

**Would io_uring help our S3 client?**
Probably not significantly. Our bottleneck is:
1. Network bandwidth (saturating NIC) — io_uring doesn't increase bandwidth
2. Concurrent request management — Tokio's scheduler already does this well
3. Connection setup (TLS handshake) — io_uring doesn't help with TLS

Where io_uring WOULD help: if our backend stores data on NVMe SSDs and we
want to read/write files with direct I/O during upload/download. But that's
the storage layer, not the S3 HTTP client layer.

---

## What Each S3 Library Actually Uses

| Library | I/O Model | Runtime | Network Backend | Syscall Facility |
|---|---|---|---|---|
| **Our crate (aws)** | Async/await + work-stealing | Tokio multi-thread | hyper → mio | epoll/kqueue |
| **Our crate (opendal)** | Async/await + work-stealing | Tokio multi-thread | reqwest → hyper → mio | epoll/kqueue |
| **boto3** | Thread pool + blocking I/O | OS threads | urllib3 → socket | select/poll |
| **Go s3manager** | Goroutines + netpoller | Go runtime (M:N) | net/http → netpoller | epoll/kqueue |
| **Java CRT** | Event loop + native threads | AWS CRT (C library) | s2n-tls + custom HTTP | epoll (Linux) |
| **Java async (Netty)** | Event loop group | Netty (NIO) | Netty channels | epoll/NIO selector |
| **JS SDK** | Single-threaded event loop | Node.js (libuv) | http/https module | epoll/kqueue |
| **object_store** | Async/await + work-stealing | Tokio | reqwest → hyper → mio | epoll/kqueue |
| **MinIO Go** | Goroutines + netpoller | Go runtime (M:N) | net/http → netpoller | epoll/kqueue |

---

## Which Model Is Best for Our Use Case?

### Our workload characteristics:
- ~10-50 concurrent HTTP connections (not 100K)
- Large payloads (8-64 MiB per request)
- Network-bandwidth-bound (saturating 25-100 Gbps NIC)
- Mixed small (HEAD, LIST) and large (multipart parts) requests
- Need predictable latency (ML training pipeline)

### Analysis:

**Thread-per-request (boto3):** Would work fine at our concurrency levels.
10-50 threads is trivial. But we'd lose Rust's zero-cost async and the
entire Tokio ecosystem. No reason to use this model in Rust.

**Epoll event loop (single-threaded):** Insufficient — a single thread
can't push 25+ Gbps. At 64 MiB chunks, each request involves significant
memcpy to/from kernel buffers. One thread becomes CPU-bound on the memory
copies.

**Tokio async/await (what we use):** Correct choice. Multi-threaded
work-stealing scheduler distributes the memcpy work across cores. hyper
efficiently manages HTTP/1.1 connection pipelining and HTTP/2 multiplexing.
Mio efficiently multiplexes the 10-50 sockets.

**io_uring:** Marginal gain for network I/O at our concurrency levels. The
syscall overhead saved (~1 µs per call) is negligible compared to the
per-request latency (~50-100 ms). Would only matter if we were doing
>100K requests/second with tiny payloads, which is not our workload.

### Verdict: **Tokio is the right choice.**

The scheduling hierarchy for our crate:

```
Level 1: Our semaphore pools (read/write split)
  → Controls how many operations can be in-flight
  → FIFO ordering, timeout, queue depth metrics

Level 2: Tokio work-stealing scheduler
  → Distributes I/O work across CPU cores
  → Polls futures when I/O is ready via mio/epoll

Level 3: hyper connection pool
  → Manages TCP connections per-origin
  → HTTP/1.1 keep-alive, connection reuse

Level 4: OS kernel (epoll/kqueue)
  → Multiplexes socket readiness
  → Delivers I/O completion notifications to mio
```

Our semaphore pools (Level 1) are the admission controller — they decide
WHAT gets to run. Tokio (Level 2) handles HOW it runs efficiently. hyper
(Level 3) manages the TCP lifecycle. The kernel (Level 4) does the actual I/O.

Each level does one thing. No level duplicates another's job. KISS.

---

## Future Consideration: io_uring for File I/O

If we add streaming file upload/download later (read from disk → send to S3),
io_uring could help on the **disk I/O** side:

```
Current (Tokio fs): read(fd, buf) → tokio spawns blocking task → OS thread
With io_uring:      submit read to SQ → kernel reads asynchronously → poll CQ
```

For reading a 50 GB checkpoint from NVMe at 3 GB/s, io_uring's zero-copy
direct I/O could reduce CPU overhead for the file read path. But this only
helps if disk read is the bottleneck (it usually isn't — network is slower).

Not needed now. File it under "interesting optimization for storage-side."
