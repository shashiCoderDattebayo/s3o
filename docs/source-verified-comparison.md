# Source-Verified Dimensional Comparison

Every entry verified from actual source code across 9 cloned repositories.
Corrections from web-search-only version noted where applicable.

---

## Concurrency

| Dimension | s3-client-aws (ours) | s3-client-opendal (ours) | boto3/s3transfer | Go s3manager | Java CRT | object_store | OpenDAL | MinIO Go |
|---|---|---|---|---|---|---|---|---|
| **Scope** | Global R/W split semaphores | Global R/W split semaphores | Global shared thread pool, `max_request_concurrency=10` (manager.py:59) | Per-transfer: `DefaultUploadConcurrency=5` goroutines per Upload() call (upload.go:37). Not shared between calls. | Global auto-tuned: `defaultTargetThroughput=10Gbps` (S3NativeClientConfiguration.java:49), optional `maxConcurrency` cap (defaults to 0 = auto) | Per-LimitStore tokio::Semaphore (limit.rs:32). One permit per ObjectStore trait method call. | Dual: operation semaphore + optional HTTP semaphore via `ConcurrentLimitLayer`. Plus per-writer `concurrent(N)`. Custom semaphore via `ConcurrentLimitSemaphore` trait. | Per-upload: `totalWorkers=4` (constants.go:58) goroutines. No global limiter. |
| **R/W isolation** | Yes — separate pools | Yes — separate pools | No — one pool | No — no global pool | No — one auto-tuned pool | No — one semaphore | No — shared semaphores | No — per-upload only |
| **Backpressure** | FIFO queue + timeout → error | FIFO queue + timeout → error | Thread pool blocks caller (BoundedExecutor) | None — goroutines accumulate | Internal adaptive, opaque | Semaphore blocks, no timeout (limit.rs:61 — bare `.acquire().await`) | Semaphore blocks indefinitely; custom semaphore trait allows user timeout | None — goroutines accumulate |
| **Queue visibility** | `s3.pool.{read,write}.queued` gauge | Same | None | None | None | None | `opendal_operation_executing` gauge (in-flight count, not queue depth) | None |
| **Deadlock prevention** | Config rejects `conn < conc + 1` | Same | N/A | N/A | N/A | N/A | N/A (custom semaphore could add) | N/A |
| **Bulk reservation** | `acquire_many` for compound ops — parts don't re-queue | Same for downloads; single permit for uploads (opendal internal) | No — each part task enters thread pool independently | N/A — per-transfer pool naturally holds goroutines | N/A — CRT schedules internally | No — one permit per put_multipart_opts, parts free-ride | No — one operation permit; writer manages parts internally | N/A — per-transfer pool |

## Metrics

| Dimension | s3-client-aws | s3-client-opendal | boto3 | Go s3manager | Java CRT | object_store | OpenDAL | MinIO Go |
|---|---|---|---|---|---|---|---|---|
| **Latency vs duration split** | Yes — separate histograms | Yes | No | No | No | No | No — single `operation_duration_seconds` conflates queue + work | No |
| **Queue wait** | `s3.op.queue_wait` histogram | Same | Not measurable | Not measurable | Not measurable | Not measurable | Not measured | Not measurable |
| **Per-op metrics** | total, ok, err, cancelled, bytes_sent, bytes_recv (6 counters) | Same | None | None | 21 metrics via CoreMetric.java: `API_CALL_DURATION`, `RETRY_COUNT`, `TIME_TO_FIRST_BYTE`, `READ_THROUGHPUT`, `WRITE_THROUGHPUT`, `ERROR_TYPE`, etc. | None | 8 op-level: duration, ttfb, bytes, bytes_rate, entries, entries_rate, errors_total, executing | None |
| **Per-HTTP metrics** | `s3.http.total`, `s3.http.duration` | Same | None | None | Per-attempt metrics in MetricCollection (attempt duration, status) | None | 9 HTTP-level: request/response duration, bytes, bytes_rate, executing, connection_errors, status_errors | None |
| **Cancellation tracking** | `s3.op.cancelled` counter via Drop | Same | No | No | No | No | No | No |
| **Metrics SPI** | `metrics` crate facade (pluggable) | Same | None | None | `MetricPublisher` interface — FUNCTIONAL. Built-in CloudWatch publisher. This is the ONLY AWS SDK with working metrics SPI. | None | `MetricsIntercept` trait — fully overridable. 5 backend crates: metrics, prometheus, prometheus-client, otel, fastmetrics. | None |
| **TTFB** | Not measured | Not measured | No | No | Yes — `TIME_TO_FIRST_BYTE` in CoreMetric.java | No | Yes — `opendal_operation_ttfb_seconds` | No |

