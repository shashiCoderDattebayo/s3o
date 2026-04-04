# Design

## What this crate is

An opinionated S3 client for our custom S3-compatible object store.
Built on `aws-sdk-s3`. Adds what the SDK doesn't provide: connection
management, observability, and safe defaults.

## Public API

```rust
client.put_object(key, body: Bytes) -> S3Result<()>    // auto-multipart
client.get_object(key) -> S3Result<Bytes>               // auto-range download
client.head_object(key) -> S3Result<ObjectMetadata>
client.delete_object(key) -> S3Result<()>
client.list_objects(prefix) -> S3Result<Vec<ObjectEntry>>
```

5 methods. `Bytes` in, `Bytes` out. Nothing else.

## Architecture

```
S3Client
├── Read Pool (FIFO semaphore, read_connections permits)
│   ├── HEAD, LIST, small GET → acquire(1)
│   └── large GET → acquire_bulk(read_concurrency), hold for entire download
│       └── range parts rotate within reserved permits via buffer_unordered
├── Write Pool (FIFO semaphore, write_connections permits)
│   ├── small PUT, DELETE → acquire(1)
│   └── large PUT → acquire_bulk(write_concurrency), hold for entire upload
│       └── multipart parts rotate within reserved permits via buffer_unordered
├── OpMetrics (per-operation: latency, duration, queue_wait, bytes, errors)
├── Tracing spans (per-operation + per-part)
└── aws-sdk-s3::Client (SigV4, retry, TLS, HTTP, connection pool)
```

## Why each custom piece exists

| File | LoC | What it does | Why SDK doesn't provide it |
|---|---|---|---|
| pool.rs | 160 | Read/write split FIFO semaphores, RAII guards, bulk reservation | No SDK has R/W isolation or bulk reservation |
| metrics.rs | 90 | Latency vs duration split, queue wait, cancellation tracking | SDK metrics SPI is unfinished (TODO in source) |
| client.rs | 384 | Auto-multipart, auto-range download, tracing spans | SDK provides raw primitives only |
| error.rs | 89 | 9-category error classification from SdkError | SDK errors are verbose, need mapping for dashboards |
| config.rs | 168 | Builder with validation, safe defaults, max_throughput preset | SDK has no connection budget or deadlock prevention |
| types.rs | 15 | ObjectMetadata, ObjectEntry | Thin wrappers over SDK response types |
| **Total** | **906** | | |

Everything else is delegated to aws-sdk-s3: SigV4, TLS, HTTP, retry, connection pool.

## Concurrency model

**Read/write split + bulk reservation.**

Why split: uploads never delay downloads. A 5 GB checkpoint save doesn't
slow down HEAD requests from other services.

Why bulk reservation: a 5 GB upload (640 parts) pays ONE queue wait, not 640.
Parts flow at full speed within the reserved permits.

Why FIFO: tokio::Semaphore is FIFO. Callers served in order. No starvation.

Why timeout: no infinite queuing. Fail fast with Timeout error after
`timeout` duration (default 30s).

```
50 concurrent requests, write_connections=10:
  10 execute immediately
  40 queue (FIFO)
  s3.pool.write.queued = 40  ← visible on dashboard
  As each completes → next in queue gets permit
  If queue wait > timeout → fail with Timeout error
```

## Metrics

```
latency  = [── queue_wait ──][── duration ──]
             waiting for        actual S3
             permit             backend work
```

| Metric | Type | What it tells you |
|---|---|---|
| s3.op.latency | Histogram | What the caller experiences |
| s3.op.duration | Histogram | Backend performance (latency minus queue wait) |
| s3.op.queue_wait | Histogram | Client saturation signal |
| s3.pool.{read,write}.active | Gauge | Current utilization |
| s3.pool.{read,write}.queued | Gauge | Backpressure signal |
| s3.op.{total,ok,err,cancelled} | Counter | Success/failure/cancel rates |
| s3.op.bytes_{sent,recv} | Counter | Throughput |
| s3.op.err{kind=...} | Counter | Error breakdown for alerts |
| s3.http.{total,duration} | Histogram | Per-HTTP-call latency |

Diagnosis: if `duration` is stable but `latency` spikes → client saturated (increase connections).
If both spike → backend slow.

## Defaults

| Setting | Default | ML Preset (`max_throughput()`) |
|---|---|---|
| read_connections | 15 | 20 |
| write_connections | 10 | 20 |
| read_concurrency | 4 | 16 |
| write_concurrency | 4 | 16 |
| chunk_size | 8 MiB | 64 MiB |
| timeout | 30s | 5 min |
| max_retries | 3 | 3 |

Config validation prevents deadlock: `read_connections >= read_concurrency + 1`.

## Future extensibility (via AWS SDK hooks)

| Need | AWS SDK hook | Effort |
|---|---|---|
| Trace header injection | `modify_before_transmit` interceptor | 15 LoC |
| xDS endpoint discovery | `ResolveEndpoint` trait (per-request) | ~100 LoC |
| p2c load balancing | Implement in endpoint resolver | ~50 LoC |
| Adaptive retry | `RetryConfig::adaptive()` | 3 LoC |
| Upload checksums | `ChecksumAlgorithm::Crc32C` on SDK calls | 10 LoC |

Zero changes to pool/metrics/client code needed for any of these.
