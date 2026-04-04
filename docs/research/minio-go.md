# MinIO Go SDK -- S3 Client Analysis

## 1. Concurrency Model

The MinIO Go SDK (`github.com/minio/minio-go/v7`) controls per-upload concurrency through the `NumThreads` field on `PutObjectOptions`. If `NumThreads` is not set (or is 0), the SDK defaults to the `totalWorkers` constant, which is **4** in current versions of the library (defined in `api-put-object.go`; earlier versions used a value of 3). The `getNumThreads()` helper method checks `opts.NumThreads > 0` and falls back to `totalWorkers` if not set. Each `FPutObject()` or `PutObject()` call spawns up to `NumThreads` goroutines for parallel part uploads.

There is **no global concurrency limiter** in the MinIO SDK. If N concurrent `FPutObject()` calls are in flight, the total goroutine count is up to `N * NumThreads`. For seekable inputs (files, `io.ReaderAt` implementations), each goroutine reads from a different file offset in parallel using `putObjectMultipartStreamFromReadAt()`. For non-seekable streams (plain `io.Reader`), reading is serial (one part at a time), but if `ConcurrentStreamParts=true` is set in `PutObjectOptions`, the SDK pre-allocates `NumThreads` buffers and uploads them in parallel via `putObjectMultipartStreamParallel()`. Without `ConcurrentStreamParts`, streaming uploads are fully sequential -- one part is read and uploaded at a time.

