//! HTTP tests for the admin surface.
//!
//! The admin surface is opt-in: it only exists when the caller constructs an
//! `AdminConfig` and passes it to `build_app_with_admin`. Tests confirm
//! both that the happy path works and that the admin routes are absent when
//! no admin config is supplied.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use lora_database::{Database, SnapshotAdmin};
use tower::ServiceExt;

use lora_server::{build_app, build_app_with_admin, AdminConfig};

fn tempdir(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "lora-admin-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn admin_routes_absent_without_config() {
    let db = Arc::new(Database::in_memory());
    let app = build_app(db);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/snapshot/save")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_save_and_load_roundtrip() {
    let dir = tempdir("save_load");
    let snapshot_path = dir.join("snap.bin");

    let db = Arc::new(Database::in_memory());
    db.execute(
        "CREATE (:Person {name: 'alice'})",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();

    let admin = AdminConfig {
        snapshot_path: snapshot_path.clone(),
        admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
    };
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    // Save.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/snapshot/save")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["nodeCount"], 1);
    assert_eq!(json["walLsn"], serde_json::Value::Null);
    assert!(snapshot_path.exists());

    // Mutate — adds a second node so we can verify the next load reverts it.
    db.execute(
        "CREATE (:Person {name: 'bob'})",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();
    assert_eq!(db.node_count(), 2);

    // Load.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/snapshot/load")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["nodeCount"], 1);
    assert_eq!(db.node_count(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn admin_save_honours_path_override() {
    let dir = tempdir("path_override");
    let default_path = dir.join("default.bin");
    let override_path = dir.join("override.bin");

    let db = Arc::new(Database::in_memory());
    db.execute(
        "CREATE (:N)",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();

    let admin = AdminConfig {
        snapshot_path: default_path.clone(),
        admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
    };
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    // Save with an explicit path override — default_path must NOT be written.
    let body = serde_json::json!({ "path": override_path.to_str().unwrap() }).to_string();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/snapshot/save")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["path"].as_str().unwrap(),
        override_path.display().to_string()
    );
    assert!(override_path.exists());
    assert!(!default_path.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn admin_load_reports_error_for_missing_file() {
    let dir = tempdir("missing");
    let snapshot_path = dir.join("nope.bin");

    let db = Arc::new(Database::in_memory());
    let admin = AdminConfig {
        snapshot_path,
        admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
    };
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/snapshot/load")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let _ = std::fs::remove_dir_all(&dir);
}
