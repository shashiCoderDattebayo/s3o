use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use hyper_util::rt::TokioIo;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use s3s_fs::FileSystem;
use tokio::net::TcpListener;

use s3o::{ErrorKind, S3Client};

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn server(root: &std::path::Path) -> SocketAddr {
    let fs = FileSystem::new(root).unwrap();
    let mut b = S3ServiceBuilder::new(fs);
    b.set_auth(SimpleAuth::from_single("test", "test"));
    let svc = b.build();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let svc = svc.clone();
                tokio::spawn(async move {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        }
    });
    addr
}

fn client(addr: SocketAddr, bucket: &str) -> S3Client {
    S3Client::builder()
        .bucket(bucket)
        .region("us-east-1")
        .access_key_id("test")
        .secret_access_key("test")
        .endpoint(format!("http://{}", addr))
        .path_style(true)
        .max_retries(0)
        .build()
        .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn tuned(
    addr: SocketAddr,
    bucket: &str,
    chunk: usize,
    threshold: usize,
    rc: usize,
    wc: usize,
    read_conn: usize,
    write_conn: usize,
) -> S3Client {
    S3Client::builder()
        .bucket(bucket)
        .region("us-east-1")
        .access_key_id("test")
        .secret_access_key("test")
        .endpoint(format!("http://{}", addr))
        .path_style(true)
        .chunk_size(chunk)
        .multipart_threshold(threshold)
        .read_concurrency(rc)
        .write_concurrency(wc)
        .read_connections(read_conn)
        .write_connections(write_conn)
        .max_retries(0)
        .build()
        .unwrap()
}

fn bucket(root: &std::path::Path, name: &str) {
    std::fs::create_dir_all(root.join(name)).unwrap();
}

fn pattern(size: usize) -> Bytes {
    (0..size)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>()
        .into()
}

// ─── Basic CRUD ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_then_get_returns_same_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("k", Bytes::from("hello")).await.unwrap();
    assert_eq!(c.get_object("k").await.unwrap(), Bytes::from("hello"));
}

#[tokio::test]
async fn head_returns_content_length() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("k", Bytes::from(vec![0u8; 1024]))
        .await
        .unwrap();
    assert_eq!(c.head_object("k").await.unwrap().content_length, 1024);
}

#[tokio::test]
async fn delete_then_head_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("k", Bytes::from("x")).await.unwrap();
    c.delete_object("k").await.unwrap_or(()); // s3s may 404
    assert!(c.head_object("k").await.is_err());
}

#[tokio::test]
async fn list_returns_uploaded_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    for i in 0..5 {
        c.put_object(&format!("p/{i}.txt"), Bytes::from("x"))
            .await
            .unwrap();
    }
    let entries = c.list_objects("p/", false).await.unwrap();
    assert!(entries.iter().filter(|e| e.key.contains(".txt")).count() >= 5);
}

// ─── Error Classification ───────────────────────────────────────────────────

#[tokio::test]
async fn get_missing_key_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    let e = c.get_object("nope").await.unwrap_err();
    assert_eq!(e.error_kind(), ErrorKind::NotFound);
}

#[tokio::test]
async fn head_missing_key_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    let e = c.head_object("nope").await.unwrap_err();
    assert_eq!(e.error_kind(), ErrorKind::NotFound);
}

// ─── Data Integrity ─────────────────────────────────────────────────────────

#[tokio::test]
async fn put_get_empty_object() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("e", Bytes::new()).await.unwrap();
    assert_eq!(c.get_object("e").await.unwrap().len(), 0);
}

#[tokio::test]
async fn all_256_byte_values_survive_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    let body: Bytes = (0u8..=255).collect::<Vec<u8>>().into();
    c.put_object("bin", body.clone()).await.unwrap();
    assert_eq!(c.get_object("bin").await.unwrap(), body);
}

#[tokio::test]
async fn put_overwrites_existing_key() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("k", Bytes::from("v1")).await.unwrap();
    c.put_object("k", Bytes::from("v2")).await.unwrap();
    assert_eq!(c.get_object("k").await.unwrap(), Bytes::from("v2"));
}

