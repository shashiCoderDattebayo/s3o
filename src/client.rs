use crate::config::{S3Config, S3ConfigBuilder};
use crate::error::{classify, ErrorKind, S3Error, S3Result};
use crate::metrics::{self, OpMetrics};
use crate::pool::Pool;
use crate::types::{ObjectEntry, ObjectMetadata};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart as AwsPart};
use aws_sdk_s3::Client;
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;
/// Opinionated S3 client with read/write split connection pools, automatic
/// multipart upload, parallel range download, per-operation metrics, and tracing.
#[derive(Clone)]
pub struct S3Client {
    s3: Client,
    bucket: String,
    config: Arc<S3Config>,
    pool: Pool,
}
impl S3Client {
    pub fn builder() -> S3ConfigBuilder {
        S3ConfigBuilder::new()
    }
    pub(crate) fn from_config(config: S3Config) -> S3Result<Self> {
        let creds = Credentials::new(
            &config.access_key_id,
            &config.secret_access_key,
            config.session_token.clone(),
            None,
            "s3-client",
        );
        let mut sdk = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .credentials_provider(creds)
            .force_path_style(config.path_style)
            .retry_config(
                aws_sdk_s3::config::retry::RetryConfig::standard()
                    .with_max_attempts(config.max_retries as u32 + 1),
            );
        if let Some(ep) = &config.endpoint {
            sdk = sdk.endpoint_url(ep);
        }
        let pool = Pool::new(
            config.read_connections,
            config.write_connections,
            config.timeout,
        );
        Ok(Self {
            s3: Client::from_conf(sdk.build()),
            bucket: config.bucket.clone(),
            config: Arc::new(config),
            pool,
        })
    }

    pub async fn put_object(&self, key: &str, body: Bytes) -> S3Result<()> {
        let span = tracing::info_span!("s3.put", bucket = %self.bucket, key, len = body.len());
        async {
            let mut m = metrics::start_operation("put");
            m.sent(body.len() as u64);
            let r = if body.len() > self.config.multipart_threshold {
                self.multipart_put(key, body, &mut m).await
            } else {
                self.simple_put(key, body, &mut m).await
            };
            finish(&mut m, &r);
            r
        }
        .instrument(span)
        .await
    }
    pub async fn get_object(&self, key: &str) -> S3Result<Bytes> {
        let span = tracing::info_span!("s3.get", bucket = %self.bucket, key);
        async {
            let mut m = metrics::start_operation("get");
            let meta = match self.stat(key, &mut m).await {
                Ok(v) => v,
                Err(e) => {
                    m.err(&e);
                    return Err(e);
                }
            };
            let r = if meta.content_length as usize > self.config.multipart_threshold {
                self.range_get(key, meta.content_length, &mut m).await
            } else {
                self.simple_get(key, &mut m).await
            };
            if let Ok(data) = &r {
                m.recv(data.len() as u64);
            }
            finish(&mut m, &r);
            r
        }
        .instrument(span)
        .await
    }
    pub async fn delete_object(&self, key: &str) -> S3Result<()> {
        let span = tracing::info_span!("s3.delete", bucket = %self.bucket, key);
        async {
            let mut m = metrics::start_operation("delete");
            let (_p, qw) = self.pool.write().await?;
            m.add_queue_wait(qw);
            let r = self
                .s3
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map(|_| ())
                .map_err(|e| classify(&e));
            finish(&mut m, &r);
            r
        }
        .instrument(span)
        .await
    }
    pub async fn head_object(&self, key: &str) -> S3Result<ObjectMetadata> {
        let span = tracing::info_span!("s3.head", bucket = %self.bucket, key);
        async {
            let mut m = metrics::start_operation("head");
            let r = self.stat(key, &mut m).await;
            finish(&mut m, &r);
            r
        }
        .instrument(span)
        .await
    }
    /// List objects under `prefix`. When `recursive` is false (default-like behavior),
    /// only immediate children are returned (uses delimiter="/"). When true, all
    /// objects under the prefix are returned regardless of depth.
    pub async fn list_objects(&self, prefix: &str, recursive: bool) -> S3Result<Vec<ObjectEntry>> {
        let span = tracing::info_span!("s3.list", bucket = %self.bucket, prefix, recursive);
        async {
            let mut m = metrics::start_operation("list");
            let (_p, qw) = self.pool.read().await?;
            m.add_queue_wait(qw);
            let r = self.paginated_list(prefix, recursive).await;
            finish(&mut m, &r);
            r
        }
        .instrument(span)
        .await
    }

