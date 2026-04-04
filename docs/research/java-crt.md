# AWS SDK for Java v2 / CRT S3 Client -- Analysis

## 1. Concurrency Model

The AWS SDK for Java v2 offers two distinct S3 async client implementations with different concurrency models. The **CRT-based client** (built on the AWS Common Runtime written in C) auto-tunes concurrency from `targetThroughputInGbps` (default **10 Gbps**). The CRT internally calculates how many TCP connections to open based on this target throughput value and the estimated bandwidth of a single connection. An optional `maxConcurrency` override can cap the total number of connections, but by default the CRT manages this autonomously. Higher `targetThroughputInGbps` values result in more pre-opened connections and more concurrent part transfers.

The **Java-based async client** (using `NettyNioAsyncHttpClient`) has a `maxConcurrency` setting on the HTTP client that defines the total connection pool size, shared across all S3 operations. Parts from multiple concurrent multipart uploads compete for the same connection pool. Unlike the CRT client, there is no automatic throughput-based tuning -- the user must manually size the connection pool based on their workload. The CRT client is recommended for high-throughput transfer workloads because its C-native implementation avoids JVM overhead for connection management and data transfer.

> **Sources:** [AWS Docs -- CRT-based S3 client](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/crt-based-s3-client.html), [AWS Blog -- Introducing CRT-based S3 Client](https://aws.amazon.com/blogs/developer/introducing-crt-based-s3-client-and-the-s3-transfer-manager-in-the-aws-sdk-for-java-2-x/), [AWS Docs -- Configure CRT HTTP clients](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/http-configuration-crt.html)

---

## 2. Auto Transfer

The CRT S3 client automatically splits uploads into multipart uploads and downloads into parallel byte-range fetches without any user intervention. The user simply calls `putObject()` or `getObject()` through the `S3AsyncClient` built with the CRT builder, and the CRT handles part splitting, parallel scheduling, and completion internally. The `minimumPartSizeInBytes` (default **8 MB**, i.e., `8 * 1024 * 1024`) controls the minimum size of each part. The CRT also automatically calculates optimal part sizes for very large objects to stay within the S3 limit of 10,000 parts per upload.

When using the `S3TransferManager` (which wraps the CRT client), higher-level operations like `uploadFile()`, `downloadFile()`, and `uploadDirectory()` are available. The Transfer Manager delegates to the underlying CRT client for all multipart optimization. The user does not need to manually initiate `CreateMultipartUpload`, `UploadPart`, or `CompleteMultipartUpload` -- the entire lifecycle is abstracted away. This is a significant architectural difference from the Go and Python SDKs, where the manager layer is a separate library built on top of the base client.

> **Sources:** [AWS Docs -- CRT-based S3 client](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/crt-based-s3-client.html), [AWS Docs -- S3 Transfer Manager](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/transfer-manager.html), [S3TransferManager Javadoc](https://sdk.amazonaws.com/java/api/latest/software/amazon/awssdk/transfer/s3/S3TransferManager.html)

---

## 3. Metrics

The AWS SDK for Java v2 is the **only** AWS SDK with a fully functional metrics SPI in production. The `MetricPublisher` interface (`software.amazon.awssdk.metrics.MetricPublisher`) receives `MetricCollection` objects containing per-request metrics after each API call completes. The built-in `CloudWatchMetricPublisher` aggregates these metrics in memory and periodically publishes them to Amazon CloudWatch in a background thread. Custom publishers can implement the same interface to route metrics to Prometheus, Datadog, or any other backend.

The `CoreMetric` class (`software.amazon.awssdk.core.metrics.CoreMetric`) defines available metric keys including `API_CALL_DURATION` (total duration including retries), `API_CALL_SUCCESSFUL` (boolean), `RETRY_COUNT`, `OPERATION_NAME`, `SERVICE_ENDPOINT`, `ENDPOINT_RESOLVE_DURATION`, `MARSHALLING_DURATION`, `SIGNING_DURATION`, `SERVICE_CALL_DURATION`, `UNMARSHALLING_DURATION`, `CREDENTIALS_FETCH_DURATION`, and `READ_THROUGHPUT` (bytes per second, defined as response bytes read divided by time-to-last-byte minus time-to-first-byte). The `MetricPublisher` can be set at the client level (for all requests) or per-request via `OverrideConfiguration`. This level of metrics integration does not exist in the Go v2 SDK (no metrics SPI) or the Rust SDK (metrics support is a TODO).

> **Sources:** [AWS Docs -- Publish SDK Metrics](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/metrics.html), [CoreMetric Javadoc](https://sdk.amazonaws.com/java/api/latest/software/amazon/awssdk/core/metrics/CoreMetric.html), [AWS Docs -- Comprehensive Metrics Reference](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/metrics-list.html), [CloudWatchMetricPublisher Javadoc](https://sdk.amazonaws.com/java/api/latest/software/amazon/awssdk/metrics/publishers/cloudwatch/CloudWatchMetricPublisher.html)

---

## 4. Connection Warm-up

The CRT S3 client pre-establishes TCP connections and completes TLS handshakes based on the `targetThroughputInGbps` setting at client construction time. A higher throughput target results in more connections being opened during the warm-up phase, before any S3 requests are made. This eliminates the latency of TCP handshake and TLS negotiation on the first request, which is particularly significant for workloads that immediately need high throughput (e.g., batch processing jobs that start with a burst of large file transfers).

The warm-up behavior is automatic and cannot be disabled independently of the throughput target. The number of pre-opened connections is proportional to `targetThroughputInGbps` divided by the estimated throughput per connection (which the CRT calculates based on platform characteristics). For the default target of 10 Gbps, the CRT may open dozens of connections during warm-up. This is a unique feature of the CRT client -- the standard Java async client (`NettyNioAsyncHttpClient`) does not perform connection pre-warming and opens connections lazily on first request.

> **Sources:** [AWS Docs -- CRT-based S3 client](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/crt-based-s3-client.html), [AWS Blog -- Introducing CRT-based S3 Client](https://aws.amazon.com/blogs/developer/introducing-crt-based-s3-client-and-the-s3-transfer-manager-in-the-aws-sdk-for-java-2-x/)

---

## 5. Memory

CRT memory consumption is proportional to the number of connections multiplied by the part size, as each connection requires buffers for in-flight parts. The CRT manages its own native buffer pool **outside** the JVM heap, which means JVM heap settings (`-Xmx`) do not cap CRT memory usage. Known OOM issues have been reported when `targetThroughputInGbps` is set too high for the available system memory: [GitHub issue #4168](https://github.com/aws/aws-sdk-java-v2/issues/4168) documents a 50 GB upload crashing a Kubernetes pod, [issue #4034](https://github.com/aws/aws-sdk-java-v2/issues/4034) reports the CRT not respecting pod memory limits, and [issue #6323](https://github.com/aws/aws-sdk-java-v2/issues/6323) reports `OutOfMemoryError` during concurrent downloads.

The `maxNativeMemoryLimitInBytes` option was introduced to cap CRT native memory usage, but as noted in [issue #894 on aws-crt-java](https://github.com/awslabs/aws-crt-java/issues/894), users have reported that the default CRT configuration can exceed 250 MB of native memory even for modest workloads. The fundamental trade-off is that higher `targetThroughputInGbps` increases both throughput and memory consumption, and users in memory-constrained environments (containers, Lambda) must carefully tune this value downward. The JVM's `-XX:MaxDirectMemorySize` flag does not constrain CRT memory since the CRT uses its own allocator via JNI.

> **Sources:** [GitHub Issue #4168 -- large file upload memory](https://github.com/aws/aws-sdk-java-v2/issues/4168), [GitHub Issue #4034 -- CRT pod memory limits](https://github.com/aws/aws-sdk-java-v2/issues/4034), [GitHub Issue #6323 -- OOM with S3TransferManager](https://github.com/aws/aws-sdk-java-v2/issues/6323), [GitHub Issue #894 -- low memory CRT](https://github.com/awslabs/aws-crt-java/issues/894)

---

## 6. Error Handling

The AWS SDK for Java v2 uses a structured exception hierarchy rooted at `SdkException` (which extends `RuntimeException`). The hierarchy branches into `SdkClientException` (for client-side failures such as network errors, serialization errors, or configuration issues) and `SdkServiceException` (for errors returned by the AWS service in the HTTP response). `AwsServiceException` extends `SdkServiceException` and adds AWS-specific fields like `awsErrorDetails()` which provides `errorCode()`, `errorMessage()`, `sdkHttpResponse()` (with status code), and `serviceName()`.

`S3Exception` extends `AwsServiceException` and is the concrete exception type thrown for S3 service errors. It provides `requestId()` and `extendedRequestId()` (the S3 host ID) for troubleshooting with AWS Support. Programmatic error handling involves catching `S3Exception` and switching on `awsErrorDetails().errorCode()` (a `String` value like `"NoSuchKey"`, `"AccessDenied"`, `"NoSuchBucket"`). There is no enum-based error code taxonomy -- all error codes are strings. For client-side errors, `SdkClientException` wraps the root cause (e.g., `IOException` for network failures) and can be unwrapped with `getCause()`.

> **Sources:** [AWS Docs -- Handling Exceptions](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/handling-exceptions.html), [S3Exception Javadoc](https://sdk.amazonaws.com/java/api/latest/software/amazon/awssdk/services/s3/model/S3Exception.html), [AWS Docs -- Exception changes in v2 migration](https://docs.aws.amazon.com/sdk-for-java/latest/developer-guide/migration-exception-changes.html)
