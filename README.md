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
s3.op.latency{op=put,status=ok}     — wall clock time including queue wait
s3.op.duration{op=put,status=ok}    — backend time excluding queue wait
s3.op.queue_wait{op=put}            — time spent waiting for a pool permit
s3.pool.active{pool=read}           — permits currently held
s3.pool.queued{pool=write}          — callers waiting for a permit
s3.op.err{op=get,kind=not_found}    — errors by classification
s3.op.cancelled{op=get}             — operations dropped mid-flight
s3.http.duration{op=put_part}       — per-HTTP-call latency
```

## Design

See [`docs/design.md`](docs/design.md) for architecture, concurrency model, and extensibility notes.

See [`docs/literature-survey.md`](docs/literature-survey.md) for comparison with boto3, Go s3manager, Java CRT, OpenDAL, object_store, and MinIO.

## License

Apache-2.0