    async fn stat(&self, key: &str, m: &mut OpMetrics) -> S3Result<ObjectMetadata> {
        let (_p, qw) = self.pool.read().await?;
        m.add_queue_wait(qw);
        let r = self
            .s3
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| classify(&e))?;
        Ok(ObjectMetadata {
            content_length: r.content_length.unwrap_or(0) as u64,
            content_type: r.content_type,
            etag: r.e_tag,
            last_modified: r
                .last_modified
                .and_then(|t| chrono::DateTime::from_timestamp(t.secs(), t.subsec_nanos())),
        })
    }
    async fn simple_put(&self, key: &str, body: Bytes, m: &mut OpMetrics) -> S3Result<()> {
        let (_p, qw) = self.pool.write().await?;
        m.add_queue_wait(qw);
        let t = Instant::now();
        let r = self
            .s3
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body))
            .send()
            .await
            .map(|_| ())
            .map_err(|e| classify(&e));
        let elapsed = t.elapsed().as_secs_f64();
        metrics::http("put", elapsed, r.is_ok());
        metrics::ttfb("put", elapsed); // PUT response = headers only, so elapsed ≈ TTFB
        r
    }
    async fn simple_get(&self, key: &str, m: &mut OpMetrics) -> S3Result<Bytes> {
        let (_p, qw) = self.pool.read().await?;
        m.add_queue_wait(qw);
        let t = Instant::now();
        let r = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| classify(&e));
        metrics::ttfb("get", t.elapsed().as_secs_f64()); // send() returns at first response byte
        let r = match r {
            Ok(output) => output
                .body
                .collect()
                .await
                .map(|b| b.into_bytes())
                .map_err(|e| S3Error::backend(ErrorKind::Other, format!("body: {e}"))),
            Err(e) => Err(e),
        };
        metrics::http("get", t.elapsed().as_secs_f64(), r.is_ok()); // total including body read
        r
    }
    async fn paginated_list(&self, prefix: &str, recursive: bool) -> S3Result<Vec<ObjectEntry>> {
        let mut entries = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .s3
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            if !recursive {
                req = req.delimiter("/");
            }
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let resp = req.send().await.map_err(|e| classify(&e))?;
            if let Some(contents) = resp.contents {
                for o in contents {
                    entries.push(ObjectEntry {
                        key: o.key.unwrap_or_default(),
                        size: o.size.unwrap_or(0) as u64,
                        etag: o.e_tag,
                        last_modified: o.last_modified.and_then(|t| {
                            chrono::DateTime::from_timestamp(t.secs(), t.subsec_nanos())
                        }),
                    });
                }
            }
            if let Some(pfx) = resp.common_prefixes {
                for p in pfx {
                    if let Some(k) = p.prefix {
                        entries.push(ObjectEntry {
                            key: k,
                            size: 0,
                            etag: None,
                            last_modified: None,
                        });
                    }
                }
            }
            match (resp.is_truncated, resp.next_continuation_token) {
                (Some(true), Some(t)) => token = Some(t),
                _ => break,
            }
        }
        Ok(entries)
    }

    async fn multipart_put(&self, key: &str, body: Bytes, m: &mut OpMetrics) -> S3Result<()> {
        let wc = self.config.write_concurrency;
        let cs = self.config.chunk_size;
        let total = body.len();
        let nparts = total.div_ceil(cs);
        // Reserve write_concurrency permits for the entire upload.
        let (_bulk, qw) = self.pool.write_bulk(wc).await?;
        m.add_queue_wait(qw);
        // Initiate
        let uid = self
            .s3
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| classify(&e))?
            .upload_id
            .ok_or_else(|| S3Error::backend(ErrorKind::Other, "no upload_id"))?;
        // Upload parts — buffer_unordered(wc) matches reserved permits.
        let chunks: Vec<_> = (0..nparts)
            .map(|i| {
                let start = i * cs;
                (
                    (i + 1) as i32,
                    body.slice(start..std::cmp::min(start + cs, total)),
                )
            })
            .collect();
        let this = self.clone();
        let uid2 = uid.clone();
        let results: Vec<S3Result<AwsPart>> =
            futures::stream::iter(chunks.into_iter().map(move |(pn, chunk)| {
                let this = this.clone();
                let uid = uid2.clone();
                async move {
                    let span = tracing::info_span!("s3.part", pn, size = chunk.len());
                    async {
                        let t = Instant::now();
                        let r = this
                            .s3
                            .upload_part()
                            .bucket(&this.bucket)
                            .key(key)
                            .upload_id(&uid)
                            .part_number(pn)
                            .body(ByteStream::from(chunk))
                            .send()
                            .await
                            .map_err(|e| classify(&e));
                        let elapsed = t.elapsed().as_secs_f64();
                        metrics::http("put_part", elapsed, r.is_ok());
                        metrics::ttfb("put_part", elapsed);
                        let etag = r?
                            .e_tag
                            .ok_or_else(|| S3Error::backend(ErrorKind::Other, "no etag"))?;
                        Ok(AwsPart::builder().part_number(pn).e_tag(etag).build())
                    }
                    .instrument(span)
                    .await
                }
            }))
            .buffer_unordered(wc)
            .collect()
            .await;
        // Collect parts or abort
        let mut parts = Vec::with_capacity(nparts);
        let mut first_err: Option<S3Error> = None;
        for r in results {
            match r {
                Ok(p) => parts.push(p),
                Err(e) if first_err.is_none() => first_err = Some(e),
                Err(_) => {} // subsequent errors dropped
            }
        }
        if let Some(e) = first_err {
            tracing::warn!(upload_id = %uid, "aborting multipart upload");
            if let Err(ae) = self
                .s3
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(&uid)
                .send()
                .await
            {
                tracing::error!(error = %ae, "abort failed — orphaned parts");
            }
            return Err(e);
        }
        // Complete
        parts.sort_by_key(|p| p.part_number);
        self.s3
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(&uid)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(|e| classify(&e))
    }

    async fn range_get(&self, key: &str, total: u64, m: &mut OpMetrics) -> S3Result<Bytes> {
        let rc = self.config.read_concurrency;
        let rs = self.config.chunk_size as u64; // reuse chunk_size for range size
                                                // Reserve read_concurrency permits for the entire download.
        let (_bulk, qw) = self.pool.read_bulk(rc).await?;
        m.add_queue_wait(qw);
        let nparts = total.div_ceil(rs);
        let ranges: Vec<_> = (0..nparts)
            .map(|i| {
                let start = i * rs;
                (start, std::cmp::min(start + rs, total), i as usize)
            })
            .collect();
        let this = self.clone();
        let results: Vec<S3Result<(usize, Bytes)>> =
            futures::stream::iter(ranges.into_iter().map(move |(start, end, idx)| {
                let this = this.clone();
                async move {
                    let span = tracing::info_span!("s3.range", idx, start, end);
                    async {
                        let t = Instant::now();
                        let range = format!("bytes={}-{}", start, end.saturating_sub(1));
                        let r = this
                            .s3
                            .get_object()
                            .bucket(&this.bucket)
                            .key(key)
                            .range(range)
                            .send()
                            .await
                            .map_err(|e| classify(&e));
                        metrics::ttfb("get_range", t.elapsed().as_secs_f64());
                        let data =
                            r?.body
                                .collect()
                                .await
                                .map(|b| b.into_bytes())
                                .map_err(|e| {
                                    S3Error::backend(ErrorKind::Other, format!("range body: {e}"))
                                })?;
                        metrics::http("get_range", t.elapsed().as_secs_f64(), true);
                        Ok((idx, data))
                    }
                    .instrument(span)
                    .await
                }
            }))
            .buffer_unordered(rc)
            .collect()
            .await;
        // Reassemble in order
        let mut parts: Vec<(usize, Bytes)> = Vec::with_capacity(results.len());
        for r in results {
            parts.push(r?);
        }
        parts.sort_by_key(|(i, _)| *i);
        let mut buf = bytes::BytesMut::with_capacity(total as usize);
        for (_, chunk) in parts {
            buf.extend_from_slice(&chunk);
        }
        Ok(buf.freeze())
    }
}
fn finish<T>(m: &mut OpMetrics, r: &S3Result<T>) {
    match r {
        Ok(_) => m.ok(),
        Err(e) => m.err(e),
    }
}
impl std::fmt::Debug for S3Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Client")
            .field("bucket", &self.bucket)
            .field("region", &self.config.region)
            .field("endpoint", &self.config.endpoint)
            .finish()
    }
}
