use chrono::Utc;
use mitmrs::model::{FlowRecord, HeaderEntry, SearchQuery};
use mitmrs::store::Store;
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
