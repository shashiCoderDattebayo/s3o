# Literature Survey: S3 Client Libraries

Source-verified analysis of 7 S3 client libraries across 4 languages.
Every claim verified from cloned source code (repos listed below).

---

## Libraries Analyzed

| Library | Language | Repo | Version |
|---|---|---|---|
| aws-sdk-s3 | Rust | aws-sdk-rust (cloned) | 1.x |
| OpenDAL | Rust | opendal (cloned) | 0.51 |
| object_store | Rust | arrow-rs-object-store (cloned) | latest |
| boto3/s3transfer | Python | s3transfer (cloned) | latest |
| s3manager | Go | aws-sdk-go-v2 (cloned) | latest |
| CRT S3 Client | Java | aws-sdk-java-v2 (cloned) | 2.x |
| MinIO | Go | minio-go (cloned) | latest |

Detailed per-library research files in `docs/research/`.

---

## Dimensional Comparison (source-verified)

### Concurrency

| Library | Scope | R/W Split | Backpressure | Queue Visibility |
|---|---|---|---|---|
| **Ours** | Global R/W split pools | **Yes** | FIFO + timeout | Gauge per pool |
| boto3 | Global 10 threads (manager.py:59) | No | Thread pool blocks | None |
| Go s3manager | Per-transfer 5 goroutines (upload.go:37) | No | None — unbounded | None |
| Java CRT | Global auto-tuned from 10 Gbps (S3NativeClientConfiguration.java:49) | No | Internal adaptive | None |
| object_store | Optional LimitStore semaphore (limit.rs:32) | No | Blocks indefinitely | None |
| OpenDAL | Dual semaphores: op-level + HTTP-level | No | Blocks indefinitely (custom semaphore trait allows timeout) | `opendal_operation_executing` gauge (in-flight, not queue) |
| MinIO Go | Per-upload 4 goroutines (constants.go:58) | No | None — unbounded | None |

**Finding:** No library provides read/write isolation. No library provides queue depth visibility. No library combines a global connection ceiling with bulk reservation for compound operations.

### Metrics

| Library | Latency/Duration Split | Queue Wait | TTFB | Per-HTTP Metrics | Cancellation | Metrics SPI |
|---|---|---|---|---|---|---|
| **Ours** | **Yes** | **Yes** | No* | Yes | **Yes** | `metrics` crate |
| boto3 | No | No | No | No | No | None |
| Go s3manager | No | No | No | No | No | None |
| Java CRT | No | No | **Yes** (CoreMetric.java) | Yes (per-attempt in MetricCollection) | No | **MetricPublisher** — the only working AWS SDK metrics SPI |
| object_store | No | No | No | No | No | None |
| OpenDAL | No (single duration) | No | **Yes** (`opendal_operation_ttfb_seconds`) | **Yes** (9 HTTP metrics) | No | **MetricsIntercept** — 5 backend implementations |
| MinIO Go | No | No | No | No | No | None |

*TTFB and bytes rate are gaps we should add (~20 LoC).

**Finding:** Java CRT and OpenDAL have the best built-in metrics. However, neither separates queue wait from backend time — the core diagnostic that lets operators distinguish "client saturated" from "backend slow."

### Transfers

| Library | Auto Multipart | Auto Range Download | Abort on Failure | Throughput Preset |
|---|---|---|---|---|
| **Ours** | Explicit create/part/complete | HEAD + parallel range GETs | Explicit + error log | `max_throughput()` |
| boto3 | Yes (8 MB threshold, manager.py:57) | Yes (range tasks in shared pool, download.py:488) | Yes | None |
| Go s3manager | Yes (5 MB, upload.go:29) | Yes (5 MB ranges, download.go:24) | Yes | None |
| Java CRT | Yes (CRT internal, 8 MB min) | Yes (CRT auto byte-ranges) | CRT handles | `targetThroughputInGbps` |
| object_store | Manual (put_multipart_opts) | get_ranges with coalescing (1 MB gap, 10 parallel — util.rs:92) | Yes | None |
| OpenDAL | Writer internal (chunk + concurrent) | No built-in | Writer Drop cleanup | None |
| MinIO Go | Yes (16 MB threshold, constants.go:28) | **No** (single GET, api-get-object-file.go:31) | Yes | None |

### Error Classification

| Library | Categories | Retryability Signal | Source |
|---|---|---|---|
| **Ours** | 9 ErrorKind | Kind-based | HTTP status mapping from 5 SdkError variants |
| boto3 | Exception hierarchy | Exception type | ClientError, BotoCoreError, etc. |
| Go s3manager | Wrapped errors | errors.As() type assertion | StatusCode + Code string |
| Java CRT | Exception hierarchy | Exception class | S3Exception with code/message |
| object_store | Generic Error enum | No retryability signal | Status code if available |
| OpenDAL | **12 ErrorKind** + 3 ErrorStatus | `is_temporary()` method | `#[non_exhaustive]` enum (error.rs:51-89) |
| MinIO Go | ErrorResponse struct | String Code matching | Code, Message, StatusCode fields |

### Extensibility for xDS / Custom Infrastructure

| Library | Custom HTTP | Header Injection | Dynamic Endpoint | p2c Path | TCP Tuning |
|---|---|---|---|---|---|
| **Ours (AWS SDK)** | Stable `HttpClient` trait | `modify_before_transmit` interceptor (15 LoC) | `ResolveEndpoint` per-request | In resolver | Custom hyper connector |
| OpenDAL | `HttpFetch` in `raw::` (unstable), via stable `HttpClientLayer` | Custom HttpFetch wrapper (15-20 LoC) | **Fixed at build time** | Must rebuild Operator | reqwest builder (easy) |
| boto3 | Custom Session adapter | before-send event | Per-request endpoint_url | Must build externally | urllib3 settings |
| Go s3manager | Custom RoundTripper | RoundTripper wrapper | EndpointResolverV2 (client-level) | Must build externally | http.Transport Dialer |
| Java CRT | CRT internal | ExecutionInterceptor | Per-request override | Must build externally | CRT internal |

**Finding:** AWS SDK Rust is the only library with a per-request endpoint resolver + stable interceptor pipeline + injectable HTTP client — the three hooks needed for xDS, traceparent propagation, and p2c load balancing.

### Data Integrity (gap in our crate)

| Library | Upload Checksum | Download Verify |
|---|---|---|
| **Ours** | Not implemented (easy to add) | None |
| boto3 | CRC32/CRC32C/SHA1/SHA256 | Validates if server returns |
| Go s3manager | Trailing checksum | Validates if present |
| Java CRT | Auto CRC32C | CRT validates |
| MinIO Go | Auto MD5 per part | Validates Content-MD5 |

---

## Key Takeaways

1. **No library does R/W split pools.** This is our unique contribution.
2. **No library separates queue wait from duration.** This is our key diagnostic.
3. **OpenDAL has the richest built-in metrics** (17 metrics, TTFB, bytes rate) but lacks queue wait decomposition.
4. **Java CRT is the only SDK with working metrics SPI.** Rust SDK's is TODO.
5. **AWS SDK Rust has the best extensibility** for our xDS/tracing roadmap.
6. **We should add:** TTFB metric, bytes rate metric, upload checksums (CRC32C). ~30 LoC total.