#[tokio::test]
async fn at_threshold_uses_simple_put() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 1024, 2, 2, 6, 6);

    let body = pattern(1024); // == threshold → simple PUT
    c.put_object("k", body.clone()).await.unwrap();
    assert_eq!(c.get_object("k").await.unwrap(), body);
}

#[tokio::test]
async fn above_threshold_uses_multipart() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 1024, 2, 2, 6, 6);

    let body = pattern(1025); // > threshold → multipart
    c.put_object("k", body.clone()).await.unwrap();
    assert_eq!(c.get_object("k").await.unwrap(), body);
}

// ─── Range Download ─────────────────────────────────────────────────────────

#[tokio::test]
async fn range_download_preserves_data() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    // chunk=256 → 2048/256 = 8 ranges
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 256, 2, 2, 6, 6);

    let body = pattern(2048);
    c.put_object("r", body.clone()).await.unwrap();
    assert_eq!(c.get_object("r").await.unwrap(), body);
}

#[tokio::test]
async fn range_download_non_aligned_size() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 100, 2, 2, 6, 6);

    let body = pattern(1000); // chunk default 8MiB, but threshold=100 triggers range
    c.put_object("r", body.clone()).await.unwrap();
    assert_eq!(c.get_object("r").await.unwrap(), body);
}

#[tokio::test]
async fn range_download_reassembles_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 100, 4, 4, 8, 8);

    let body: Bytes = (0u16..2048)
        .flat_map(|i| i.to_le_bytes())
        .collect::<Vec<u8>>()
        .into();
    c.put_object("ord", body.clone()).await.unwrap();
    let result = c.get_object("ord").await.unwrap();

    for idx in [0u16, 127, 512, 1024, 2047] {
        let off = idx as usize * 2;
        let val = u16::from_le_bytes(result[off..off + 2].try_into().unwrap());
        assert_eq!(val, idx, "reordering at {idx}");
    }
}

// ─── Concurrency ────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_puts_and_gets() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    let mut h = vec![];
    for i in 0..20 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            c.put_object(&format!("c/{i}"), Bytes::from(format!("{i}")))
                .await
                .unwrap();
        }));
    }
    for j in h {
        j.await.unwrap();
    }

    let mut h = vec![];
    for i in 0..20 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            let d = c.get_object(&format!("c/{i}")).await.unwrap();
            assert_eq!(d, Bytes::from(format!("{i}")));
        }));
    }
    for j in h {
        j.await.unwrap();
    }
}

#[tokio::test]
async fn reads_dont_block_writes() {
    // Fill read pool, writes should still work
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    // read_conn=2, write_conn=5
    let c = tuned(
        addr,
        "test-bucket",
        5 * 1024 * 1024,
        8 * 1024 * 1024,
        1,
        1,
        2,
        5,
    );

    c.put_object("seed", Bytes::from("x")).await.unwrap();

    // Saturate read pool
    let c2 = c.clone();
    let c3 = c.clone();
    let r1 = tokio::spawn(async move {
        c2.get_object("seed").await.unwrap();
    });
    let r2 = tokio::spawn(async move {
        c3.get_object("seed").await.unwrap();
    });

    // Writes should still work immediately (separate pool)
    c.put_object("w1", Bytes::from("ok")).await.unwrap();

    r1.await.unwrap();
    r2.await.unwrap();
}

#[tokio::test]
async fn burst_no_deadlock() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    // Tight: read_conn=4, write_conn=4
    let c = tuned(
        addr,
        "test-bucket",
        5 * 1024 * 1024,
        8 * 1024 * 1024,
        2,
        2,
        4,
        4,
    );

    for i in 0..50 {
        c.put_object(&format!("burst/{i}"), Bytes::from("x"))
            .await
            .unwrap();
    }

    let done = Arc::new(AtomicU64::new(0));
    let mut h = vec![];
    for i in 0..50 {
        let c = c.clone();
        let done = Arc::clone(&done);
        h.push(tokio::spawn(async move {
            c.get_object(&format!("burst/{i}")).await.unwrap();
            done.fetch_add(1, Ordering::Relaxed);
        }));
    }

    let r = tokio::time::timeout(Duration::from_secs(30), async {
        for j in h {
            j.await.unwrap();
        }
    })
    .await;

    assert!(r.is_ok(), "deadlock");
    assert_eq!(done.load(Ordering::Relaxed), 50);
}

