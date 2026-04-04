# Source Verification Log

Fact-checking claims about Java SDK v2 and JavaScript SDK v3 against actual source code.
Verified on: 2026-04-04

---

## 1. Java SDK v2 (aws-sdk-java-v2)

### 1.1 MetricPublisher Interface

**File:** `core/metrics-spi/src/main/java/software/amazon/awssdk/metrics/MetricPublisher.java`

**VERIFIED:** `MetricPublisher` is a public interface (line 40):
```java
public interface MetricPublisher extends SdkAutoCloseable {
```

It has two methods:
- `void publish(MetricCollection metricCollection);` (line 57)
- `void close();` (line 66, inherited from SdkAutoCloseable)

The interface is annotated `@ThreadSafe` and `@SdkPublicApi`. Javadoc confirms it receives
a stream of `MetricCollection` objects via `publish()`, and implementations must be threadsafe.

### 1.2 CRT Default Target Throughput

**File:** `services/s3/src/main/java/software/amazon/awssdk/services/s3/internal/crt/S3NativeClientConfiguration.java`

**VERIFIED:** Default throughput is **10 Gbps** (line 49):
```java
private static final long DEFAULT_TARGET_THROUGHPUT_IN_GBPS = 10;
```

Applied at lines 96-97:
```java
this.targetThroughputInGbps = builder.targetThroughputInGbps == null ?
                              DEFAULT_TARGET_THROUGHPUT_IN_GBPS : builder.targetThroughputInGbps;
```

Also confirmed in S3CrtAsyncClientBuilder.java Javadoc (line 152-153):
"By default, it is 10 gigabits per second."

### 1.3 CRT Default Part Size

**File:** `services/s3/src/main/java/software/amazon/awssdk/services/s3/internal/crt/S3NativeClientConfiguration.java`

**VERIFIED:** Default part size is **8 MB** (line 47):
```java
static final long DEFAULT_PART_SIZE_IN_BYTES = 8L * 1024 * 1024;
```

Also confirmed in S3CrtAsyncClientBuilder.java Javadoc (line 117):
"By default, it is 8MB."

### 1.4 CRT maxConcurrency Default

**File:** `services/s3/src/main/java/software/amazon/awssdk/services/s3/internal/crt/S3NativeClientConfiguration.java`

**VERIFIED:** maxConcurrency defaults to **0** (line 100), which tells CRT to auto-calculate based on targetThroughputInGbps:
```java
this.maxConcurrency = builder.maxConcurrency == null ? 0 : builder.maxConcurrency;
```

S3CrtAsyncClientBuilder.java confirms (line 175-176):
"If not provided, the TransferManager will calculate the optional number of connections
based on targetThroughputInGbps."

### 1.5 S3CrtAsyncClientBuilder Config Methods

**File:** `services/s3/src/main/java/software/amazon/awssdk/services/s3/S3CrtAsyncClientBuilder.java`

**VERIFIED:** The builder interface exposes the following key methods:
- `targetThroughputInGbps(Double)` (line 168)
- `maxConcurrency(Integer)` (line 183)
- `minimumPartSizeInBytes(Long)` (line 122)
- `maxNativeMemoryLimitInBytes(Long)` (line 141)
- `initialReadBufferSizeInBytes(Long)` (line 229)
- `thresholdInBytes(Long)` (line 330)
- `httpConfiguration(S3CrtHttpConfiguration)` (line 238)
- `retryConfiguration(S3CrtRetryConfiguration)` (line 246)
- `crossRegionAccessEnabled(Boolean)` (line 310)
- `futureCompletionExecutor(Executor)` (line 355)

### 1.6 MetricCollection and MetricPublisher (Metrics SPI)

**File:** `core/metrics-spi/src/main/java/software/amazon/awssdk/metrics/`

**VERIFIED:**
- `MetricPublisher` is an interface (line 40 of MetricPublisher.java)
- `DefaultMetricCollection` implements `MetricCollection` (internal class)
- `EmptyMetricCollection` implements `MetricCollection` (internal class)

### 1.7 CoreMetric -- Recorded Metrics

**File:** `core/sdk-core/src/main/java/software/amazon/awssdk/core/metrics/CoreMetric.java`

**VERIFIED:** The following static `SdkMetric` fields are defined:

| Metric Name             | Type       | Level  | Line |
|-------------------------|------------|--------|------|
| SERVICE_ID              | String     | ERROR  | 33   |
| OPERATION_NAME          | String     | ERROR  | 40   |
| API_CALL_SUCCESSFUL     | Boolean    | ERROR  | 46   |
| RETRY_COUNT             | Integer    | ERROR  | 53   |
| SERVICE_ENDPOINT        | URI        | ERROR  | 59   |
| API_CALL_DURATION       | Duration   | INFO   | 68   |
| CREDENTIALS_FETCH_DURATION | Duration | INFO  | 74   |
| TOKEN_FETCH_DURATION    | Duration   | INFO   | 80   |
| BACKOFF_DELAY_DURATION  | Duration   | INFO   | 87   |
| MARSHALLING_DURATION    | Duration   | INFO   | 93   |
| SIGNING_DURATION        | Duration   | INFO   | 99   |
| SERVICE_CALL_DURATION   | Duration   | INFO   | 107  |
| UNMARSHALLING_DURATION  | Duration   | INFO   | 115  |
| AWS_REQUEST_ID          | String     | INFO   | 121  |
| AWS_EXTENDED_REQUEST_ID | String     | INFO   | 127  |
| TIME_TO_FIRST_BYTE      | Duration   | TRACE  | 135  |
| TIME_TO_LAST_BYTE       | Duration   | TRACE  | 145  |
| READ_THROUGHPUT          | Double     | TRACE  | 156  |
| WRITE_THROUGHPUT         | Double     | TRACE  | 177  |
| ENDPOINT_RESOLVE_DURATION | Duration | INFO   | 183  |
| ERROR_TYPE              | String     | INFO   | 200  |

