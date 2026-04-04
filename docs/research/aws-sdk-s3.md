# AWS SDK for Rust -- S3 Client Analysis

## 1. Concurrency Model

The AWS SDK for Rust provides no built-in concurrency limiter. The SDK creates a hyper-based HTTP client (`Connector` in `aws-smithy-http-client/src/client.rs`) with its own connection pool, but exposes no semaphore or admission control mechanism. Each `send().await` call independently opens or reuses a connection from the hyper connection pool. When 100 concurrent callers each invoke PutObject, all 100 proceed simultaneously -- the only backstop is hyper's internal idle connection pool limit configured via `pool_idle_timeout` in the `ConnectorBuilder`. There is no concept of operation-level or HTTP-level concurrency caps in the SDK itself.

**Source:** `sdk/aws-smithy-http-client/src/client.rs` -- `ConnectorBuilder` struct and `wrap_connector()` method.

## 2. Connection Pooling

The SDK uses hyper's connection pool via `hyper_util::client::legacy::Builder`. The `ConnectorBuilder` in `aws-smithy-http-client` exposes `enable_tcp_nodelay(bool)` (default `true` as set in `Connector::builder()`) and `pool_idle_timeout` for idle connection management. However, it does NOT expose `tcp_keepalive` or active pool size limits through its high-level API. For full TCP tuning (keepalive, max connections per host, etc.), users must build a custom hyper connector and inject it via the `HttpClient` trait from `aws-smithy-runtime-api`. The `wrap_connector()` method accepts any `tower::Service<Uri>` implementation, enabling custom transport injection.

**Source:** `sdk/aws-smithy-http-client/src/client.rs` lines 110-122 (`ConnectorBuilder` struct definition) and lines 186-238 (`wrap_connector()` method).

## 3. Retry

`RetryConfig` in `aws-smithy-types/src/retry.rs` supports three modes: `Standard` (exponential backoff with jitter, default 3 attempts), `Adaptive` (adds client-side throttling, experimental), and disabled (achieved by setting `max_attempts` to 1). Configurable parameters include: `max_attempts` (default 3), `initial_backoff` (default 1 second), `max_backoff` (default 20 seconds), and `reconnect_mode`. The `ReconnectMode::ReconnectOnTransientError` (the default) poisons the in-use connection on transient errors, forcing a new TCP connection on the next retry attempt. `ReconnectMode::ReuseAllConnections` disables this behavior, keeping connections alive even after transient failures.

**Source:** `sdk/aws-smithy-types/src/retry.rs` -- `RetryConfig::standard()` (lines 321-330), `RetryConfigBuilder::build()` (lines 267-281), `ReconnectMode` enum (lines 304-313).

## 4. Metrics

The `aws-smithy-observability` crate defines a `TelemetryProvider` with a `ProvideMeter` trait that vends `Meter` instances. The `Meter` supports creating `MonotonicCounter`, `UpDownCounter`, `Histogram`, and async `Gauge` instruments via the `ProvideInstrument` trait. However, this system is NOT fully integrated into the runtime -- the crate's `lib.rs` contains a TODO comment: "once we have finalized everything and integrated metrics with our runtime libraries update this with detailed usage docs and examples." The default `TelemetryProvider` uses a `NoopMeterProvider`, meaning the S3 client emits zero metrics natively. All observability is the caller's responsibility, typically achieved by implementing custom interceptors.

**Source:** `sdk/aws-smithy-observability/src/lib.rs` line 17 (TODO comment), `sdk/aws-smithy-observability/src/provider.rs` (TelemetryProvider struct), `sdk/aws-smithy-observability/src/instruments.rs` (instrument traits), `sdk/aws-smithy-observability/src/meter.rs` (ProvideMeter trait).

## 5. Interceptors

The `Intercept` trait in `aws-smithy-runtime-api/src/client/interceptors.rs` defines 20 hooks across the full request lifecycle. Key mutable hooks include: `modify_before_serialization` (alter the input), `modify_before_signing` (add auth headers before signing), `modify_before_transmit` (inject custom headers like `traceparent`), `modify_before_deserialization` (alter the raw HTTP response), `modify_before_attempt_completion` (alter the output per-attempt), and `modify_before_completion` (alter the final output). Key read hooks include: `read_before_execution`, `read_before_attempt`, `read_after_attempt` (per-retry metrics), and `read_after_execution` (per-operation metrics). Each hook receives typed context -- for example, `BeforeTransmitInterceptorContextMut` gives mutable access to request headers, while `FinalizerInterceptorContextRef` provides read access to the output or error. The interceptor runs per-attempt, so retries receive fresh header injection.