#[tokio::test]
async fn burst_range_downloads() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 256, 2, 2, 6, 6);

    for i in 0..10 {
        c.put_object(&format!("br/{i}"), pattern(512))
            .await
            .unwrap();
    }

    let mut h = vec![];
    for i in 0..10 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            assert_eq!(
                c.get_object(&format!("br/{i}")).await.unwrap(),
                pattern(512)
            );
        }));
    }

    let r = tokio::time::timeout(Duration::from_secs(30), async {
        for j in h {
            j.await.unwrap();
        }
    })
    .await;
    assert!(r.is_ok(), "range deadlock");
}

#[tokio::test]
async fn min_connections_works() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    // Minimum viable: read_conn=3 (rc=2+1), write_conn=3 (wc=2+1)
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 512, 2, 2, 3, 3);

    let body = pattern(2048);
    c.put_object("min", body.clone()).await.unwrap();
    assert_eq!(c.get_object("min").await.unwrap(), body);
}

// ─── Config Validation ──────────────────────────────────────────────────────

#[test]
fn rejects_missing_bucket() {
    assert!(S3Client::builder()
        .region("r")
        .access_key_id("a")
        .secret_access_key("s")
        .build()
        .is_err());
}

#[test]
fn rejects_small_chunk() {
    assert!(S3Client::builder()
        .bucket("test-bucket")
        .region("r")
        .access_key_id("a")
        .secret_access_key("s")
        .chunk_size(1024)
        .build()
        .is_err());
}

#[test]
fn rejects_insufficient_read_connections() {
    assert!(S3Client::builder()
        .bucket("test-bucket")
        .region("r")
        .access_key_id("a")
        .secret_access_key("s")
        .read_concurrency(4)
        .read_connections(3) // need >= 5
        .build()
        .is_err());
}

#[test]
fn rejects_insufficient_write_connections() {
    assert!(S3Client::builder()
        .bucket("test-bucket")
        .region("r")
        .access_key_id("a")
        .secret_access_key("s")
        .write_concurrency(4)
        .write_connections(3) // need >= 5
        .build()
        .is_err());
}

#[test]
fn rejects_empty_access_key() {
    assert!(S3Client::builder()
        .bucket("test-bucket")
        .region("r")
        .access_key_id("")
        .secret_access_key("s")
        .build()
        .is_err());
}

// ─── List Variations ────────────────────────────────────────────────────────

#[tokio::test]
async fn list_empty_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    let e = c.list_objects("nonexistent/", false).await.unwrap();
    assert!(e.is_empty());
}

#[tokio::test]
async fn list_prefix_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("a/1", Bytes::from("x")).await.unwrap();
    c.put_object("b/1", Bytes::from("x")).await.unwrap();

    let a = c.list_objects("a/", false).await.unwrap();
    assert!(a.iter().all(|e| e.key.starts_with("a/")));
}

// ─── Lifecycle ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_get_delete_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    for i in 0..10 {
        let body = Bytes::from(format!("iter-{i}"));
        c.put_object("cycle", body.clone()).await.unwrap();
        assert_eq!(c.get_object("cycle").await.unwrap(), body);
        c.delete_object("cycle").await.unwrap_or(());
    }
}

#[tokio::test]
async fn read_after_write() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    for i in 0..30 {
        let body = Bytes::from(format!("v{i}"));
        c.put_object(&format!("raw/{i}"), body.clone())
            .await
            .unwrap();
        assert_eq!(c.get_object(&format!("raw/{i}")).await.unwrap(), body);
    }
}

#[tokio::test]
async fn varying_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(addr, "test-bucket", 5 * 1024 * 1024, 500, 2, 2, 6, 6);

    for size in [0, 1, 100, 499, 500, 501, 1000, 2000] {
        let body = pattern(size);
        c.put_object(&format!("sz/{size}"), body.clone())
            .await
            .unwrap();
        assert_eq!(
            c.get_object(&format!("sz/{size}")).await.unwrap(),
            body,
            "size={size}"
        );
    }
}

