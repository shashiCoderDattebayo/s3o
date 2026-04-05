use crate::error::{S3Error, S3Result};
use std::time::Duration;
/// All configuration for the S3 client. Immutable after construction.
#[derive(Debug, Clone)]
pub struct S3Config {
    // Required
    pub(crate) bucket: String,
    pub(crate) region: String,
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) session_token: Option<String>,
    // Endpoint
    pub(crate) endpoint: Option<String>,
    pub(crate) path_style: bool,
    // Transfer
    pub(crate) chunk_size: usize,
    pub(crate) multipart_threshold: usize,
    // Concurrency — read/write split
    pub(crate) read_connections: usize,
    pub(crate) write_connections: usize,
    pub(crate) read_concurrency: usize,
    pub(crate) write_concurrency: usize,
    // Integrity
    pub(crate) upload_checksum: bool,
    // Timeouts & retry
    pub(crate) timeout: Duration,
    pub(crate) max_retries: usize,
}
const CHUNK: usize = 8 * 1024 * 1024;
const THRESHOLD: usize = 8 * 1024 * 1024;
const READ_CONN: usize = 15;
const WRITE_CONN: usize = 10;
const READ_CONC: usize = 4;
const WRITE_CONC: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(30);
const RETRIES: usize = 3;
const MIN_CHUNK: usize = 5 * 1024 * 1024;
#[derive(Debug, Default)]
pub struct S3ConfigBuilder {
    bucket: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    endpoint: Option<String>,
    path_style: Option<bool>,
    chunk_size: Option<usize>,
    multipart_threshold: Option<usize>,
    read_connections: Option<usize>,
    write_connections: Option<usize>,
    read_concurrency: Option<usize>,
    write_concurrency: Option<usize>,
    upload_checksum: Option<bool>,
    timeout: Option<Duration>,
    max_retries: Option<usize>,
}
macro_rules! builder_fn {
    ($name:ident, String) => {
        pub fn $name(mut self, v: impl Into<String>) -> Self {
            self.$name = Some(v.into());
            self
        }
    };
    ($name:ident, $t:ty) => {
        pub fn $name(mut self, v: $t) -> Self {
            self.$name = Some(v);
            self
        }
    };
}

impl S3ConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-configure for maximum throughput workloads (ML checkpoints, large datasets).
    ///
    /// Sets: 64 MiB chunks, 16x concurrency, 20 connections per pool, 5 min timeout.
    /// Caller still must set bucket, region, and credentials.
    pub fn max_throughput(mut self) -> Self {
        self.chunk_size = Some(64 * 1024 * 1024);
        self.multipart_threshold = Some(64 * 1024 * 1024);
        self.read_concurrency = Some(16);
        self.write_concurrency = Some(16);
        self.read_connections = Some(20);
        self.write_connections = Some(20);
        self.timeout = Some(Duration::from_secs(300));
        self
    }
    builder_fn!(bucket, String);
    builder_fn!(region, String);
    builder_fn!(access_key_id, String);
    builder_fn!(secret_access_key, String);
    builder_fn!(session_token, String);
    builder_fn!(endpoint, String);
    builder_fn!(path_style, bool);
    builder_fn!(chunk_size, usize);
    builder_fn!(multipart_threshold, usize);
    builder_fn!(read_connections, usize);
    builder_fn!(write_connections, usize);
    builder_fn!(read_concurrency, usize);
    builder_fn!(write_concurrency, usize);
    builder_fn!(upload_checksum, bool);
    builder_fn!(timeout, Duration);
    builder_fn!(max_retries, usize);
    pub fn build(self) -> S3Result<crate::S3Client> {
        let c = self.validate()?;
        crate::S3Client::from_config(c)
    }

    fn validate(self) -> S3Result<S3Config> {
        let required = |v: Option<String>, name: &str| -> S3Result<String> {
            v.filter(|s| !s.is_empty())
                .ok_or_else(|| S3Error::config(format!("{name} is required")))
        };
        let bucket = required(self.bucket, "bucket")?;
        let region = required(self.region, "region")?;
        let access_key_id = required(self.access_key_id, "access_key_id")?;
        let secret_access_key = required(self.secret_access_key, "secret_access_key")?;
        let chunk_size = self.chunk_size.unwrap_or(CHUNK);
        let read_connections = self.read_connections.unwrap_or(READ_CONN);
        let write_connections = self.write_connections.unwrap_or(WRITE_CONN);
        let read_concurrency = self.read_concurrency.unwrap_or(READ_CONC);
        let write_concurrency = self.write_concurrency.unwrap_or(WRITE_CONC);
        if chunk_size < MIN_CHUNK {
            return Err(S3Error::config(format!(
                "chunk_size must be >= 5MiB, got {chunk_size}"
            )));
        }
        if read_concurrency == 0 || write_concurrency == 0 {
            return Err(S3Error::config("concurrency must be > 0"));
        }
        if read_connections < read_concurrency + 1 {
            return Err(S3Error::config(format!(
                "read_connections ({read_connections}) must be >= read_concurrency + 1 ({})",
                read_concurrency + 1
            )));
        }
        if write_connections < write_concurrency + 1 {
            return Err(S3Error::config(format!(
                "write_connections ({write_connections}) must be >= write_concurrency + 1 ({})",
                write_concurrency + 1
            )));
        }
        Ok(S3Config {
            bucket,
            region,
            access_key_id,
            secret_access_key,
            session_token: self.session_token,
            endpoint: self.endpoint,
            path_style: self.path_style.unwrap_or(false),
            chunk_size,
            multipart_threshold: self.multipart_threshold.unwrap_or(THRESHOLD),
            read_connections,
            write_connections,
            read_concurrency,
            write_concurrency,
            upload_checksum: self.upload_checksum.unwrap_or(false),
            timeout: self.timeout.unwrap_or(TIMEOUT),
            max_retries: self.max_retries.unwrap_or(RETRIES),
        })
    }
}