**Source:** `sdk/aws-smithy-runtime-api/src/client/interceptors.rs` -- `Intercept` trait (line 68), hook method definitions via `interceptor_trait_fn!` macro and direct implementations (lines 89-588).

## 6. Endpoint Resolution

The `ResolveEndpoint` trait in `aws-smithy-runtime-api/src/client/endpoint.rs` is called per-request with `EndpointResolverParams` (a type-erased container holding operation-specific parameters plus arbitrary properties). The `resolve_endpoint()` method returns an `EndpointFuture` that resolves to an `Endpoint` (from `aws-smithy-types`) containing the URI and optional headers/auth config. This is the natural integration point for dynamic endpoint selection such as xDS: a custom resolver can hold `Arc<RwLock<Vec<Endpoint>>>` updated by a gRPC stream, and `resolve_endpoint()` performs p2c or round-robin selection. Since hyper maintains per-origin connection pools, different resolved endpoints get separate connection lifecycles. The `finalize_params()` method allows post-creation mutation of the parameters before resolution.

**Source:** `sdk/aws-smithy-runtime-api/src/client/endpoint.rs` -- `ResolveEndpoint` trait (lines 87-106), `EndpointResolverParams` (lines 37-80), `SharedEndpointResolver` (lines 112-134).

## 7. Error Handling

`SdkError<E, R>` in `aws-smithy-runtime-api/src/client/result.rs` is a `#[non_exhaustive]` enum with 5 variants: `ConstructionFailure` (request failed before being sent), `TimeoutError` (request may or may not have been sent), `DispatchFailure` (HTTP response not received), `ResponseError` (response received but not parseable), and `ServiceError` (typed error from the service plus raw HTTP response). `DispatchFailure` wraps a `ConnectorError` which provides `is_timeout()`, `is_io()`, `is_user()`, and `is_other()` classification methods. `ServiceError` contains the typed error `E` and raw response `R`, accessible via `err()` and `raw()`. The `into_service_error()` method converts any non-service error into an unhandled variant of `E` for ergonomic match patterns.

**Source:** `sdk/aws-smithy-runtime-api/src/client/result.rs` -- `SdkError` enum (lines 320-337), `DispatchFailure` methods (lines 202-237), `ConnectorError` classification (lines 650-676).

## 8. Multipart Upload

The SDK provides raw primitives for multipart upload: `CreateMultipartUpload`, `UploadPart`, `CompleteMultipartUpload`, and `AbortMultipartUpload` as individual operations in the `aws-sdk-s3` crate. There is NO built-in auto-multipart orchestration -- the caller must manage chunking, concurrent part uploads, and abort-on-failure logic. The `aws-sdk-s3` Transfer Manager exists in developer preview but is not stable and is not part of the standard SDK surface. For production use, callers typically implement their own multipart orchestrator using the raw operations.

**Source:** `aws-sdk-s3` operation modules for `CreateMultipartUpload`, `UploadPart`, `CompleteMultipartUpload`, `AbortMultipartUpload`.

## 9. Range Download

GetObject supports the standard HTTP `Range` header for byte-range requests, allowing callers to download specific byte ranges from an object. There is no built-in parallel range download -- the caller must issue a HEAD request to determine the object's size, split the content into byte ranges, fetch them concurrently using multiple GetObject calls with Range headers, and reassemble the results. The Transfer Manager (developer preview) may include range-based parallel download, but this is not part of the stable SDK.

**Source:** `aws-sdk-s3` GetObject operation -- `range()` setter on the fluent builder.

## 10. Data Integrity

The SDK supports `ChecksumAlgorithm` with values CRC32, CRC32C, SHA1, and SHA256 on PutObject and multipart upload operations. When a checksum algorithm is specified on the request, the SDK computes the checksum over the request body and includes it in the request headers. On download (GetObject), the SDK validates checksums if the server returns them in the response headers (`x-amz-checksum-*`). For multipart uploads, per-part checksums are computed and a composite checksum is validated upon completion. This provides end-to-end data integrity verification when both client and server opt in.

**Source:** `aws-sdk-s3` -- `ChecksumAlgorithm` enum, PutObject builder `checksum_algorithm()` setter, `sdk/s3/tests/checksums.rs` for test coverage.
