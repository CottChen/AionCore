#![allow(clippy::disallowed_types)]

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Json, Multipart, Query, State};
use axum::routing::{get, post};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tower_http::limit::RequestBodyLimitLayer;

use aionui_api_types::{
    ApiResponse, BrowseDirectoryQuery, BrowseDirectoryResponse, CancelZipRequest, CopyFilesRequest, CopyFilesResponse,
    CreateTempFileRequest, DirOrFileResponse, FetchRemoteImageRequest, FileChangeInfoResponse, FileMetadataResponse,
    FileWatchRequest, GetFileMetadataRequest, GetFilesByDirRequest, GetImageBase64Request, ListWorkspaceFilesRequest,
    ReadFileBufferRequest, ReadFileRequest, RemoveEntryRequest, RenameRequest, RenameResponse, SnapshotBaselineRequest,
    SnapshotCompareResponse, SnapshotDiscardRequest, SnapshotInfoResponse, SnapshotStageRequest,
    SnapshotWorkspaceRequest, WorkspaceFlatFileResponse, WorkspaceOfficeWatchRequest, WriteFileRequest, ZipRequest,
};
use aionui_auth::CurrentUser;
use aionui_common::ApiError;
use aionui_common::constants::UPLOAD_MAX_SIZE;

use crate::browse;
use crate::error::FileError;
<<<<<<< HEAD
use crate::traits::{FileServiceRef, FileWatchServiceRef, SnapshotServiceRef, UploadWorkspaceResolverRef};
=======
use crate::traits::{ClipboardWriterRef, FileServiceRef, ItemRevealerRef, SnapshotServiceRef, SystemFileOpenerRef};

/// Request-body cap for `PUT /api/fs/content`, aligned with the 256 MB read cap
/// so large files can be saved (the 10 MB global limit would otherwise 413).
const CONTENT_MAX_SIZE: usize = 256 * 1024 * 1024;
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
use crate::types::{
    CompareResult, CopyResult, DirOrFile, FileChangeInfo, FileMetadata, SnapshotInfo, SnapshotMode, WorkspaceFlatFile,
    ZipEntry,
};

impl From<FileError> for ApiError {
    fn from(error: FileError) -> Self {
        match error {
            FileError::BadRequest(message) => ApiError::BadRequest(message),
            FileError::Forbidden(message) => ApiError::Forbidden(message),
            FileError::PathOutsideSandbox {
                message,
                field,
                operation,
            } => ApiError::PathOutsideSandbox {
                message,
                field,
                operation,
            },
            FileError::NotFound(message) => ApiError::NotFound(message),
            FileError::Internal(message) => ApiError::Internal(message),
        }
    }
}

// ---------------------------------------------------------------------------
// Router state
// ---------------------------------------------------------------------------

type BrowseRootsResolver = dyn Fn() -> Vec<PathBuf> + Send + Sync;

/// Lazily resolves roots for the shallow `/api/fs/browse` endpoint.
#[derive(Clone)]
pub struct BrowseRoots {
    roots: Arc<OnceLock<Vec<PathBuf>>>,
    resolver: Arc<BrowseRootsResolver>,
}

impl BrowseRoots {
    pub fn new() -> Self {
        Self {
            roots: Arc::new(OnceLock::new()),
            resolver: Arc::new(browse::default_browse_roots),
        }
    }

    #[cfg(test)]
    fn with_resolver(resolver: impl Fn() -> Vec<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            roots: Arc::new(OnceLock::new()),
            resolver: Arc::new(resolver),
        }
    }

    fn get(&self) -> Vec<PathBuf> {
        self.roots.get_or_init(|| (self.resolver)()).clone()
    }
}

impl Default for BrowseRoots {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for all file-related route handlers.
#[derive(Clone)]
pub struct FileRouterState {
    pub file_service: FileServiceRef,
    pub upload_workspace_resolver: UploadWorkspaceResolverRef,
    pub watch_service: FileWatchServiceRef,
    pub snapshot_service: SnapshotServiceRef,
<<<<<<< HEAD
=======
    /// Resolves pe-addressed copy/reveal targets (`/api/fs/copy`,
    /// `/api/fs/reveal`) to absolute paths.
    pub project: Arc<aionui_project::ProjectService>,
    /// Reveals a resolved absolute path in the OS file manager
    /// (`/api/fs/reveal`). Injected by composition over the shell service.
    pub revealer: ItemRevealerRef,
    /// Opens a resolved absolute path with the OS default application
    /// (`/api/fs/open-system`). Injected by composition over the shell service.
    pub system_opener: SystemFileOpenerRef,
    /// Writes a resolved absolute path to the OS clipboard
    /// (`/api/fs/copy-absolute-path`). Injected by composition over the shell
    /// service; the path is written server-side and never returned to the client.
    pub clipboard: ClipboardWriterRef,
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
    pub allowed_roots: Vec<std::path::PathBuf>,
    /// Roots permitted by the shallow `/api/fs/browse` endpoint. This is
    /// typically wider than `allowed_roots` (it includes `cwd`, Windows
    /// drive letters, and `/` on Unix) because the WebUI host-file picker
    /// legitimately needs to reach outside any single workspace.
    pub browse_roots: BrowseRoots,
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the file router with all `/api/fs/*` routes.
///
/// All routes require authentication (applied by the caller).
pub fn file_routes(state: FileRouterState) -> Router {
    // Upload route carries its own body-size limit (UPLOAD_MAX_SIZE, 30 MB).
    // We first disable the global `DefaultBodyLimit` that `aionui-app`
    // installs (otherwise the `Multipart` extractor would cap the body at
    // `BODY_LIMIT`), then apply `RequestBodyLimitLayer` as the sole hard
    // cap. The layers are added in outer->inner order via `.layer()`.
    let upload_router = Router::new()
        .route("/api/fs/upload", post(upload_file))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(UPLOAD_MAX_SIZE))
        .with_state(state.clone());