## Transfers

| Dimension | s3-client-aws | s3-client-opendal | boto3 | Go s3manager | Java CRT | object_store | OpenDAL | MinIO Go |
|---|---|---|---|---|---|---|---|---|
| **Auto multipart** | Explicit create/part/complete when body > threshold | opendal Writer handles internally | Yes — threshold 8 MB (manager.py:57) | Yes — Uploader auto-splits at PartSize 5 MB (upload.go:29) | Yes — CRT auto-splits, min part 8 MB (S3NativeClientConfiguration.java:47) | Manual — put_multipart_opts returns handle | Yes — Writer with chunk()+concurrent() | Yes — auto at `minPartSize=16MB` (constants.go:28) |
| **Auto range download** | HEAD + parallel range GETs via buffer_unordered | Same | Yes — splits into range tasks submitted to shared pool (download.py:488-505) | Yes — Downloader splits into PartSize ranges, Concurrency=5 (download.go:24,28). Bypassed if user sets Range header. | Yes — CRT auto byte-ranges | `get_ranges()` with coalesce (1 MB gap, 10 parallel fetches — util.rs:92,95) | No — single read() call | No — single GetObject stream (api-get-object-file.go:31-80) |
| **Abort on failure** | Explicit AbortMultipartUpload + error log | Writer Drop cleanup | Yes | Yes | CRT handles | Yes | Writer Drop | Yes |
| **Throughput preset** | `max_throughput()`: 64 MiB chunks, 16x concurrency | Same | None | None | `targetThroughputInGbps` auto-tunes everything | None | None | None |
| **Memory model** | write_concurrency × chunk_size (Bytes::slice zero-copy) | Same for downloads | Tag semaphores: 10 × chunksize per direction (manager.py:66-67) | Concurrency × PartSize per transfer (upload.go:37,29) | CRT internal — native memory outside JVM heap. Known OOM at high throughput targets. | Implementation-dependent | writer concurrent(N) × chunk(M) internal | totalWorkers × partSize per upload |

## Error Handling

| Dimension | s3-client-aws | s3-client-opendal | boto3 | Go s3manager | Java CRT | object_store | OpenDAL | MinIO Go |
|---|---|---|---|---|---|---|---|---|
| **Classification** | 9 ErrorKind categories from 5 SdkError variants | 9 ErrorKind from 12 opendal ErrorKind variants | Exception hierarchy (ClientError, BotoCoreError, etc.) | error wrapping — `errors.As()` for type assertion | S3Exception with code + SdkServiceException/SdkClientException hierarchy | Generic Error enum | 12 ErrorKind variants + 3 ErrorStatus (Permanent/Temporary/Persistent). `#[non_exhaustive]`. | ErrorResponse struct with string Code |
| **Retryability signal** | Our ErrorKind + SDK RetryConfig | Our ErrorKind + opendal ErrorStatus.is_temporary() | Retry mode logic checks status codes | SDK retry handler checks error type | CRT internal retry classification | No built-in | `ErrorStatus::Temporary` → retry; `Permanent`/`Persistent` → no retry | max 10 retries, not user-configurable |
| **RAII safety** | PermitGuard Drop + debug_assert | Same | GC | Goroutine scoped | JVM GC | No RAII guard on LimitStore semaphore | Layer-managed | Goroutine scoped |

## Extensibility