Note: The metric names use string values like `"RetryCount"`, `"ApiCallDuration"`,
`"MarshallingDuration"` -- NOT constants named `RETRY_COUNT` etc. The Java field names
differ from the published metric string names.

---

## 2. JavaScript SDK v3 (aws-sdk-js-v3)

### 2.1 lib-storage File Structure

**Directory:** `lib/lib-storage/src/`

**VERIFIED:** Key source files found:
- `Upload.ts` (main upload class)
- `Upload.spec.ts` (tests)
- `types.ts` (type definitions)
- `byteLength.spec.ts`
- `runtimeConfig.ts`, `runtimeConfig.shared.ts`, `runtimeConfig.browser.ts`
- `chunks/getChunkUint8Array.ts`
- `index.spec.ts`
- `lib-storage.e2e.spec.ts`

### 2.2 Upload Class Definition

**File:** `lib/lib-storage/src/Upload.ts`

**VERIFIED:** (line 45):
```typescript
export class Upload extends EventEmitter {
```

### 2.3 queueSize Default

**File:** `lib/lib-storage/src/Upload.ts`

**VERIFIED:** Default queueSize is **4** (line 57):
```typescript
private readonly queueSize: number = 4;
```

Also confirmed in types.ts (line 30):
"default: 4" in the JSDoc for queueSize.

The constructor applies options (line 94):
```typescript
this.queueSize = options.queueSize || this.queueSize;
```

### 2.4 partSize Default

**File:** `lib/lib-storage/src/Upload.ts`

**VERIFIED:** MIN_PART_SIZE is **5 MB** (line 50):
```typescript
private static MIN_PART_SIZE = 1024 * 1024 * 5;
```

The actual partSize defaults dynamically (lines 111-112):
```typescript
this.partSize =
  options.partSize || Math.max(Upload.MIN_PART_SIZE, Math.floor((this.totalBytes || 0) / this.MAX_PARTS));
```

So partSize = max(5MB, totalBytes / 10000) when not explicitly set. For most files under ~50GB,
the effective default is **5 MB**. This is confirmed in types.ts (line 37): "Default: 5 mb".

**CORRECTION vs common claim:** The partSize is NOT a simple constant 5MB default -- it dynamically
scales up for large files to stay under the 10,000 part limit.

### 2.5 MAX_PARTS Limit

**File:** `lib/lib-storage/src/Upload.ts`

**VERIFIED:** (line 54):
```typescript
private MAX_PARTS = 10_000;
```

### 2.6 maxSockets / Connection Limit

**Finding:** The `node-http-handler` package has been moved to `reserved/packages/node-http-handler/`
and simply re-exports from `@smithy/node-http-handler` (an external Smithy dependency):
```typescript
export * from "@smithy/node-http-handler";
```

There is **no maxSockets default defined in the aws-sdk-js-v3 repo itself**. The connection
limit is controlled by the Smithy HTTP handler (external dependency) or by Node.js's default
`http.Agent` behavior (Node.js default maxSockets = Infinity since Node.js v19; prior versions
defaulted to 5 per host).

Integration tests in `packages-internal/core` reference `maxSockets: 25` as test values,
but these are not defaults -- they are test configuration values.

### 2.7 Managed Download

**Finding:** `find aws-sdk-js-v3 -path "*managed-download*" -name "*.ts"` returned **no results**.

**CONFIRMED:** There is NO managed download utility in aws-sdk-js-v3. The lib-storage package
only provides the `Upload` class for managed multipart uploads. There is no equivalent
`Download` class. Users must implement their own download logic using `GetObjectCommand`
with range requests, or use the `@aws-sdk/lib-storage` Upload class only for uploads.

---

## Summary of Key Facts

| Claim | Status | Actual Value | Source |
|-------|--------|--------------|--------|
| Java CRT default throughput = 10 Gbps | CONFIRMED | `DEFAULT_TARGET_THROUGHPUT_IN_GBPS = 10` | S3NativeClientConfiguration.java:49 |
| Java CRT default part size = 8 MB | CONFIRMED | `DEFAULT_PART_SIZE_IN_BYTES = 8L * 1024 * 1024` | S3NativeClientConfiguration.java:47 |
| Java CRT maxConcurrency auto-calculated | CONFIRMED | Defaults to 0 (CRT calculates from throughput) | S3NativeClientConfiguration.java:100 |
| MetricPublisher is a public interface | CONFIRMED | `public interface MetricPublisher extends SdkAutoCloseable` | MetricPublisher.java:40 |
| CoreMetric has RETRY_COUNT | CONFIRMED | Field name `RETRY_COUNT`, metric string `"RetryCount"` | CoreMetric.java:53 |
| CoreMetric has API_CALL_DURATION | CONFIRMED | Field name `API_CALL_DURATION`, metric string `"ApiCallDuration"` | CoreMetric.java:68 |
| CoreMetric has MARSHALLING_DURATION | CONFIRMED | Field name `MARSHALLING_DURATION`, metric string `"MarshallingDuration"` | CoreMetric.java:93 |
| JS Upload queueSize default = 4 | CONFIRMED | `private readonly queueSize: number = 4` | Upload.ts:57 |
| JS Upload partSize default = 5 MB | PARTIALLY CORRECT | Min is 5MB, but dynamically scales for large files | Upload.ts:50,111-112 |
| JS maxSockets in node-http-handler | NOT IN THIS REPO | Delegated to `@smithy/node-http-handler` | reserved/packages/node-http-handler/src/index.ts |
| JS managed download exists | NOT FOUND | No managed-download files exist in the repo | find returned empty |