    Router::new()
        // A. Core file operations
        .route("/api/fs/browse", get(browse_directory))
        .route("/api/fs/dir", post(get_files_by_dir))
        .route("/api/fs/list", post(list_workspace_files))
        .route("/api/fs/metadata", post(get_file_metadata))
        .route("/api/fs/read", post(read_file))
        .route("/api/fs/read-buffer", post(read_file_buffer))
        .route("/api/fs/write", post(write_file))
        .route("/api/fs/copy", post(copy_files))
<<<<<<< HEAD
        .route("/api/fs/remove", post(remove_entry))
        .route("/api/fs/rename", post(rename_entry))
        .route("/api/fs/temp", post(create_temp_file))
=======
        .route("/api/fs/reveal", post(reveal_item))
        .route("/api/fs/open-system", post(open_system_file))
        .route("/api/fs/copy-absolute-path", post(copy_absolute_path))
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
        .route("/api/fs/image-base64", post(get_image_base64))
        .route("/api/fs/fetch-remote-image", post(fetch_remote_image))
        .route("/api/fs/zip", post(create_zip))
        .route("/api/fs/zip/cancel", post(cancel_zip))
        // D. File watch
        .route("/api/fs/watch/start", post(start_watch))
        .route("/api/fs/watch/stop", post(stop_watch))
        .route("/api/fs/watch/stop-all", post(stop_all_watches))
        .route("/api/fs/office-watch/start", post(start_office_watch))
        .route("/api/fs/office-watch/stop", post(stop_office_watch))
        // E. Workspace snapshot
        .route("/api/fs/snapshot/init", post(snapshot_init))
        .route("/api/fs/snapshot/info", post(snapshot_info))
        .route("/api/fs/snapshot/compare", post(snapshot_compare))
        .route("/api/fs/snapshot/baseline", post(snapshot_baseline))
        .route("/api/fs/snapshot/stage", post(snapshot_stage_file))
        .route("/api/fs/snapshot/stage-all", post(snapshot_stage_all))
        .route("/api/fs/snapshot/unstage", post(snapshot_unstage_file))
        .route("/api/fs/snapshot/unstage-all", post(snapshot_unstage_all))
        .route("/api/fs/snapshot/discard", post(snapshot_discard))
        .route("/api/fs/snapshot/reset", post(snapshot_reset))
        .route("/api/fs/snapshot/branches", post(snapshot_branches))
        .route("/api/fs/snapshot/dispose", post(snapshot_dispose))
        .with_state(state)
        .merge(upload_router)
}

// ---------------------------------------------------------------------------
// A. Core file operations — handlers
// ---------------------------------------------------------------------------

/// `GET /api/fs/browse` — shallow directory listing for the WebUI host-file
/// picker. Runs on the Tokio blocking pool because it does synchronous
/// filesystem I/O.
async fn browse_directory(
    State(state): State<FileRouterState>,
    Query(query): Query<BrowseDirectoryQuery>,
) -> Result<Json<ApiResponse<BrowseDirectoryResponse>>, ApiError> {
    let show_files = matches!(query.show_files.as_deref(), Some("true") | Some("1"));
    let raw_path = query.path.clone();
    let browse_roots = state.browse_roots.clone();

    let response = tokio::task::spawn_blocking(move || {
        let roots = browse_roots.get();
        browse::browse(raw_path.as_deref(), show_files, &roots)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("browse task failed: {}", e)))??;

    Ok(Json(ApiResponse::ok(response)))
}

async fn get_files_by_dir(
    State(state): State<FileRouterState>,
    body: Result<Json<GetFilesByDirRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<DirOrFileResponse>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let items = state.file_service.get_files_by_dir(&req.dir, &req.root).await?;
    let response: Vec<DirOrFileResponse> = items.into_iter().map(to_dir_or_file_response).collect();
    Ok(Json(ApiResponse::ok(response)))
}

async fn list_workspace_files(
    State(state): State<FileRouterState>,
    body: Result<Json<ListWorkspaceFilesRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<WorkspaceFlatFileResponse>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let root = req.root.trim();
    if root.is_empty() {
        return Err(ApiError::BadRequest("root is required".to_owned()));
    }
    let items = state
        .file_service
        .list_workspace_files_with_extra_root(root, Some(Path::new(root)))
        .await?;

    let response: Vec<WorkspaceFlatFileResponse> = items.into_iter().map(to_flat_file_response).collect();
    Ok(Json(ApiResponse::ok(response)))
}

async fn get_file_metadata(
    State(state): State<FileRouterState>,
    body: Result<Json<GetFileMetadataRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<FileMetadataResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let meta = state
        .file_service
        .get_file_metadata(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(to_metadata_response(meta))))
}

async fn read_file(
    State(state): State<FileRouterState>,
    body: Result<Json<ReadFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let content = state
        .file_service
        .read_file(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(content)))
}

