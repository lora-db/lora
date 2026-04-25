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
use lora_database::{Database, SnapshotAdmin, SyncMode, WalAdmin, WalConfig};
use tower::ServiceExt;

use lora_server::{build_app, build_app_with_admin, AdminConfig, SnapshotAdminConfig};

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
        snapshot: Some(SnapshotAdminConfig {
            path: snapshot_path.clone(),
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }),
        wal: None,
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
        snapshot: Some(SnapshotAdminConfig {
            path: default_path.clone(),
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }),
        wal: None,
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
        snapshot: Some(SnapshotAdminConfig {
            path: snapshot_path,
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }),
        wal: None,
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

// ---------------------------------------------------------------------------
// WAL admin endpoints
// ---------------------------------------------------------------------------

fn enabled(wal_dir: &std::path::Path) -> WalConfig {
    WalConfig::Enabled {
        dir: wal_dir.to_path_buf(),
        sync_mode: SyncMode::PerCommit,
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

#[tokio::test]
async fn wal_routes_absent_without_wal_admin() {
    // Snapshot-only admin config: WAL endpoints must not be mounted
    // even if a WAL is available. Operators have to opt in by setting
    // `AdminConfig.wal = Some(...)`.
    let dir = tempdir("wal-absent");
    let snapshot_path = dir.join("snap.bin");

    let db = Arc::new(Database::in_memory());
    let admin = AdminConfig::snapshot_only(
        snapshot_path,
        Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
    );
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    for path in ["/admin/checkpoint", "/admin/wal/status", "/admin/wal/truncate"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} should not be mounted without AdminConfig.wal"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn wal_status_and_checkpoint_endpoints() {
    let dir = tempdir("wal-status");
    let wal_dir = dir.join("wal");
    std::fs::create_dir_all(&wal_dir).unwrap();
    let snapshot_path = dir.join("snap.bin");

    let db = Arc::new(Database::open_with_wal(enabled(&wal_dir)).unwrap());
    db.execute(
        "CREATE (:N {id: 1})",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();
    db.execute(
        "CREATE (:N {id: 2})",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();

    let admin = AdminConfig {
        snapshot: Some(SnapshotAdminConfig {
            path: snapshot_path.clone(),
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }),
        wal: Some(Arc::clone(&db) as Arc<dyn WalAdmin>),
    };
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    // Status reflects committed traffic.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/wal/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let durable = json["durableLsn"].as_u64().unwrap();
    assert!(durable > 0, "durableLsn should advance with committed writes");

    // Checkpoint writes a snapshot at the configured path with a
    // non-null walLsn.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/checkpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["nodeCount"], 2);
    assert!(json["walLsn"].is_u64(), "checkpoint must stamp walLsn");
    assert!(snapshot_path.exists());

    // wal/truncate without a body uses the durable LSN as fence and
    // returns 204.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/wal/truncate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn wal_admin_routes_mount_without_snapshot_path() {
    // WAL-only admin: only --wal-dir was set, no --snapshot-path.
    // /admin/wal/{status,truncate} must still mount;
    // /admin/snapshot/{save,load} must not.
    let dir = tempdir("wal-only");
    let wal_dir = dir.join("wal");
    std::fs::create_dir_all(&wal_dir).unwrap();

    let db = Arc::new(Database::open_with_wal(enabled(&wal_dir)).unwrap());
    db.execute(
        "CREATE (:N {id: 1})",
        Some(lora_database::ExecuteOptions {
            format: lora_database::ResultFormat::RowArrays,
        }),
    )
    .unwrap();

    let admin = AdminConfig::wal_only(Arc::clone(&db) as Arc<dyn WalAdmin>);
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    // wal/status mounts.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/wal/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // wal/truncate mounts.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/wal/truncate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // /admin/checkpoint without a configured default path AND without
    // a body path → 400 with a hint.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/checkpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("--snapshot-path"),
        "error should mention --snapshot-path or `path` body"
    );

    // /admin/checkpoint WITH a body path works.
    let body = serde_json::json!({ "path": dir.join("ckpt.bin").to_str().unwrap() })
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/checkpoint")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Snapshot save/load routes must NOT be mounted.
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

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn wal_status_errors_when_wal_admin_is_disconnected() {
    // Edge case: an `AdminConfig.wal` that points at a database
    // without a live WAL (e.g. WalConfig::Disabled). The endpoint
    // surfaces the trait error as a 500 with a useful message
    // rather than panicking.
    let dir = tempdir("wal-err");
    let snapshot_path = dir.join("snap.bin");

    let db = Arc::new(Database::in_memory()); // no WAL attached
    let admin = AdminConfig {
        snapshot: Some(SnapshotAdminConfig {
            path: snapshot_path,
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }),
        wal: Some(Arc::clone(&db) as Arc<dyn WalAdmin>),
    };
    let app = build_app_with_admin(Arc::clone(&db), Some(admin));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/wal/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["error"].as_str().unwrap().contains("WAL"));

    let _ = std::fs::remove_dir_all(&dir);
}
