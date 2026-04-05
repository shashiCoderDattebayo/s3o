# s3o

S3 client for S3-compatible object stores. Built on [`aws-sdk-s3`](https://crates.io/crates/aws-sdk-s3).

Adds read/write connection pool isolation, automatic multipart/range transfers, and per-operation latency/duration metrics.

## Quick start

```rust
use s3o::S3Client;
use bytes::Bytes;

let client = S3Client::builder()
    .bucket("my-bucket")
    .region("us-east-1")
    .access_key_id("AKID")
    .secret_access_key("SECRET")
    .endpoint("https://s3.example.com")
    .path_style(true)
    .build()?;

client.put_object("key", Bytes::from("data")).await?;
let data = client.get_object("key").await?;
```

Objects above `multipart_threshold` (default 8 MiB) are automatically split into parallel multipart uploads / range downloads.

## Features

### Operations

| Operation | Behavior |
|---|---|
| `put_object(key, body)` | Simple PUT for small objects, multipart for large (auto based on threshold) |
| `get_object(key)` | HEAD for size, then simple GET or parallel range download |
| `head_object(key)` | Returns size, content type, etag, last modified |
| `delete_object(key)` | Deletes the object |
| `list_objects(prefix, recursive)` | Non-recursive (immediate children) or recursive (all descendants) |

### Connection management

| Capability | Detail |
|---|---|
| Read/write split pools | Two independent FIFO semaphores. Uploads never delay downloads. |
| Bulk permit reservation | A 5 GB upload (640 parts) pays one queue wait, not 640. Parts flow within reserved slots. |
| Backpressure with timeout | Callers queue in FIFO order. If a permit isn't available within `timeout`, returns error. No infinite blocking. |
| Deadlock prevention | Builder rejects configs where `connections < concurrency + 1`. |
| Queue depth visibility | `s3.pool.{read,write}.queued` gauge shows how many callers are waiting. |

### Observability

| Capability | Detail |
|---|---|
| Latency vs duration | `s3.op.latency` = wall clock (includes queue wait). `s3.op.duration` = backend time (excludes queue wait). Gap = client saturation. |
| Time to first byte | `s3.http.ttfb` = time from request send to response headers. |
| Throughput | `s3.op.bytes_rate` = bytes/sec excluding queue wait. |
| Error classification | 9 categories: network, timeout, auth, not_found, throttled, server_error, client_error, config, other. |
| Cancellation tracking | If a tokio task is dropped mid-operation, `s3.op.cancelled` is incremented. Permits are always released via RAII. |
| Per-HTTP metrics | `s3.http.total` and `s3.http.duration` per individual S3 HTTP call (including multipart parts and range chunks). |

### Data integrity

| Capability | Detail |
|---|---|
| Upload checksums | Opt-in CRC32C via `.upload_checksum(true)`. SDK computes and sends checksum, backend verifies. Off by default to avoid CPU overhead. |

## How the connection pools work

```
                         S3Client
                            |
              .-----------------------------.
              |                             |
         Read Pool (15)                Write Pool (10)
              |                             |
    .---------.---------.         .---------.---------.
    |         |         |         |         |         |
  HEAD(1)  LIST(1)   GET        PUT      DELETE(1) multipart
                      |          |                    |
              size <= threshold?  size <= threshold?   |
              |           |      |           |        |
             yes          no    yes          no       |
              |           |      |           |        |
           simple      reserve  simple    reserve     |
           GET(1)      bulk(4)  PUT(1)    bulk(4)     |
                         |                   |        |
                    .----+----.         .----+----.   |
                    |  |  |  |         |  |  |  |    |
                   range parts        upload parts   |
                   (no re-queue)      (no re-queue)  |

    (1) = acquires 1 permit
    bulk(4) = acquires read_concurrency or write_concurrency permits upfront
    Parts rotate within reserved permits via buffer_unordered.
    Each pool is a FIFO tokio::Semaphore with timeout.
```

## Configuration

All settings have defaults. Override what you need.

```rust
let client = S3Client::builder()
    .bucket("b").region("r")
    .access_key_id("a").secret_access_key("s")
    // Pool sizing
    .read_connections(15)     // permits for reads (HEAD, GET, LIST)
    .write_connections(10)    // permits for writes (PUT, DELETE)
    .read_concurrency(4)     // parallel range GETs per download
    .write_concurrency(4)    // parallel parts per upload
    // Transfer
    .chunk_size(8 * 1024 * 1024)
    .multipart_threshold(8 * 1024 * 1024)
    // Integrity
    .upload_checksum(false)   // set true for CRC32C on uploads
    // Resilience
    .timeout(Duration::from_secs(30))
    .max_retries(3)
    .build()?;
```

For large-file workloads (ML checkpoints):

```rust
let client = S3Client::builder()
    .max_throughput()  // 64 MiB chunks, 16x concurrency, 5 min timeout
    .bucket("checkpoints").region("r")
    .access_key_id("a").secret_access_key("s")
    .build()?;
```

The builder rejects invalid configs at build time (e.g. `read_connections < read_concurrency + 1`).

## Metrics

Uses the [`metrics`](https://crates.io/crates/metrics) facade. Install any recorder.

```
s3.op.latency{op=put,status=ok}     — wall clock including queue wait
s3.op.duration{op=put,status=ok}    — backend time excluding queue wait
s3.op.queue_wait{op=put}            — time waiting for a pool permit
s3.op.bytes_rate{op=put}            — throughput in bytes/sec
s3.pool.active{pool=read}           — permits currently held
s3.pool.queued{pool=write}          — callers waiting for a permit
s3.op.err{op=get,kind=not_found}    — errors by classification
s3.op.cancelled{op=get}             — operations dropped mid-flight
s3.http.duration{op=put_part}       — per-HTTP-call total time
s3.http.ttfb{op=get}                — time to first byte
```

## Roadmap

Not implemented yet. Each has a clear integration point via the AWS SDK.

| Feature | Integration point | When |
|---|---|---|
| Trace header propagation (traceparent, x-request-id) | `modify_before_transmit` interceptor on the SDK config. ~15 lines. | When distributed tracing is set up across the storage stack. |
| xDS endpoint discovery | `ResolveEndpoint` trait — called per-request by the SDK. Resolver holds endpoints from a gRPC stream. | When we move from DNS round-robin to xDS control plane. |
| p2c / least-connections load balancing | Implement selection logic inside the endpoint resolver. | After xDS, when we have multiple backend endpoints. |
| Health-aware routing | Resolver tracks health per endpoint, excludes unhealthy from selection. | After p2c. |
| Adaptive retry (client-side throttling) | `RetryConfig::adaptive()` — 3 lines to switch on. | When backend reports throttling and we want the client to self-limit. |

None of these require changes to pool, metrics, or client code.

## Design

See [`docs/design.md`](docs/design.md) for architecture, concurrency model, and queue depth trade-offs.

See [`docs/literature-survey.md`](docs/literature-survey.md) for source-verified comparison with boto3, Go s3manager, Java CRT, OpenDAL, object_store, and MinIO.

## License

Apache-2.0