async fn read_file_buffer(
    State(state): State<FileRouterState>,
    body: Result<Json<ReadFileBufferRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let data = state
        .file_service
        .read_file_buffer(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    // Binary data is base64-encoded for JSON transport.
    let encoded = data.map(|bytes| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    });
    Ok(Json(ApiResponse::ok(encoded)))
}

async fn write_file(
    State(state): State<FileRouterState>,
    body: Result<Json<WriteFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let workspace = req.workspace.unwrap_or_else(|| {
        std::path::Path::new(&req.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let ok = state
        .file_service
        .write_file(&req.path, req.data.as_bytes(), &workspace)
        .await?;
    Ok(Json(ApiResponse::ok(ok)))
}

async fn copy_files(
    State(state): State<FileRouterState>,
    body: Result<Json<CopyFilesRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<CopyFilesResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let result = state
        .file_service
        .copy_files_to_workspace(&req.file_paths, &req.workspace, req.source_root.as_deref())
        .await?;
    Ok(Json(ApiResponse::ok(to_copy_response(result))))
}

async fn remove_entry(
    State(state): State<FileRouterState>,
    body: Result<Json<RemoveEntryRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let workspace = req.workspace.unwrap_or_else(|| {
        std::path::Path::new(&req.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    state.file_service.remove_entry(&req.path, &workspace).await?;
    Ok(Json(ApiResponse::success()))
}

<<<<<<< HEAD
async fn rename_entry(
=======
/// Reveal the resolved absolute path via the revealer port. Split from the
/// handler so the resolve → reveal wiring (and its no-local-path / reveal-failed
/// error mapping) is unit-testable with a mock revealer, independent of the
/// project service (`resolve_reference` is covered in `aionui-project`).
async fn reveal_resolved(
    revealer: &dyn crate::traits::IItemRevealer,
    absolute_path: Option<String>,
) -> Result<(), FileError> {
    let abs = absolute_path.ok_or_else(|| FileError::BadRequest("reveal target is not a local path".to_owned()))?;
    revealer.reveal(&abs).await
}

/// `POST /api/fs/copy-absolute-path` — resolve a pe-addressed file/dir to its
/// absolute device path and write it to the OS clipboard, for the Explorer "copy
/// absolute path" action. Returns void.
///
/// Mirrors `/api/fs/reveal` / `/api/fs/open-system`: the backend resolves the
/// path server-side and performs the OS action (here: a clipboard write) itself,
/// so the absolute path is NEVER returned to the client — the no-abs-to-client
/// posture and INV-OPEN hold unchanged (the error branch carries coded errors
/// only, no path). A non-local reference (a folder root that no longer resolves)
/// yields a path-free BadRequest.
async fn copy_absolute_path(
    State(state): State<FileRouterState>,
    Extension(user): Extension<CurrentUser>,
    body: Result<Json<RevealItemRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let resolved = state
        .project
        .resolve_reference(
            &user.id,
            aionui_project::ReferenceInput {
                pe_id: req.pe_id,
                relative_path: req.relative_path,
                op: aionui_project::FileOp::Read,
            },
        )
        .await
        .map_err(ApiError::from)?;
    copy_absolute_path_resolved(state.clipboard.as_ref(), resolved.absolute_path).await?;
    Ok(Json(ApiResponse::success()))
}

/// Write the resolved absolute path to the clipboard via the clipboard port, or
/// fail with a path-free `BadRequest` when the reference is not a local path.
/// Split from the handler so the resolve → clipboard wiring (no-local-path /
/// clipboard-failure mapping) is unit-testable with a mock writer, independent of
/// the project service (`resolve_reference` is covered in `aionui-project`).
/// Symmetric with [`reveal_resolved`].
async fn copy_absolute_path_resolved(
    clipboard: &dyn crate::traits::IClipboardWriter,
    absolute_path: Option<String>,
) -> Result<(), FileError> {
    let abs = absolute_path.ok_or_else(|| FileError::BadRequest("copy target is not a local path".to_owned()))?;
    clipboard.write_text(&abs).await
}

/// Collapse a `ChatFileRef` resolution failure into a path-free API error.
///
/// Shared by every `ChatFileRef`-addressed endpoint (content read/write, metadata,
/// stream, open-system). The shared `From<ProjectError> for ApiError` mapping is
/// **not** usable for these: it renders the error's `Display` as the public message
/// *and* repeats the path under `details.path`, while several variants
/// (`ChatFileMissing`, `LocalPathNotReadable`, `UploadPathOutsideRoot`) carry the
/// absolute path. Both outlets have to be closed — sealing only the message leaves
/// `details` as a second channel.
///
/// These callers address files by identity, so the absolute path is resolved
/// server-side and the client has never seen it; disclosing it in an error would be
/// telling the client something it had no way to know. Endpoints keyed on
/// client-supplied paths are a different case (echoing back what the caller sent
/// reveals nothing) and keep using the shared mapping.
///
/// Resolution failures collapse to one code deliberately: from the client's side
/// "we could not resolve what you named" is a single outcome, and splitting it
/// further would start signalling *why* — which is where path detail creeps back
/// in. Internal failures stay `INTERNAL_ERROR`, whose public message is already
/// fixed.
fn chat_file_resolve_error(err: aionui_project::ProjectError) -> ApiError {
    let code = err.code();
    tracing::warn!(target: "chat_file", error = %err, code, "could not resolve chat file reference");
    match err {
        aionui_project::ProjectError::Database(_) => ApiError::Internal("failed to resolve target".to_owned()),
        _ => ApiError::coded(
            axum::http::StatusCode::NOT_FOUND,
            "FILE_NOT_FOUND",
            "The requested file no longer exists.",
            None::<serde_json::Value>,
        ),
    }
}

/// `POST /api/fs/open-system` — open a `ChatFileRef`-addressed file with the OS
/// default application ("open in system editor"). Preview surfaces this as the
/// escape hatch for files it declines to render (oversized or unsupported
/// formats), so it accepts all three preview sources rather than only project
/// files the way `/api/fs/reveal` does.
///
/// # INV-OPEN (invariant — do not weaken)
///
/// This endpoint's sole effect is invoking the system opener **on the backend
/// host**. It must never return the resolved absolute path to the client in any
/// form:
///
/// - success → empty body;
/// - failure → a stable error code plus a message that says nothing about the
///   path (no `message`, no `details`, no code carrying a path fragment).
///
/// The client addressed this by identity and has no absolute path of its own; the
/// one resolved here is server-side knowledge. Both failure sources are therefore
/// narrowed on purpose: [`chat_file_resolve_error`] discards the resolver's
/// path-bearing context, and the opener adapter logs its cause instead of
/// returning it ([`FileError::TargetNotFound`] has no payload to fill). The
/// earlier reveal implementation threaded a shell error's path through
/// `NotFound(String)` into the response body; leaving nothing to forward is what
/// stops that recurring.
async fn open_system_file(
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
    State(state): State<FileRouterState>,
    body: Result<Json<RenameRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<RenameResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let workspace = req.workspace.unwrap_or_else(|| {
        std::path::Path::new(&req.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let new_path = state
        .file_service
        .rename_entry_with_extra_root(&req.path, &req.new_name, Some(Path::new(&workspace)))
        .await?;
    Ok(Json(ApiResponse::ok(RenameResponse { new_path })))
}

async fn create_temp_file(
    State(state): State<FileRouterState>,
    body: Result<Json<CreateTempFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let path = state.file_service.create_temp_file(&req.file_name).await?;
    Ok(Json(ApiResponse::ok(path)))
}

/// Fields extracted from a `/api/fs/upload` multipart request.
struct UploadMultipartFields {
    file_data: Vec<u8>,
    file_name: Option<String>,
    dispo_file_name: Option<String>,
    conversation_id: Option<String>,
    workspace_relative_path: Option<String>,
}

/// Strip any directory component from a file name and reject empty results.
/// The returned name is guaranteed not to contain path separators; deeper
/// traversal validation happens in [`IFileService::create_upload_file`].
fn sanitize_upload_filename(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or("");
    let last = last.trim();
    if last.is_empty() { None } else { Some(last.to_owned()) }
}

async fn extract_upload_multipart(mut multipart: Multipart) -> Result<UploadMultipartFields, ApiError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut dispo_file_name: Option<String> = None;
    let mut conversation_id: Option<String> = None;
    let mut workspace_relative_path: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "file" => {
                // Capture the Content-Disposition filename (if any) before
                // consuming the field body — `field.file_name()` is only
                // available on the field metadata, not on the Bytes below.
                dispo_file_name = field.file_name().and_then(sanitize_upload_filename);
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "file_name" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read file_name: {e}")))?;
                if let Some(name) = sanitize_upload_filename(&text) {
                    file_name = Some(name);
                }
            }
            "conversation_id" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read conversation_id: {e}")))?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    conversation_id = Some(trimmed.to_owned());
                }
            }
            "workspace_relative_path" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read workspace_relative_path: {e}")))?;
                workspace_relative_path = Some(text);
            }
            _ => {}
        }
    }

    let file_data = file_data.ok_or_else(|| ApiError::BadRequest("missing 'file' field".to_owned()))?;

    Ok(UploadMultipartFields {
        file_data,
        file_name,
        dispo_file_name,
        conversation_id,
        workspace_relative_path,
    })
}

async fn upload_file(
    State(state): State<FileRouterState>,
    Extension(user): Extension<CurrentUser>,
    multipart: Multipart,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let fields = extract_upload_multipart(multipart).await?;

    let file_name = fields.file_name.or(fields.dispo_file_name).ok_or_else(|| {
        ApiError::BadRequest("missing file name: provide 'file_name' or a multipart filename".to_owned())
    })?;

    let force_workspace = fields.workspace_relative_path.is_some();
    let workspace = match fields.conversation_id.as_deref() {
        Some(conversation_id) => {
            state
                .upload_workspace_resolver
                .resolve_workspace(&user.id, conversation_id, force_workspace)
                .await?
        }
        None if force_workspace => {
            return Err(ApiError::BadRequest(
                "workspace upload requires conversation_id".to_owned(),
            ));
        }
        None => None,
    };

    let path = if let Some(workspace) = workspace {
        let relative_dir = fields.workspace_relative_path.as_deref().unwrap_or("uploads");
        state
            .file_service
            .create_workspace_upload_file(&file_name, &fields.file_data, &workspace, Path::new(relative_dir))
            .await?
    } else {
        state
            .file_service
            .create_upload_file(&file_name, &fields.file_data, fields.conversation_id.as_deref())
            .await?
    };
    Ok(Json(ApiResponse::ok(path)))
}

async fn get_image_base64(
    State(state): State<FileRouterState>,
    body: Result<Json<GetImageBase64Request>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let data_url = state
        .file_service
        .get_image_base64(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(data_url)))
}

async fn fetch_remote_image(
    State(state): State<FileRouterState>,
    body: Result<Json<FetchRemoteImageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let data_url = state.file_service.fetch_remote_image(&req.url).await;
    Ok(Json(ApiResponse::ok(data_url)))
}

async fn create_zip(
    State(state): State<FileRouterState>,
    body: Result<Json<ZipRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let entries: Vec<ZipEntry> = req.files.into_iter().map(to_zip_entry).collect();
    let ok = state
        .file_service
        .create_zip_with_extra_roots(
            &req.path,
            entries,
            req.request_id,
            req.workspace.as_deref().map(Path::new),
            req.source_root.as_deref().map(Path::new),
        )
        .await?;
    Ok(Json(ApiResponse::ok(ok)))
}

async fn cancel_zip(
    State(state): State<FileRouterState>,
    body: Result<Json<CancelZipRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let ok = state.file_service.cancel_zip(&req.request_id).await;
    Ok(Json(ApiResponse::ok(ok)))
}

// ---------------------------------------------------------------------------
// D. File watch — handlers
// ---------------------------------------------------------------------------

async fn start_watch(
    State(state): State<FileRouterState>,
    body: Result<Json<FileWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.watch_service.start_watch(&req.file_path).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_watch(
    State(state): State<FileRouterState>,
    body: Result<Json<FileWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.watch_service.stop_watch(&req.file_path).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_all_watches(State(state): State<FileRouterState>) -> Result<Json<ApiResponse<()>>, ApiError> {
    state.watch_service.stop_all_watches().await?;
    Ok(Json(ApiResponse::success()))
}

async fn start_office_watch(
    State(state): State<FileRouterState>,
    body: Result<Json<WorkspaceOfficeWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let allowed_roots: Vec<&Path> = state.allowed_roots.iter().map(std::path::PathBuf::as_path).collect();
    crate::path_safety::validate_path_with_extra_root(&req.workspace, &allowed_roots, Some(Path::new(&req.workspace)))?;
    state.watch_service.start_office_watch(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_office_watch(
    State(state): State<FileRouterState>,
    body: Result<Json<WorkspaceOfficeWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.watch_service.stop_office_watch(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

// ---------------------------------------------------------------------------
// E. Workspace snapshot — handlers
// ---------------------------------------------------------------------------

async fn snapshot_init(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotInfoResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let info = state.snapshot_service.init(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_snapshot_info_response(info))))
}

async fn snapshot_info(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotInfoResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let info = state.snapshot_service.get_info(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_snapshot_info_response(info))))
}

async fn snapshot_compare(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotCompareResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let result = state.snapshot_service.compare(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_compare_response(result))))
}

async fn snapshot_baseline(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotBaselineRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let content = state
        .snapshot_service
        .get_baseline_content(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::ok(content)))
}

async fn snapshot_stage_file(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotStageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state
        .snapshot_service
        .stage_file(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_stage_all(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.snapshot_service.stage_all(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_unstage_file(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotStageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state
        .snapshot_service
        .unstage_file(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_unstage_all(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.snapshot_service.unstage_all(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_discard(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotDiscardRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state
        .snapshot_service
        .discard_file(&req.workspace, &req.file_path, req.operation)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_reset(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotDiscardRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state
        .snapshot_service
        .reset_file(&req.workspace, &req.file_path, req.operation)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_branches(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let branches = state.snapshot_service.get_branches(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(branches)))
}

async fn snapshot_dispose(
    State(state): State<FileRouterState>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.snapshot_service.dispose(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

// ---------------------------------------------------------------------------
// Domain → DTO conversions
// ---------------------------------------------------------------------------

fn to_dir_or_file_response(d: DirOrFile) -> DirOrFileResponse {
    let children = if d.is_dir {
        Some(d.children.into_iter().map(to_dir_or_file_response).collect())
    } else {
        None
    };
    DirOrFileResponse {
        name: d.name,
        full_path: d.full_path,
        relative_path: d.relative_path,
        is_dir: d.is_dir,
        is_file: !d.is_dir,
        children,
    }
}

fn to_flat_file_response(f: WorkspaceFlatFile) -> WorkspaceFlatFileResponse {
    WorkspaceFlatFileResponse {
        name: f.name,
        full_path: f.full_path,
        relative_path: f.relative_path,
    }
}

fn to_metadata_response(m: FileMetadata) -> FileMetadataResponse {
    FileMetadataResponse {
        name: m.name,
        path: m.path,
        size: m.size,
        mime_type: m.mime_type,
        last_modified: m.last_modified,
        is_directory: if m.is_directory { Some(true) } else { None },
    }
}

fn to_copy_response(r: CopyResult) -> CopyFilesResponse {
    CopyFilesResponse {
        copied_files: r.copied_files,
        failed_files: r.failed_files,
    }
}

fn to_zip_entry(e: aionui_api_types::ZipFileEntry) -> ZipEntry {
    if let Some(content) = e.content {
        ZipEntry::Text { name: e.name, content }
    } else if let Some(file_path) = e.file_path {
        ZipEntry::Disk {
            name: e.name,
            file_path,
        }
    } else {
        // Fallback: treat as empty text entry
        ZipEntry::Text {
            name: e.name,
            content: String::new(),
        }
    }
}

fn to_snapshot_info_response(info: SnapshotInfo) -> SnapshotInfoResponse {
    let mode = match info.mode {
        SnapshotMode::GitRepo => aionui_api_types::SnapshotMode::GitRepo,
        SnapshotMode::Snapshot => aionui_api_types::SnapshotMode::Snapshot,
    };
    SnapshotInfoResponse {
        mode,
        branch: info.branch,
    }
}

fn to_file_change_response(c: FileChangeInfo) -> FileChangeInfoResponse {
    FileChangeInfoResponse {
        file_path: c.file_path,
        relative_path: c.relative_path,
        operation: c.operation,
    }
}

fn to_compare_response(r: CompareResult) -> SnapshotCompareResponse {
    SnapshotCompareResponse {
        staged: r.staged.into_iter().map(to_file_change_response).collect(),
        unstaged: r.unstaged.into_iter().map(to_file_change_response).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn file_path_outside_sandbox_maps_to_explicit_api_code() {
        let api_err = ApiError::from(FileError::PathOutsideSandbox {
            message: "path '/tmp/x' is outside the allowed sandbox".into(),
            field: Some("path"),
            operation: Some("access"),
        });
        assert_eq!(api_err.error_code(), "PATH_OUTSIDE_SANDBOX");
        assert_eq!(api_err.error_details().unwrap()["field"], "path");
        assert_eq!(api_err.error_details().unwrap()["operation"], "access");
    }

    #[test]
    fn browse_roots_are_resolved_lazily() {
        let calls = Arc::new(AtomicUsize::new(0));
        let roots = BrowseRoots::with_resolver({
            let calls = calls.clone();
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                vec![std::env::current_dir().unwrap()]
            }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let first = roots.get();
        let second = roots.get();

        assert!(!first.is_empty());
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

<<<<<<< HEAD
=======
    #[tokio::test]
    async fn system_opener_receives_the_resolved_absolute_path() {
        // The port takes the trusted absolute path resolved from the identity — the
        // handler must not hand it anything client-derived.
        let mock = MockSystemOpener::new(false);
        let result = crate::traits::ISystemFileOpener::open(&mock, "/abs/target.docx").await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(*mock.calls.lock().unwrap(), vec!["/abs/target.docx".to_owned()]);
    }

    /// INV-OPEN end to end at the seam: a failure carrying a path-shaped input must
    /// still render a response with no trace of it.
    #[tokio::test]
    async fn system_open_failure_response_is_path_free() {
        let mock = MockSystemOpener::new(true);
        let err = crate::traits::ISystemFileOpener::open(&mock, "/Users/someone/secret-dir/private.docx")
            .await
            .expect_err("must fail");

        let api_err = ApiError::from(err);
        assert_eq!(api_err.error_code(), "FILE_NOT_FOUND");
        let public = api_err.public_message();
        let details = format!("{:?}", api_err.error_details());
        for haystack in [&public, &details] {
            assert!(
                !haystack.contains("secret-dir") && !haystack.contains("private.docx"),
                "response must not disclose the resolved path, got {haystack:?}"
            );
        }
    }

    // -- reveal_resolved: resolve → reveal wiring (mock revealer seam) ---------

    /// Records the absolute paths handed to `reveal`, and optionally fails.
    struct MockRevealer {
        calls: std::sync::Mutex<Vec<String>>,
        fail: bool,
    }
    impl MockRevealer {
        fn new(fail: bool) -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                fail,
            }
        }
    }
    #[async_trait::async_trait]
    impl crate::traits::IItemRevealer for MockRevealer {
        async fn reveal(&self, absolute_path: &str) -> Result<(), FileError> {
            self.calls.lock().unwrap().push(absolute_path.to_owned());
            if self.fail {
                Err(FileError::RevealFailed("mock reveal failed".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn reveal_resolved_passes_absolute_path_to_revealer() {
        let mock = MockRevealer::new(false);
        let result = reveal_resolved(&mock, Some("/abs/target.txt".to_owned())).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(
            *mock.calls.lock().unwrap(),
            vec!["/abs/target.txt".to_owned()],
            "revealer must receive the resolved absolute path"
        );
    }

    #[tokio::test]
    async fn reveal_resolved_without_local_path_is_bad_request_and_skips_reveal() {
        let mock = MockRevealer::new(false);
        let result = reveal_resolved(&mock, None).await;
        assert!(
            matches!(result, Err(FileError::BadRequest(_))),
            "non-local target must be BadRequest, got {result:?}"
        );
        assert!(mock.calls.lock().unwrap().is_empty(), "revealer must not be called");
    }

    #[tokio::test]
    async fn reveal_resolved_propagates_reveal_failure() {
        let mock = MockRevealer::new(true);
        let result = reveal_resolved(&mock, Some("/abs/x".to_owned())).await;
        assert!(
            matches!(result, Err(FileError::RevealFailed(_))),
            "reveal failure must propagate, got {result:?}"
        );
    }

    // -- copy_absolute_path_resolved: resolve → clipboard wiring (mock seam) ----

    /// Records the text handed to `write_text`, and optionally fails.
    struct MockClipboardWriter {
        calls: std::sync::Mutex<Vec<String>>,
        fail: bool,
    }
    impl MockClipboardWriter {
        fn new(fail: bool) -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                fail,
            }
        }
    }
    #[async_trait::async_trait]
    impl crate::traits::IClipboardWriter for MockClipboardWriter {
        async fn write_text(&self, text: &str) -> Result<(), FileError> {
            self.calls.lock().unwrap().push(text.to_owned());
            if self.fail {
                Err(FileError::Internal("mock clipboard failed".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn copy_absolute_path_resolved_writes_the_absolute_path_to_the_clipboard() {
        // The backend performs the OS action (clipboard write) itself; the abs is
        // never returned — the handler returns void on success.
        let mock = MockClipboardWriter::new(false);
        let result = copy_absolute_path_resolved(&mock, Some("/abs/target.txt".to_owned())).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(
            *mock.calls.lock().unwrap(),
            vec!["/abs/target.txt".to_owned()],
            "clipboard must receive the resolved absolute path"
        );
    }

    #[tokio::test]
    async fn copy_absolute_path_resolved_without_local_path_is_bad_request_and_skips_clipboard() {
        let mock = MockClipboardWriter::new(false);
        let result = copy_absolute_path_resolved(&mock, None).await;
        assert!(
            matches!(result, Err(FileError::BadRequest(_))),
            "non-local target must be BadRequest, got {result:?}"
        );
        assert!(mock.calls.lock().unwrap().is_empty(), "clipboard must not be called");
    }

    #[tokio::test]
    async fn copy_absolute_path_resolved_propagates_clipboard_failure() {
        let mock = MockClipboardWriter::new(true);
        let result = copy_absolute_path_resolved(&mock, Some("/abs/x".to_owned())).await;
        assert!(result.is_err(), "clipboard failure must propagate, got {result:?}");
    }

    /// The resolver seam shared by every `ChatFileRef`-addressed endpoint — the leak
    /// the shared `From<ProjectError>` mapping introduces. Verified against it
    /// directly: that mapping renders `ChatFileMissing`'s `Display` as the public
    /// message ("attached file does not exist: /Users/…/private.docx") *and* repeats
    /// the path under `details.path`. Both outlets are asserted, because sealing only
    /// the message would leave `details` as a second channel.
    #[test]
    fn chat_file_resolve_error_is_path_free_for_path_bearing_variants() {
        let secret = "/Users/someone/secret-dir/private.docx";
        let variants = [
            aionui_project::ProjectError::ChatFileMissing {
                path: secret.to_owned(),
            },
            aionui_project::ProjectError::LocalPathNotReadable {
                path: secret.to_owned(),
            },
            aionui_project::ProjectError::UploadPathOutsideRoot {
                path: secret.to_owned(),
            },
        ];

        for err in variants {
            let api_err = chat_file_resolve_error(err);
            assert_eq!(api_err.error_code(), "FILE_NOT_FOUND");
            assert_eq!(api_err.status_code(), axum::http::StatusCode::NOT_FOUND);

            let public = api_err.public_message();
            let details = format!("{:?}", api_err.error_details());
            for haystack in [&public, &details] {
                assert!(
                    !haystack.contains("secret-dir") && !haystack.contains("private.docx"),
                    "resolve failure must not disclose the path, got {haystack:?}"
                );
            }
        }
    }

    /// Every `ChatFileRef`-addressed handler must route its resolver failure through
    /// [`chat_file_resolve_error`], not the shared `From<ProjectError>` mapping.
    ///
    /// Asserted against the source text because the alternative — spinning up five
    /// authenticated handlers with a real `ProjectService` — would not actually pin
    /// this: the wiring is a single `map_err` per handler, and a future edit swapping
    /// one back to `ApiError::from` is exactly the regression worth catching. The
    /// count guards against a sixth such endpoint being added without a decision:
    /// bump it deliberately, having checked the new one addresses files by identity.
    #[test]
    fn every_chat_file_ref_endpoint_uses_the_sealed_resolver_mapping() {
        // Scan handler code only. This test module mentions both needles in its own
        // assertions, and counting those would inflate the totals.
        let source = include_str!("routes.rs");
        let handlers = source
            .split_once("\n#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("routes.rs has a #[cfg(test)] module");

        let resolve_calls = handlers.matches(".resolve_chat_file_ref(").count();
        let sealed = handlers.matches(".map_err(chat_file_resolve_error)?").count();

        assert_eq!(
            resolve_calls, 5,
            "expected 5 ChatFileRef-addressed endpoints (content read/write, metadata, stream, \
             open-system); found {resolve_calls} — a new one must be checked for identity \
             addressing and sealed before bumping this"
        );
        assert_eq!(
            sealed, resolve_calls,
            "every resolve_chat_file_ref call must map its error through \
             chat_file_resolve_error; {sealed} of {resolve_calls} do. A handler using \
             ApiError::from leaks the resolved absolute path in message and details."
        );
    }

    /// The `Database` arm (reachable through `resolve_reference`) is deliberately
    /// not covered here: constructing a `DbError` would mean adding `aionui-db` as a
    /// dependency of this crate purely for a test. Its correctness rests on
    /// `ApiError::Internal`'s public message being a fixed string, which
    /// `aionui-common` already tests.
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
    #[test]
    fn dir_or_file_response_conversion_file() {
        let d = DirOrFile {
            name: "test.txt".into(),
            full_path: "/ws/test.txt".into(),
            relative_path: "test.txt".into(),
            is_dir: false,
            children: vec![],
        };
        let r = to_dir_or_file_response(d);
        assert_eq!(r.name, "test.txt");
        assert!(!r.is_dir);
        assert!(r.is_file);
        assert!(r.children.is_none());
    }

    #[test]
    fn dir_or_file_response_conversion_dir_with_children() {
        let d = DirOrFile {
            name: "src".into(),
            full_path: "/ws/src".into(),
            relative_path: "src".into(),
            is_dir: true,
            children: vec![DirOrFile {
                name: "main.rs".into(),
                full_path: "/ws/src/main.rs".into(),
                relative_path: "src/main.rs".into(),
                is_dir: false,
                children: vec![],
            }],
        };
        let r = to_dir_or_file_response(d);
        assert!(r.is_dir);
        assert!(!r.is_file);
        let children = r.children.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "main.rs");
    }

    #[test]
    fn flat_file_response_conversion() {
        let f = WorkspaceFlatFile {
            name: "lib.rs".into(),
            full_path: "/ws/src/lib.rs".into(),
            relative_path: "src/lib.rs".into(),
        };
        let r = to_flat_file_response(f);
        assert_eq!(r.name, "lib.rs");
        assert_eq!(r.full_path, "/ws/src/lib.rs");
        assert_eq!(r.relative_path, "src/lib.rs");
    }

    #[test]
    fn metadata_response_conversion_file() {
        let m = FileMetadata {
            name: "readme.md".into(),
            path: "/ws/readme.md".into(),
            size: 1024,
            mime_type: "text/markdown".into(),
            last_modified: 1700000000000,
            is_directory: false,
        };
        let r = to_metadata_response(m);
        assert_eq!(r.name, "readme.md");
        assert_eq!(r.size, 1024);
        assert!(r.is_directory.is_none());
    }

    #[test]
    fn metadata_response_conversion_directory() {
        let m = FileMetadata {
            name: "src".into(),
            path: "/ws/src".into(),
            size: 0,
            mime_type: "".into(),
            last_modified: 1700000000000,
            is_directory: true,
        };
        let r = to_metadata_response(m);
        assert_eq!(r.is_directory, Some(true));
    }

    #[test]
    fn zip_entry_conversion_text() {
        let e = aionui_api_types::ZipFileEntry {
            name: "a.txt".into(),
            content: Some("hello".into()),
            file_path: None,
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Text { name, content } => {
                assert_eq!(name, "a.txt");
                assert_eq!(content, "hello");
            }
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn zip_entry_conversion_disk() {
        let e = aionui_api_types::ZipFileEntry {
            name: "b.bin".into(),
            content: None,
            file_path: Some("/src/b.bin".into()),
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Disk { name, file_path } => {
                assert_eq!(name, "b.bin");
                assert_eq!(file_path, "/src/b.bin");
            }
            _ => panic!("expected Disk variant"),
        }
    }

    #[test]
    fn zip_entry_conversion_empty_fallback() {
        let e = aionui_api_types::ZipFileEntry {
            name: "empty.txt".into(),
            content: None,
            file_path: None,
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Text { name, content } => {
                assert_eq!(name, "empty.txt");
                assert!(content.is_empty());
            }
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn snapshot_info_response_git_repo() {
        let info = SnapshotInfo {
            mode: SnapshotMode::GitRepo,
            branch: Some("main".into()),
        };
        let r = to_snapshot_info_response(info);
        assert_eq!(r.mode, aionui_api_types::SnapshotMode::GitRepo);
        assert_eq!(r.branch, Some("main".into()));
    }

    #[test]
    fn snapshot_info_response_snapshot_mode() {
        let info = SnapshotInfo {
            mode: SnapshotMode::Snapshot,
            branch: None,
        };
        let r = to_snapshot_info_response(info);
        assert_eq!(r.mode, aionui_api_types::SnapshotMode::Snapshot);
        assert!(r.branch.is_none());
    }

    #[test]
    fn compare_response_conversion() {
        use aionui_common::FileChangeOperation;
        let result = CompareResult {
            staged: vec![FileChangeInfo {
                file_path: "/ws/a.txt".into(),
                relative_path: "a.txt".into(),
                operation: FileChangeOperation::Create,
            }],
            unstaged: vec![FileChangeInfo {
                file_path: "/ws/b.txt".into(),
                relative_path: "b.txt".into(),
                operation: FileChangeOperation::Modify,
            }],
        };
        let r = to_compare_response(result);
        assert_eq!(r.staged.len(), 1);
        assert_eq!(r.staged[0].file_path, "/ws/a.txt");
        assert_eq!(r.staged[0].operation, FileChangeOperation::Create);
        assert_eq!(r.unstaged.len(), 1);
        assert_eq!(r.unstaged[0].operation, FileChangeOperation::Modify);
    }

    // ---- sanitize_upload_filename -----------------------------------------

    #[test]
    fn sanitize_upload_filename_strips_directory_components() {
        assert_eq!(sanitize_upload_filename("a/b/c.png").as_deref(), Some("c.png"));
        assert_eq!(sanitize_upload_filename("C:\\tmp\\d.jpg").as_deref(), Some("d.jpg"));
        assert_eq!(
            sanitize_upload_filename("  spaced.txt  ").as_deref(),
            Some("spaced.txt")
        );
    }

    #[test]
    fn sanitize_upload_filename_rejects_empty() {
        assert_eq!(sanitize_upload_filename(""), None);
        assert_eq!(sanitize_upload_filename("   "), None);
        assert_eq!(sanitize_upload_filename("/"), None);
        assert_eq!(sanitize_upload_filename("a/b/"), None);
    }

    #[test]
    fn sanitize_upload_filename_plain_passthrough() {
        assert_eq!(sanitize_upload_filename("image.png").as_deref(), Some("image.png"));
    }
}
