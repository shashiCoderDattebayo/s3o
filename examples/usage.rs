//! Complete usage example.
//!
//! Start a local S3-compatible server (e.g. MinIO):
//!   docker run -p 9000:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin minio/minio server /data
//!
//! Run:
//!   cargo run --example usage

use std::time::Duration;

use bytes::Bytes;
use s3o::{ErrorKind, S3Client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- Client initialization ---

    // Default settings (15 read / 10 write connections, 8 MiB chunks)
    let client = S3Client::builder()
        .bucket("test-bucket")
        .region("us-east-1")
        .access_key_id("minioadmin")
        .secret_access_key("minioadmin")
        .endpoint("http://localhost:9000")
        .path_style(true)
        .build()?;

    // Or: max throughput for ML checkpoints (64 MiB chunks, 16x concurrency)
    let _fast = S3Client::builder()
        .max_throughput()
        .bucket("checkpoints")
        .region("us-east-1")
        .access_key_id("minioadmin")
        .secret_access_key("minioadmin")
        .endpoint("http://localhost:9000")
        .path_style(true)
        .build()?;

    // Or: fully custom pool sizing
    let _custom = S3Client::builder()
        .bucket("test-bucket")
        .region("us-east-1")
        .access_key_id("minioadmin")
        .secret_access_key("minioadmin")
        .endpoint("http://localhost:9000")
        .path_style(true)
        .read_connections(30)
        .write_connections(10)
        .read_concurrency(8)
        .write_concurrency(4)
        .chunk_size(16 * 1024 * 1024)
        .multipart_threshold(16 * 1024 * 1024)
        .timeout(Duration::from_secs(120))
        .max_retries(5)
        .build()?;

    // --- Operations ---

    // put (auto-multipart for large objects)
    client
        .put_object("greeting.txt", Bytes::from("hello"))
        .await?;
    client
        .put_object("data/file1.txt", Bytes::from("one"))
        .await?;
    client
        .put_object("data/sub/file2.txt", Bytes::from("two"))
        .await?;
    println!("uploaded 3 objects");

    // get (auto-range download for large objects)
    let data = client.get_object("greeting.txt").await?;
    println!("get: {}", String::from_utf8_lossy(&data));

    // head
    let meta = client.head_object("greeting.txt").await?;
    println!("head: {} bytes, etag={:?}", meta.content_length, meta.etag);

    // list (non-recursive — immediate children only)
    let entries = client.list_objects("data/", false).await?;
    println!("list data/ (non-recursive): {} entries", entries.len());
    for e in &entries {
        println!("  {} ({} bytes)", e.key, e.size);
    }

    // list (recursive — all objects at any depth)
    let all = client.list_objects("data/", true).await?;
    println!("list data/ (recursive): {} entries", all.len());
    for e in &all {
        println!("  {} ({} bytes)", e.key, e.size);
    }

    // delete
    client.delete_object("greeting.txt").await?;
    println!("deleted greeting.txt");

    // --- Error handling ---

    match client.get_object("nonexistent").await {
        Ok(_) => {}
        Err(e) => match e.error_kind() {
            ErrorKind::NotFound => println!("expected: key not found"),
            ErrorKind::Auth => println!("check credentials"),
            ErrorKind::Throttled => println!("rate limited"),
            ErrorKind::Timeout => println!("timed out"),
            kind => println!("error ({kind}): {e}"),
        },
    }

    Ok(())
}
