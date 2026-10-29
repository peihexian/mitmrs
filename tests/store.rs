use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use mitmrs::model::{FlowRecord, HeaderEntry, SearchQuery};
use mitmrs::store::Store;
use std::io::Write;
use std::path::PathBuf;

fn flow(seq: usize, body: &str, host: &str) -> FlowRecord {
    let mut rec = FlowRecord::new(
        format!("00000000-0000-7000-8000-{seq:012}"),
        Utc::now(),
        "127.0.0.1:5555".into(),
        "https".into(),
        host.into(),
        443,
        "POST".into(),
        format!("/api/{seq}"),
        None,
        "HTTP/1.1".into(),
        Some(host.into()),
    );
    rec.request_headers = vec![HeaderEntry::new("content-type", "application/json")];
    rec.response_headers = vec![HeaderEntry::new("content-type", "application/json")];
    rec.request_body = body.as_bytes().to_vec();
    rec.request_body_size = body.len() as u64;
    rec.response_body = format!(r#"{{"ok":true,"n":{seq}}}"#).into_bytes();
    rec.response_body_size = rec.response_body.len() as u64;
    rec.status = Some(200);
    rec.reason = Some("OK".into());
    rec.duration_ms = Some(12);
    rec.ended_at = Some(Utc::now());
    rec.protocol = "http".into();
    rec
}

async fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(PathBuf::from(dir.path()).join("test.db"))
        .await
        .unwrap();
    (store, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_do_not_report_database_locked() {
    let (store, _dir) = temp_store().await;
    let mut tasks = Vec::new();
    for worker in 0..8 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            for index in 0..100 {
                let id = format!("w{worker:02}-{index:03}");
                let mut record = flow(index, r#"{"q":"stress"}"#, "stress.example");
                record.id = id.clone();
                store.insert(record).await.expect("concurrent insert");
                store
                    .update_partial_response(id.clone(), vec![b'x'; 32], 32, false)
                    .await
                    .expect("concurrent partial update");
                store
                    .update_partial_response(id, vec![b'x'; 64], 64, false)
                    .await
                    .expect("concurrent partial update");
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let stats = store.stats().await.unwrap();
    assert_eq!(stats.total, 800);
    assert_eq!(stats.bytes.request, 800 * 14);
    assert_eq!(stats.bytes.response, 800 * 64);
    let record = store
        .get_record("w00-000")
        .await
        .unwrap()
        .expect("flow should exist");
    assert_eq!(record.response_body_size, 64);
}

#[tokio::test]
async fn second_instance_on_the_same_database_is_rejected() {
    let (store, dir) = temp_store().await;
    let db = PathBuf::from(dir.path()).join("test.db");
    let error = Store::open(db.clone())
        .await
        .err()
        .expect("a second instance must not share the database");
    assert!(
        error.to_string().contains("already using"),
        "unexpected error: {error}"
    );
    drop(store);
    // The advisory lock is released with the first store.
    let reopened = Store::open(db).await.expect("lock should be released");
    drop(reopened);
}

#[tokio::test]
async fn insert_search_get_export_import() {
    let (store, _dir) = temp_store().await;
    for i in 0..3 {
        store
            .insert(flow(
                i,
                &format!(r#"{{"message":"needle-{i}"}}"#),
                "api.example.com",
            ))
            .await
            .unwrap();
    }
    let q = SearchQuery {
        body_contains: Some("needle-1".into()),
        direction: Some("request".into()),
        limit: Some(10),
        ..Default::default()
    };
    let found = store.search(&q).await.unwrap();
    assert_eq!(found.total, 1);
    assert_eq!(found.items[0].path, "/api/1");

    let q = SearchQuery {
        host: Some("example".into()),
        method: Some("post".into()),
        status: Some("2xx".into()),
        limit: Some(10),
        ..Default::default()
    };
    let found = store.search(&q).await.unwrap();
    assert_eq!(found.total, 3);

    let id = found.items[0].id.clone();
    let detail = store.get(&id, "text", 1_000_000).await.unwrap().unwrap();
    assert!(detail.request.body_text.unwrap().contains("needle"));
    // Plaintext bodies are left exactly as captured.
    assert!(detail.request.body_decoded.is_none());
    assert!(detail.request.body_base64.is_none());

    let json = store
        .export_json(&SearchQuery::default(), 100)
        .await
        .unwrap();
    let imported_dir = tempfile::tempdir().unwrap();
    let store2 = Store::open(PathBuf::from(imported_dir.path()).join("import.db"))
        .await
        .unwrap();
    let result = store2.import_json(&json).await.unwrap();
    assert_eq!(result.imported, 3);
    let stats = store2.stats().await.unwrap();
    assert_eq!(stats.total, 3);

    let har = store
        .export_har(&SearchQuery::default(), 100)
        .await
        .unwrap();
    let store3 = Store::open(PathBuf::from(imported_dir.path()).join("har.db"))
        .await
        .unwrap();
    let result = store3.import_json(&har).await.unwrap();
    assert_eq!(result.imported, 3);
}

#[tokio::test]
async fn detail_view_inflates_compressed_bodies() {
    let (store, _dir) = temp_store().await;
    let payload = br#"{"gzip":true,"hint":"auto-decoded"}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    let gz = encoder.finish().unwrap();

    let mut record = flow(11, "{}", "gz.example");
    record.response_headers = vec![
        HeaderEntry::new("content-type", "application/json"),
        HeaderEntry::new("content-encoding", "gzip"),
    ];
    record.response_mime = Some("application/json".into());
    record.response_body = gz.clone();
    record.response_body_size = gz.len() as u64;
    store.insert(record).await.unwrap();

    let found = store.search(&SearchQuery::default()).await.unwrap();
    let id = found.items[0].id.clone();
    let detail = store.get(&id, "text", 1_000_000).await.unwrap().unwrap();
    let decoded = detail
        .response
        .body_decoded
        .expect("gzip response should be auto-decoded");
    assert_eq!(decoded.encoding, "gzip");
    assert!(decoded.text);
    assert!(!decoded.truncated);
    assert_eq!(
        detail.response.body_text.as_deref(),
        Some(String::from_utf8_lossy(payload).as_ref())
    );
    // The raw wire bytes stay available for the hex view and downloads.
    assert!(detail.response.body_base64.is_some());
}

#[tokio::test]
async fn delete_and_clear() {
    let (store, _dir) = temp_store().await;
    let seq = store.insert(flow(9, "{}", "a.test")).await.unwrap();
    assert!(seq > 0);
    let found = store.search(&SearchQuery::default()).await.unwrap();
    let id = found.items[0].id.clone();
    assert!(store.delete(&id).await.unwrap());
    assert_eq!(store.stats().await.unwrap().total, 0);
    store.insert(flow(10, "{}", "b.test")).await.unwrap();
    assert_eq!(store.clear().await.unwrap(), 1);
    assert_eq!(store.stats().await.unwrap().total, 0);
}