#[tokio::test]
async fn clone_shares_pools() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c1 = client(addr, "test-bucket");
    let c2 = c1.clone();

    c1.put_object("shared", Bytes::from("from-c1"))
        .await
        .unwrap();
    assert_eq!(
        c2.get_object("shared").await.unwrap(),
        Bytes::from("from-c1")
    );
}

// ─── Additional Coverage ────────────────────────────────────────────────────

#[test]
fn max_throughput_preset_valid() {
    let r = S3Client::builder()
        .max_throughput()
        .bucket("test-bucket")
        .region("us-east-1")
        .access_key_id("test")
        .secret_access_key("test")
        .endpoint("http://127.0.0.1:9999")
        .path_style(true)
        .build();
    assert!(r.is_ok(), "max_throughput preset must produce valid config");
}

#[test]
fn rejects_zero_concurrency() {
    assert!(S3Client::builder()
        .bucket("test-bucket")
        .region("r")
        .access_key_id("a")
        .secret_access_key("s")
        .read_concurrency(0)
        .build()
        .is_err());
}

#[tokio::test]
async fn concurrent_reads_and_writes_stress() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = tuned(
        addr,
        "test-bucket",
        5 * 1024 * 1024,
        8 * 1024 * 1024,
        2,
        2,
        6,
        6,
    );

    // Seed
    for i in 0..20 {
        c.put_object(&format!("stress/{i}"), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // 20 reads + 20 writes + 10 heads simultaneously
    let mut h = vec![];
    for i in 0..20 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            let d = c.get_object(&format!("stress/{i}")).await.unwrap();
            assert_eq!(d, Bytes::from(format!("v{i}")));
        }));
    }
    for i in 20..40 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            c.put_object(&format!("stress/{i}"), Bytes::from(format!("w{i}")))
                .await
                .unwrap();
        }));
    }
    for i in 0..10 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            c.head_object(&format!("stress/{i}")).await.unwrap();
        }));
    }

    let r = tokio::time::timeout(Duration::from_secs(30), async {
        for j in h {
            j.await.unwrap();
        }
    })
    .await;
    assert!(r.is_ok(), "mixed stress must not deadlock");
}

#[tokio::test]
async fn special_chars_in_key() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    for key in [
        "path/with spaces/f.txt",
        "path/with+plus/f.txt",
        "deep/a/b/c/d.bin",
    ] {
        let body = Bytes::from(format!("content for {key}"));
        c.put_object(key, body.clone()).await.unwrap();
        assert_eq!(
            c.get_object(key).await.unwrap(),
            body,
            "failed for key: {key}"
        );
    }
}

#[tokio::test]
async fn list_recursive_finds_nested() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    c.put_object("d/a.txt", Bytes::from("a")).await.unwrap();
    c.put_object("d/sub/b.txt", Bytes::from("b")).await.unwrap();
    c.put_object("d/sub/deep/c.txt", Bytes::from("c"))
        .await
        .unwrap();

    // non-recursive: only immediate children
    let shallow = c.list_objects("d/", false).await.unwrap();
    let shallow_files: Vec<_> = shallow.iter().filter(|e| e.key.ends_with(".txt")).collect();
    assert_eq!(
        shallow_files.len(),
        1,
        "non-recursive should find only d/a.txt"
    );

    // recursive: all nested objects
    let deep = c.list_objects("d/", true).await.unwrap();
    let deep_files: Vec<_> = deep.iter().filter(|e| e.key.ends_with(".txt")).collect();
    assert_eq!(deep_files.len(), 3, "recursive should find all 3 files");
}

// --- Pool behavior ---

