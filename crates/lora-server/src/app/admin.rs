use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use lora_database::{LoraErrorCode, SnapshotAdmin, SnapshotMeta, WalAdmin};

use super::errors::{json_rejection_response, lora_error_response, ErrorResponse};
use super::types::{SnapshotRequest, SnapshotResponse, WalStatusResponse, WalTruncateRequest};

/// Snapshot admin surface. Mounted as a unit so that
/// `/admin/snapshot/{save,load}` always have a configured default
/// path: an operator who set `--snapshot-path` is the one paying the
/// cost of the route's existence, and they reasonably expect the
/// path to be resolved automatically when no `path` field is sent in
/// the request body.
#[derive(Clone)]
pub struct SnapshotAdminConfig {
    pub path: PathBuf,
    pub admin: Arc<dyn SnapshotAdmin>,
}

/// Configuration for the admin surface. Snapshot and WAL admin are
/// independent: each set of routes mounts only when its corresponding
/// field is `Some`.
///
/// - `snapshot.is_some()` mounts `POST /admin/snapshot/save` and
///   `POST /admin/snapshot/load` against the configured path
///   (the body's optional `path` field overrides per request).
/// - `wal.is_some()` mounts `POST /admin/wal/status` and
///   `POST /admin/wal/truncate` unconditionally, plus
///   `POST /admin/checkpoint` (which uses `snapshot.path` as a default
///   when present, and otherwise requires `path` in the request body).
///
/// The endpoints are intentionally opt-in: exposing them without
/// authentication on a network-reachable interface is a footgun, so
/// the caller must explicitly construct an `AdminConfig` and pass it
/// to the server. There is no implicit default path.
///
/// A request body `path` override is rejected unless `path_root` is set,
/// and then it must stay inside that directory. This limits which files
/// a caller can overwrite or read; it is not authentication.
#[derive(Clone, Default)]
pub struct AdminConfig {
    /// Snapshot save/load admin. `None` to disable
    /// `/admin/snapshot/{save,load}`.
    pub snapshot: Option<SnapshotAdminConfig>,
    /// WAL admin. `None` to disable `/admin/wal/*` and
    /// `/admin/checkpoint`.
    pub wal: Option<Arc<dyn WalAdmin>>,
    /// Directory that request-body `path` overrides must stay inside.
    /// `None` rejects every override. Should be an absolute path.
    pub path_root: Option<PathBuf>,
}

impl AdminConfig {
    /// Construct a snapshot-only admin config (no WAL endpoints).
    pub fn snapshot_only(snapshot_path: PathBuf, admin: Arc<dyn SnapshotAdmin>) -> Self {
        Self {
            snapshot: Some(SnapshotAdminConfig {
                path: snapshot_path,
                admin,
            }),
            wal: None,
            path_root: None,
        }
    }

    /// Construct a WAL-only admin config (no snapshot endpoints). The
    /// `/admin/checkpoint` route still mounts but every call needs a
    /// `path` in the request body since there is no configured
    /// default.
    pub fn wal_only(wal: Arc<dyn WalAdmin>) -> Self {
        Self {
            snapshot: None,
            wal: Some(wal),
            path_root: None,
        }
    }

    /// True when neither admin surface is configured. The router
    /// merge then becomes a no-op and the admin routes don't exist.
    /// Allow request-body `path` overrides inside `root`.
    pub fn with_path_root(mut self, root: PathBuf) -> Self {
        self.path_root = Some(root);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.snapshot.is_none() && self.wal.is_none()
    }
}

pub(crate) fn build_admin_router(cfg: AdminConfig) -> Router {
    let mut router = Router::new();

    if let Some(snap) = cfg.snapshot.clone() {
        let snapshot_state = SnapshotAdminState {
            cfg: snap,
            path_root: cfg.path_root.clone(),
        };
        let snapshot_router: Router = Router::new()
            .route("/admin/snapshot/save", post(admin_snapshot_save))
            .route("/admin/snapshot/load", post(admin_snapshot_load))
            .with_state(snapshot_state);
        router = router.merge(snapshot_router);
    }

    if let Some(wal) = cfg.wal.clone() {
        let wal_state = WalAdminState {
            // Reuse the snapshot path as the default checkpoint
            // target when present so a body-less
            // `POST /admin/checkpoint` writes to the same file the
            // snapshot endpoints use. When no snapshot path is
            // configured, the handler requires `path` in the body.
            default_checkpoint_path: cfg.snapshot.as_ref().map(|s| s.path.clone()),
            path_root: cfg.path_root.clone(),
            wal,
        };
        let wal_router: Router = Router::new()
            .route("/admin/checkpoint", post(admin_checkpoint))
            .route("/admin/wal/status", post(admin_wal_status))
            .route("/admin/wal/truncate", post(admin_wal_truncate))
            .with_state(wal_state);
        router = router.merge(wal_router);
    }

    router
}

/// State plumbed into the WAL admin handlers.
#[derive(Clone)]
struct WalAdminState {
    /// Default target for `POST /admin/checkpoint` when the body
    /// omits `path`. `None` when the operator did not pass
    /// `--snapshot-path`; in that case the handler returns 400 with
    /// a hint.
    default_checkpoint_path: Option<PathBuf>,
    path_root: Option<PathBuf>,
    wal: Arc<dyn WalAdmin>,
}

/// State plumbed into the snapshot admin handlers.
#[derive(Clone)]
struct SnapshotAdminState {
    cfg: SnapshotAdminConfig,
    path_root: Option<PathBuf>,
}

/// The request-body `path` override, if one was sent.
fn requested_path(req: Option<&SnapshotRequest>) -> Option<&str> {
    req.and_then(|r| r.path.as_deref())
        .filter(|p| !p.trim().is_empty())
}

