#![warn(clippy::disallowed_types)]

//! File system operations: read/write, path safety, file watching, snapshots, and zip.
pub mod browse;
pub mod error;
pub mod path_safety;
pub mod routes;
pub mod service;
pub mod snapshot_service;
pub mod traits;
pub mod types;
pub mod watch_service;

pub use error::FileError;
pub use path_safety::{has_traversal, validate_path, validate_path_for_write};
pub use routes::{BrowseRoots, FileRouterState, file_routes};
pub use service::FileService;
pub use snapshot_service::SnapshotService;
pub use traits::{
<<<<<<< HEAD
    FileServiceRef, FileWatchServiceRef, IFileService, IFileWatchService, ISnapshotService, IUploadWorkspaceResolver,
    SnapshotServiceRef, UploadWorkspaceResolverRef,
=======
    ClipboardWriterRef, FileServiceRef, IClipboardWriter, IFileService, IItemRevealer, ISnapshotService,
    ISystemFileOpener, ItemRevealerRef, SnapshotServiceRef, SystemFileOpenerRef,
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
};
pub use types::{
    CompareResult, ContentUpdateEvent, ContentUpdateOperation, CopyResult, DirOrFile, FileChangeInfo, FileMetadata,
    FileWatchEvent, OfficeFileAddedEvent, SnapshotInfo, SnapshotMode, WorkspaceFlatFile, ZipEntry,
};
pub use watch_service::FileWatchService;
