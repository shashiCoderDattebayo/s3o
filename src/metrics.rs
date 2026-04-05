//! Per-operation metrics tracking.

use std::time::{Duration, Instant};

use crate::error::S3Error;

pub(crate) fn start_operation(op: &'static str) -> OpMetrics {
    metrics::counter!("s3.op.total", "op" => op).increment(1);
    OpMetrics {
        op,
        start: Instant::now(),
        queue_wait: Duration::ZERO,
        bytes_sent: 0,
        bytes_recv: 0,
        done: false,
    }
}

pub(crate) struct OpMetrics {
    op: &'static str,
    start: Instant,
    queue_wait: Duration,
    bytes_sent: u64,
    bytes_recv: u64,
    done: bool,
}

impl OpMetrics {
    pub fn add_queue_wait(&mut self, d: Duration) {
        self.queue_wait += d;
    }

    pub fn sent(&mut self, n: u64) {
        self.bytes_sent += n;
    }

    pub fn recv(&mut self, n: u64) {
        self.bytes_recv += n;
    }

    pub fn ok(&mut self) {
        self.done = true;
        self.record("ok");
        metrics::counter!("s3.op.ok", "op" => self.op).increment(1);
        if self.bytes_sent > 0 {
            metrics::counter!("s3.op.bytes_sent", "op" => self.op).increment(self.bytes_sent);
        }
        if self.bytes_recv > 0 {
            metrics::counter!("s3.op.bytes_recv", "op" => self.op).increment(self.bytes_recv);
        }
        // bytes rate = total bytes / duration (excluding queue wait)
        let duration = self.start.elapsed().saturating_sub(self.queue_wait);
        let total_bytes = self.bytes_sent + self.bytes_recv;
        if total_bytes > 0 && !duration.is_zero() {
            let rate = total_bytes as f64 / duration.as_secs_f64();
            metrics::gauge!("s3.op.bytes_rate", "op" => self.op).set(rate);
        }
    }

    pub fn err(&mut self, e: &S3Error) {
        self.done = true;
        self.record("err");
        metrics::counter!("s3.op.err", "op" => self.op, "kind" => e.error_kind().to_string())
            .increment(1);
    }

    fn record(&self, status: &'static str) {
        let latency = self.start.elapsed();
        let duration = latency.saturating_sub(self.queue_wait);

        metrics::histogram!("s3.op.latency", "op" => self.op, "status" => status)
            .record(latency.as_secs_f64());
        metrics::histogram!("s3.op.duration", "op" => self.op, "status" => status)
            .record(duration.as_secs_f64());

        if !self.queue_wait.is_zero() {
            metrics::histogram!("s3.op.queue_wait", "op" => self.op)
                .record(self.queue_wait.as_secs_f64());
        }
    }
}

impl Drop for OpMetrics {
    fn drop(&mut self) {
        if !self.done {
            self.record("cancelled");
            metrics::counter!("s3.op.cancelled", "op" => self.op).increment(1);
        }
    }
}

pub(crate) fn http(op: &'static str, secs: f64, ok: bool) {
    let s = if ok { "ok" } else { "err" };
    metrics::counter!("s3.http.total", "op" => op).increment(1);
    metrics::histogram!("s3.http.duration", "op" => op, "status" => s).record(secs);
}

/// Time to first byte — from request send to response headers received.
pub(crate) fn ttfb(op: &'static str, secs: f64) {
    metrics::histogram!("s3.http.ttfb", "op" => op).record(secs);
}