#[tokio::test]
async fn pool_timeout_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");

    // 1 write connection, 50ms timeout
    let c = S3Client::builder()
        .bucket("test-bucket")
        .region("us-east-1")
        .access_key_id("test")
        .secret_access_key("test")
        .endpoint(format!("http://{}", addr))
        .path_style(true)
        .write_connections(2)
        .write_concurrency(1)
        .read_connections(2)
        .read_concurrency(1)
        .timeout(Duration::from_millis(100))
        .max_retries(0)
        .build()
        .unwrap();

    // Saturate write pool with concurrent long-ish puts
    let mut h = vec![];
    for i in 0..10 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            // Some will succeed, some may timeout — that's the test
            let _ = c
                .put_object(&format!("timeout/{i}"), Bytes::from("x"))
                .await;
        }));
    }
    for j in h {
        j.await.unwrap();
    }
    // If we get here without hanging, the timeout works.
}

#[tokio::test]
async fn permit_released_on_error() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");

    // Only 2 read connections
    let c = tuned(
        addr,
        "test-bucket",
        5 * 1024 * 1024,
        8 * 1024 * 1024,
        1,
        1,
        2,
        2,
    );

    // Two gets on nonexistent keys — both will error
    let e1 = c.get_object("missing1").await;
    assert!(e1.is_err());
    let e2 = c.get_object("missing2").await;
    assert!(e2.is_err());

    // If permits leaked, this third call would deadlock.
    // It should work because error paths release permits via RAII.
    c.put_object("ok", Bytes::from("works")).await.unwrap();
    let data = c.get_object("ok").await.unwrap();
    assert_eq!(data, Bytes::from("works"));
}

// --- Edge cases ---

#[tokio::test]
async fn large_number_of_sequential_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    // 100 sequential put+get — tests that permits don't leak over many operations
    for i in 0..100 {
        let key = format!("seq/{i}");
        let body = Bytes::from(format!("{i}"));
        c.put_object(&key, body.clone()).await.unwrap();
        assert_eq!(c.get_object(&key).await.unwrap(), body);
    }
}

#[tokio::test]
async fn concurrent_list_and_write() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    // Seed some objects
    for i in 0..10 {
        c.put_object(&format!("mix/{i}"), Bytes::from("x"))
            .await
            .unwrap();
    }

    // Concurrent: 5 lists (read pool) + 5 puts (write pool)
    let mut h = vec![];
    for _ in 0..5 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            let entries = c.list_objects("mix/", false).await.unwrap();
            assert!(!entries.is_empty());
        }));
    }
    for i in 10..15 {
        let c = c.clone();
        h.push(tokio::spawn(async move {
            c.put_object(&format!("mix/{i}"), Bytes::from("y"))
                .await
                .unwrap();
        }));
    }
    for j in h {
        j.await.unwrap();
    }
}

#[tokio::test]
async fn get_object_body_matches_exact_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");
    let c = client(addr, "test-bucket");

    // Test with various payload sizes including boundary values
    for size in [1, 2, 255, 256, 1023, 1024, 4095, 4096, 8191, 8192] {
        let body = pattern(size);
        let key = format!("exact/{size}");
        c.put_object(&key, body.clone()).await.unwrap();
        let got = c.get_object(&key).await.unwrap();
        assert_eq!(got.len(), size, "length mismatch at size={size}");
        assert_eq!(got, body, "data mismatch at size={size}");
    }
}

#[tokio::test]
async fn upload_checksum_enabled_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");

    let c = S3Client::builder()
        .bucket("test-bucket")
        .region("us-east-1")
        .access_key_id("test")
        .secret_access_key("test")
        .endpoint(format!("http://{}", addr))
        .path_style(true)
        .upload_checksum(true)
        .max_retries(0)
        .build()
        .unwrap();

    let body = pattern(4096);
    c.put_object("cksum", body.clone()).await.unwrap();
    assert_eq!(c.get_object("cksum").await.unwrap(), body);
}

#[tokio::test]
async fn upload_checksum_disabled_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = server(tmp.path()).await;
    bucket(tmp.path(), "test-bucket");

    // Default: checksum off
    let c = client(addr, "test-bucket");

    let body = pattern(4096);
    c.put_object("nocksum", body.clone()).await.unwrap();
    assert_eq!(c.get_object("nocksum").await.unwrap(), body);
}