| Dimension | s3-client-aws | s3-client-opendal | boto3 | Go s3manager | Java CRT | object_store | OpenDAL |
|---|---|---|---|---|---|---|---|
| **Custom HTTP transport** | Stable `HttpClient` trait (aws-smithy-runtime-api) | `HttpFetch` trait in `raw::`, injected via stable `HttpClientLayer` | Custom urllib3 adapter | Custom `http.RoundTripper` | CRT internal — no user swap | No — reqwest hardcoded | `HttpFetch` trait + `HttpClientLayer` (stable layer API) |
| **Request interceptors** | 20 hooks via `Intercept` trait across 10 lifecycle stages | None — custom HttpFetch is the only hook | Event system: before-send, before-sign, etc. Note: provide-client-params doesn't fire for TransferManager (issue #1535) | RoundTripper wrapping (single hook) | ExecutionInterceptor interface | None | None — HttpFetch wrapping only |
| **Header injection** | `modify_before_transmit` — mutable header access, ~15 LoC | Custom HttpFetch wrapper — mutate Request<Buffer> headers, ~15-20 LoC | before-send event hook | RoundTripper wrapper | ExecutionInterceptor | Not possible | Custom HttpFetch wrapper |
| **Dynamic endpoints** | `ResolveEndpoint` trait — per-request | Fixed at build time | Per-request via endpoint_url param | EndpointResolverV2 — configured at client build, called per-request with params | Per-request endpoint override | Fixed at build time | Fixed at build time (ENDPOINT_TEMPLATES resolved once) |
| **xDS path** | Endpoint resolver with `Arc<RwLock<>>` — natural fit | Must rebuild Operator | Must rebuild Session | EndpointResolverV2 — similar to AWS Rust | Must rebuild | Must rebuild | Must rebuild Operator |
| **TCP tuning** | ConnectorBuilder: `tcp_nodelay` (default true). Keepalive/buffers need custom connector. | reqwest builder: `tcp_keepalive()`, `tcp_nodelay()`, `connect_timeout()`, `pool_max_idle_per_host()` — all one-liners | urllib3 adapter settings | `http.Transport` DialContext with custom `net.Dialer` | CRT internal | `ClientOptions` on builder | reqwest builder (same as our opendal crate) |
| **API stability** | Stable 1.x | HttpFetch in `raw::` (unstable); HttpClientLayer in `layers::` (stable) | Stable | Stable | Stable | Stable | Mixed — raw unstable, layers stable |

## Data Integrity (discovered dimension)

| Dimension | s3-client-aws | s3-client-opendal | boto3 | Go s3manager | Java CRT | object_store | OpenDAL | MinIO Go |
|---|---|---|---|---|---|---|---|---|
| **Upload checksum** | Not implemented (SDK supports CRC32/CRC32C/SHA1/SHA256 if configured) | Not implemented | Supports ChecksumAlgorithm | Trailing checksum via middleware | CRT auto-computes CRC32C | Checksum config on builder | Service-dependent | Auto-MD5 on parts |
| **Download verify** | None | None | Validates if server returns checksum | Validates if present | CRT validates CRC32C | Validates if present | None | Validates Content-MD5 |

---

## Corrections Applied From Source Verification

| Library | Previous claim | Actual (from source) | File:Line |
|---|---|---|---|
| MinIO Go | `minPartSize = 5 MiB` | `minPartSize = 16 MiB` threshold, `absMinPartSize = 5 MiB` hard minimum | constants.go:24,28 |
| JS SDK | No global connection limit | `maxSockets` exists in Smithy node-http-handler (external dep, not in this repo) | — |
| JS SDK | `@aws/s3-managed-downloader` exists | NOT in aws-sdk-js-v3 repo. May exist as separate awslabs project but not verified from source. | — |
| JS SDK | partSize default = 5 MB static | Minimum 5 MB but dynamically scales to `totalBytes/10000` for large files | Upload.ts:111-112 |
| Java CRT | maxConcurrency has a default | Defaults to 0 (auto-calculated from throughput target) | S3NativeClientConfiguration.java:100 |
| Go SDK | "custom endpoint per-call" | Endpoint configured at client level via EndpointResolverV2, not per-API-call | — |
| boto3 | "no parallel download" | DOES parallel range download for large objects | download.py:488-505 |
| OpenDAL | "6 ErrorKind variants" | 12 variants + 3 ErrorStatus states | error.rs:51-89 |
| OpenDAL | "MetricsLayer per-op only" | 17 metrics: 8 op-level + 9 HTTP-level | observe-metrics-common |
| OpenDAL | "TracingLayer per-op only" | Also http::fetch span per HTTP request | tracing/src/lib.rs:185-195 |