> **Sources:** [minio-go/api-put-object.go](https://github.com/minio/minio-go/blob/master/api-put-object.go), [minio-go package docs](https://pkg.go.dev/github.com/minio/minio-go/v7), [DeepWiki -- Uploading Objects](https://deepwiki.com/minio/minio-go/3.1-uploading-objects)

---

## 2. Memory

For seekable inputs (`io.ReaderAt`), each goroutine reads directly from the source at different offsets, so the memory footprint is approximately `NumThreads * PartSize`. With defaults of 4 threads and a minimum part size of 5 MB (actual part size depends on object size and the 10,000 parts limit), this is roughly 20-640 MB depending on part size. For non-seekable streams with `ConcurrentStreamParts=true`, the SDK pre-allocates exactly `NumThreads` buffers of `PartSize` bytes each, reading into them serially and uploading in parallel, so memory consumption is precisely `NumThreads * PartSize`.

Without `ConcurrentStreamParts` on a streaming upload, only **one** part buffer is allocated at a time, since parts are read and uploaded sequentially. This makes the non-concurrent streaming path the most memory-efficient option, using only `PartSize` bytes of buffer memory. For objects smaller than 16 MiB, `PutObject()` performs a single atomic `PUT` operation without multipart splitting, and the entire object is buffered in memory. The SDK automatically selects the upload strategy based on object size: single `PUT` for objects under the minimum multipart size, and multipart for larger objects.

> **Sources:** [minio-go/api-put-object-streaming.go](https://github.com/minio/minio-go/blob/master/api-put-object-streaming.go), [minio-go/api-put-object-multipart.go](https://github.com/minio/minio-go/blob/master/api-put-object-multipart.go), [minio-go package docs](https://pkg.go.dev/github.com/minio/minio-go/v7)

---

## 3. Download

`GetObject()` returns a single HTTP GET response stream (`*minio.Object`) wrapping an `io.ReadCloser`. There is **no built-in parallel range download** mechanism. The SDK does not split large objects into concurrent byte-range requests the way the AWS SDK's s3 manager Downloader does. Each `GetObject()` call results in a single `GetObject` HTTP request to the server, and the caller reads the response body sequentially.

Range requests are supported through `GetObjectOptions.SetRange(start, end)`, which sets the HTTP `Range` header on the request. However, the user must implement their own concurrent range-GET logic if parallel downloads are desired -- the SDK provides no orchestration for splitting, dispatching, and reassembling range responses. The `FGetObject()` convenience function downloads an object to a local file but also uses a single sequential GET internally. This is a significant feature gap compared to the AWS SDK Go v2 `Downloader` and Python s3transfer, both of which provide automatic parallel download with configurable concurrency.

> **Sources:** [minio-go/api-get-object.go](https://github.com/minio/minio-go/blob/master/api-get-object.go), [minio-go package docs -- GetObject](https://pkg.go.dev/github.com/minio/minio-go/v7), [GitHub Issue #1347 -- Range ignored](https://github.com/minio/minio-go/issues/1347)

---

## 4. Retry

The MinIO Go SDK includes a retry mechanism implemented in `retry.go` with exponential backoff and jitter. The maximum number of retries is **10** (defined as a constant), with a default retry unit of **200 milliseconds** and a maximum retry cap of **1 second**. The backoff formula follows the "Full Jitter" pattern: `random_between(0, min(cap, base * 2^attempt))`, with the attempt value capped at 30 to prevent integer overflow. The `MaxJitter` value is 1.0 (full randomization), and a `NoJitter` option is available to disable randomization.

However, the retry strategy is **not user-configurable** through the public API. There are no exposed options to change `max_attempts`, backoff base, or retry cap on the `minio.Client`. The retry logic handles transient network errors (connection resets, timeouts) and certain HTTP status codes, but HTTP 400 errors are **not** retried. The SDK also includes a separate "continuous retry" mechanism (`retry-continous.go`) used internally for long-running operations that should retry indefinitely until success. Users who need custom retry behavior must implement their own retry wrapper around the SDK client methods.

> **Sources:** [minio-go/retry.go](https://github.com/minio/minio-go/blob/master/retry.go), [minio-go/retry-continous.go](https://github.com/minio/minio-go/blob/master/retry-continous.go)

---

## 5. Metrics

The MinIO Go SDK provides **zero** metrics, tracing, or observability instrumentation. There are no counters for requests made, bytes transferred, or errors encountered. There is no integration with OpenTelemetry, Prometheus, StatsD, or any other metrics framework. There are no hooks, callbacks, or interfaces for injecting custom metrics collection. The SDK does not emit tracing spans or support distributed tracing context propagation.

The only diagnostic facility is Go's standard `log` package output, which can be enabled for HTTP request/response debugging. For production observability, users must wrap the `minio.Client` methods or the underlying `http.RoundTripper` with custom instrumentation. This is consistent with the SDK's design philosophy as a lightweight, minimal S3 client library.

> **Sources:** [minio-go package docs](https://pkg.go.dev/github.com/minio/minio-go/v7), [minio-go GitHub repository](https://github.com/minio/minio-go)

---

## 6. Error Handling

The MinIO Go SDK defines an `ErrorResponse` struct in `api-error-response.go` with the following fields: `XMLName` (`xml.Name`), `Code` (`string`), `Message` (`string`), `BucketName` (`string`), `Key` (`string`), `RequestID` (`string`, XML tag `RequestId`), `HostID` (`string`, XML tag `HostId`), `Region` (`string`), and `StatusCode` (`int`, representing the underlying HTTP status code). The `ErrorResponse` implements the `error` interface, so it can be returned directly from SDK methods and type-asserted by callers.

Error classification requires **string matching** on the `Code` field (e.g., `"NoSuchKey"`, `"NoSuchBucket"`, `"AccessDenied"`, `"BucketAlreadyOwnedByYou"`). There is no enum-based error taxonomy or typed error constants for programmatic matching. Callers must use type assertion (`err.(minio.ErrorResponse)`) or `errors.As()` to extract the `ErrorResponse` from returned errors, then switch on the `Code` string. The `StatusCode` field provides the HTTP status code for coarse-grained classification (4xx vs 5xx), but fine-grained error handling requires string comparison against S3 error codes.

> **Sources:** [minio-go/api-error-response.go](https://github.com/minio/minio-go/blob/master/api-error-response.go), [minio-go package docs](https://pkg.go.dev/github.com/minio/minio-go/v7)

---

## 7. Checksum

The MinIO Go SDK auto-computes checksums for part uploads depending on the upload path and configuration. When `SendContentMd5` is enabled in `PutObjectOptions`, the SDK computes an MD5 digest for each part and includes it as the `Content-MD5` header. When `SendContentMd5` is not enabled and `DisableContentSha256` is also not set, the SDK computes SHA-256 for request signing purposes (standard AWS Signature v4). For non-seekable streaming uploads where both MD5 and SHA-256 are disabled, the SDK falls back to computing CRC32C for each part as a lightweight integrity check.

User-provided checksums can be specified via `PutObjectOptions`, including custom checksum values. The SDK supports the `ChecksumType` field on `PutObjectOptions` for selecting the checksum algorithm. However, as documented in [GitHub issue #1956](https://github.com/minio/minio-go/issues/1956), full end-to-end checksum validation for multipart uploads (including composite checksums on `CompleteMultipartUpload`) is not fully exposed in the public API. The `CompleteMultipartUpload` request includes the ETags (MD5-based) of each uploaded part for server-side validation by S3.

> **Sources:** [minio-go/api-put-object-multipart.go](https://github.com/minio/minio-go/blob/master/api-put-object-multipart.go), [minio-go/api-put-object-streaming.go](https://github.com/minio/minio-go/blob/master/api-put-object-streaming.go), [GitHub Issue #1956 -- multipart checksum validation](https://github.com/minio/minio-go/issues/1956), [GitHub Issue #914 -- uploads never do checksums](https://github.com/minio/minio-go/issues/914)