/// Check a request-body `path` override against the configured root.
///
/// The override may be relative (joined onto the root) or absolute, but
/// must not contain `..` and must land inside the root. Without a root
/// every override is rejected. The check is lexical: a symlink inside the
/// root that points elsewhere is followed, so keep the root operator-owned.
fn confine_override(root: Option<&Path>, raw: &str) -> Result<PathBuf, String> {
    let Some(root) = root else {
        return Err(
            "`path` overrides are disabled: start the server with --admin-path-root <DIR> to allow paths inside DIR"
                .to_string(),
        );
    };
    let requested = Path::new(raw);
    if requested
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!("`path` must not contain `..`: {raw}"));
    }
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if resolved == root || !resolved.starts_with(root) {
        return Err(format!(
            "`path` must name a file inside the admin path root {}: {raw}",
            root.display()
        ));
    }
    Ok(resolved)
}

/// Extract the target path for a snapshot operation: the (confined)
/// request-body override if present, else the configured default.
fn resolve_snapshot_path(
    state: &SnapshotAdminState,
    req: Option<&SnapshotRequest>,
) -> Result<PathBuf, String> {
    match requested_path(req) {
        Some(p) => confine_override(state.path_root.as_deref(), p),
        None => Ok(state.cfg.path.clone()),
    }
}

fn bad_path_response(msg: impl Into<String>) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse::from_parts(LoraErrorCode::Config, msg)),
    )
        .into_response()
}

async fn admin_snapshot_save(
    State(state): State<SnapshotAdminState>,
    body: Result<Json<SnapshotRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match parse_optional_json(body) {
        Ok(req) => req,
        Err(err) => return json_rejection_response(err),
    };
    let path = match resolve_snapshot_path(&state, req.as_ref()) {
        Ok(p) => p,
        Err(msg) => return bad_path_response(msg),
    };

    match state.cfg.admin.save_snapshot(&path) {
        Ok(meta) => snapshot_response(meta, path),
        Err(err) => lora_error_response(err),
    }
}

async fn admin_snapshot_load(
    State(state): State<SnapshotAdminState>,
    body: Result<Json<SnapshotRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match parse_optional_json(body) {
        Ok(req) => req,
        Err(err) => return json_rejection_response(err),
    };
    let path = match resolve_snapshot_path(&state, req.as_ref()) {
        Ok(p) => p,
        Err(msg) => return bad_path_response(msg),
    };

    match state.cfg.admin.load_snapshot(&path) {
        Ok(meta) => snapshot_response(meta, path),
        Err(err) => lora_error_response(err),
    }
}

fn resolve_checkpoint_path(
    state: &WalAdminState,
    req: Option<&SnapshotRequest>,
) -> Result<PathBuf, String> {
    match requested_path(req) {
        Some(p) => confine_override(state.path_root.as_deref(), p),
        None => state.default_checkpoint_path.clone().ok_or_else(|| {
            "no checkpoint path: start the server with --snapshot-path, or pass `path` in the request body (needs --admin-path-root)"
                .to_string()
        }),
    }
}

async fn admin_checkpoint(
    State(state): State<WalAdminState>,
    body: Result<Json<SnapshotRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match parse_optional_json(body) {
        Ok(req) => req,
        Err(err) => return json_rejection_response(err),
    };
    let path = match resolve_checkpoint_path(&state, req.as_ref()) {
        Ok(p) => p,
        Err(msg) => return bad_path_response(msg),
    };

    match state.wal.checkpoint(&path) {
        Ok(meta) => snapshot_response(meta, path),
        Err(err) => lora_error_response(err),
    }
}

fn snapshot_response(meta: SnapshotMeta, path: PathBuf) -> axum::response::Response {
    (
        StatusCode::OK,
        Json(SnapshotResponse {
            format_version: meta.format_version,
            node_count: meta.node_count as u64,
            relationship_count: meta.relationship_count as u64,
            wal_lsn: meta.wal_lsn,
            path: path.display().to_string(),
        }),
    )
        .into_response()
}

async fn admin_wal_status(State(state): State<WalAdminState>) -> impl IntoResponse {
    match state.wal.wal_status() {
        Ok(s) => (
            StatusCode::OK,
            Json(WalStatusResponse {
                durable_lsn: s.durable_lsn,
                next_lsn: s.next_lsn,
                active_segment_id: s.active_segment_id,
                oldest_segment_id: s.oldest_segment_id,
                bg_failure: s.bg_failure,
            }),
        )
            .into_response(),
        Err(err) => lora_error_response(err),
    }
}

async fn admin_wal_truncate(
    State(state): State<WalAdminState>,
    body: Result<Json<WalTruncateRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match parse_optional_json(body) {
        Ok(req) => req,
        Err(err) => return json_rejection_response(err),
    };
    // No body / no fence => truncate up to the WAL's current durable
    // LSN. That's the natural "drop everything safe to drop" default.
    let fence = match req.and_then(|r| r.fence_lsn) {
        Some(lsn) => lsn,
        None => match state.wal.wal_status() {
            Ok(s) => s.durable_lsn,
            Err(err) => return lora_error_response(err),
        },
    };

    match state.wal.wal_truncate(fence) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => lora_error_response(err),
    }
}

fn parse_optional_json<T>(
    body: Result<Json<T>, JsonRejection>,
) -> Result<Option<T>, JsonRejection> {
    match body {
        Ok(Json(value)) => Ok(Some(value)),
        Err(JsonRejection::MissingJsonContentType(_)) => Ok(None),
        Err(err) => Err(err),
    }
}
