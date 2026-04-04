# Go aws-sdk-go-v2 s3manager -- S3 Client Analysis

## 1. Concurrency Model

The s3 manager package (`github.com/aws/aws-sdk-go-v2/feature/s3/manager`) uses **per-transfer** concurrency, not global concurrency. The `Uploader` struct has a `Concurrency` field (default `DefaultUploadConcurrency` = **5**) that controls how many goroutines upload parts in parallel for a **single** `Upload()` call. Similarly, the `Downloader` struct has a `Concurrency` field (default `DefaultDownloadConcurrency` = **5**) that controls how many goroutines download byte-ranges in parallel for a **single** `Download()` call. Each call to `Upload()` or `Download()` spawns its own set of goroutines independently.

There is **no global concurrency limiter** built into the manager. If application code calls `Upload()` concurrently from 10 goroutines, each with `Concurrency=5`, the result is 50 concurrent part-upload goroutines making HTTP requests simultaneously. The only implicit limit on outbound connections is the `http.Transport`'s `MaxIdleConnsPerHost` (default 2 in the standard library, but the SDK sets its own default) and `MaxConnsPerHost`. Users who need a global concurrency ceiling must implement their own semaphore or use a custom `http.RoundTripper` wrapper.

> **Sources:** [Go Packages -- feature/s3/manager](https://pkg.go.dev/github.com/aws/aws-sdk-go-v2/feature/s3/manager), [AWS SDK for Go V2 S3 Utilities](https://aws.github.io/aws-sdk-go-v2/docs/sdk-utilities/s3/), [AWS Developer Guide -- S3 Utilities](https://docs.aws.amazon.com/sdk-for-go/v2/developer-guide/sdk-utilities-s3.html)

---

## 2. Memory Model

Memory consumption per upload is approximately `PartSize * Concurrency`. The default `PartSize` is `DefaultUploadPartSize` = **5 MB** (`5,242,880` bytes), so with the default `Concurrency` of 5, each concurrent `Upload()` call buffers approximately **25 MB** in memory. If an application runs 100 goroutines each calling `Upload()` with default settings, the buffer memory alone is approximately 2.5 GB. For `io.Reader` sources (non-seekable), the SDK must buffer the full part contents in memory before uploading, making the memory cost unavoidable.

The `BufferProvider` field on the `Uploader` allows custom memory allocation strategies. The built-in `NewBufferedReadSeekerWriteToPool(size)` creates a pool of reusable byte buffers of the given size, reducing GC pressure. For body values that implement `io.ReaderAt` (e.g., `*os.File`), the Uploader can read directly from the source at different offsets without buffering entire parts, significantly reducing memory usage. Users can implement the `ReadSeekerWriteToProvider` interface to provide their own buffer pooling strategy.

> **Sources:** [Go Packages -- manager.Uploader](https://pkg.go.dev/github.com/aws/aws-sdk-go-v2/feature/s3/manager), [AWS SDK Go V2 Discussion #1650 -- memory improvements](https://github.com/aws/aws-sdk-go-v2/discussions/1650), [AWS SDK Go V2 Issue #482 -- reduce downloader memory](https://github.com/aws/aws-sdk-go-v2/issues/482)

---

## 3. Retry

The s3 manager delegates retry behavior to the underlying AWS SDK client, which uses the `retry.Standard` retryer by default. The standard retryer implements truncated exponential backoff with jitter, using a default of `MaxAttempts` = **3** (total attempts, including the initial request). The maximum backoff delay is **20 seconds**, consistent across AWS SDKs. Retry is applied at the **per-HTTP-request** level, not per-part or per-transfer -- if a single `UploadPart` HTTP request fails, only that request is retried up to `MaxAttempts`, not the entire multipart upload.

If a part upload exhausts all retries and fails permanently, the `Upload()` call returns an error, but the multipart upload is **not** automatically aborted (unlike boto3's s3transfer). Users must handle `AbortMultipartUpload` themselves if they want to clean up incomplete uploads. The SDK provides helper functions `retry.AddWithMaxAttempts()` and `retry.AddWithMaxBackoffDelay()` to customize retry behavior on a per-client basis. The `retry.Standard` retryer also implements a rate-limiting token bucket to prevent retry storms during sustained throttling from the service.

> **Sources:** [Retries and Timeouts -- AWS SDK for Go V2](https://aws.github.io/aws-sdk-go-v2/docs/configuring-sdk/retries-timeouts/), [Go Packages -- aws/retry](https://pkg.go.dev/github.com/aws/aws-sdk-go-v2/aws/retry), [AWS SDK Go V2 Developer Guide -- retries](https://docs.aws.amazon.com/sdk-for-go/v2/developer-guide/configure-retries-timeouts.html)

---

## 4. Endpoint Resolution

Endpoint resolution in aws-sdk-go-v2 is handled through the `EndpointResolverV2` interface, which is invoked **per-request** with operation-specific parameters (region, FIPS settings, dual-stack, etc.). The `BaseEndpoint` client option allows specifying a base hostname (e.g., a local MinIO instance or a VPC endpoint), and this value is passed as a parameter to `EndpointResolverV2.ResolveEndpoint()` on every request, where the resolver can inspect and potentially modify it. The resolution v2 system replaced the deprecated `EndpointResolver` (v1) interface.

However, `BaseEndpoint` and `EndpointResolverV2` are configured at **client construction time**. You cannot change the resolver or base endpoint on a per-call basis without constructing a new client instance. The `EndpointParameters` struct is populated automatically by the SDK for each request and includes fields like `Region`, `Bucket`, `ForcePathStyle`, `Accelerate`, `UseGlobalEndpoint`, and `DisableMultiRegionAccessPoints`. Custom resolvers implementing `EndpointResolverV2` receive these parameters and return a `smithyendpoints.Endpoint` with a URI and optional headers/properties.

> **Sources:** [Configuring Client Endpoints -- AWS SDK for Go V2](https://aws.github.io/aws-sdk-go-v2/docs/configuring-sdk/endpoints/), [AWS Developer Guide -- Configure Endpoints](https://docs.aws.amazon.com/sdk-for-go/v2/developer-guide/configure-endpoints.html)

---

## 5. Downloader

The `Downloader` splits large objects into byte-range GET requests of `PartSize` bytes (default `DefaultDownloadPartSize` = **5 MB**) and downloads them concurrently using `Concurrency` goroutines (default **5**). The caller must provide an `io.WriterAt` (typically an `*os.File`) so that downloaded ranges can be written to the correct byte offset in parallel, enabling concurrent disk writes without sequential coordination. The `aws.WriteAtBuffer` type is provided for in-memory downloads.

If the caller specifies a `Range` header in the `GetObjectInput`, the Downloader performs a **single** `GetObject` request with that range and ignores the `Concurrency` setting entirely. This is documented behavior: providing a Range input parameter disables the parallel download optimization. The Downloader first issues a `GetObject` for the first part to determine the `Content-Length` of the full object (from the `Content-Range` response header), then calculates the remaining byte ranges and dispatches them concurrently. The minimum allowed part size is **5 MB**.

> **Sources:** [Go Packages -- manager.Downloader](https://pkg.go.dev/github.com/aws/aws-sdk-go-v2/feature/s3/manager), [AWS SDK for Go V2 S3 Utilities](https://aws.github.io/aws-sdk-go-v2/docs/sdk-utilities/s3/), [AWS Developer Guide -- S3 Utilities](https://docs.aws.amazon.com/sdk-for-go/v2/developer-guide/sdk-utilities-s3.html)

---

## 6. Metrics

The aws-sdk-go-v2 SDK provides **no built-in metrics SPI**. There are no counters, histograms, timers, or tracing spans emitted by the SDK or the s3 manager package. Unlike the AWS SDK for Java v2 (which has a functional `MetricPublisher` interface and CloudWatch integration), the Go v2 SDK has no equivalent metrics abstraction. There is no OpenTelemetry integration, no StatsD integration, and no Prometheus integration provided out of the box.

All observability must be implemented externally by the user. The recommended approach is to wrap the `http.RoundTripper` used by the SDK's HTTP client with a custom implementation that records request duration, status codes, retries, and bytes transferred. The SDK's middleware stack also allows inserting custom `middleware.SerializeMiddleware` or `middleware.DeserializeMiddleware` handlers, but these are intended for request/response manipulation, not metrics collection.

> **Sources:** [Go Packages -- feature/s3/manager](https://pkg.go.dev/github.com/aws/aws-sdk-go-v2/feature/s3/manager), [AWS SDK Go V2 GitHub repository](https://github.com/aws/aws-sdk-go-v2)

---

## 7. Error Handling

Errors from the aws-sdk-go-v2 S3 client are wrapped with contextual information using the `smithy` error hierarchy. The primary mechanism for inspecting errors is `errors.As()` from the Go standard library. `smithy.APIError` provides `ErrorCode()`, `ErrorMessage()`, and `ErrorFault()` methods for programmatic error classification. For HTTP-level details, `awshttp.ResponseError` (from `github.com/aws/aws-sdk-go-v2/aws/transport/http`) exposes `ServiceRequestID()` and the underlying HTTP status code. S3-specific errors can be extracted via `s3.ResponseError`, which adds `ServiceHostID()`.

The SDK also generates typed error structs for modeled S3 error responses, such as `types.NoSuchKey`, `types.NoSuchBucket`, and `types.BucketAlreadyExists` (in `github.com/aws/aws-sdk-go-v2/service/s3/types`). These can be matched with `errors.As()`. Operation-level context is available through `smithy.OperationError`, which provides `Service()`, `Operation()`, and `Unwrap()` methods. However, not all S3 error codes have corresponding typed structs -- some error codes (e.g., `NoSuchTagSet`) require string-matching on `ErrorCode()` via the `smithy.APIError` interface, as documented in [GitHub issue #2878](https://github.com/aws/aws-sdk-go-v2/issues/2878).

> **Sources:** [Handling Errors -- AWS SDK for Go V2](https://aws.github.io/aws-sdk-go-v2/docs/handling-errors/), [AWS Developer Guide -- Handle Errors](https://docs.aws.amazon.com/sdk-for-go/v2/developer-guide/handle-errors.html), [GitHub Issue #2878 -- missing error types](https://github.com/aws/aws-sdk-go-v2/issues/2878)
