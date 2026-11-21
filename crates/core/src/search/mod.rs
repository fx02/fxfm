//! Name search (Alt+F7): query parsing, matching, ranking and the in-memory name index.

pub mod archives;
pub mod index;
pub mod query;
pub mod scan;
pub mod store;

pub use archives::{
    index_archive, index_archives_under, needs_reindex, ArchiveOptions, ArchiveOutcome,
    ArchiveStamp,
};
pub use index::{apply_change, Change, EntryId, Index, Meta};
pub use query::{fold, rank, Query, Term};
pub use scan::{
    background, default_excludes, denial, in_background, is_dataless, meta_of,
    needs_full_disk_access, scan_into, walk_search, Denial, ScanOptions, ScanReport, WalkHit,
};
pub use store::{
    load, save, write_unsynced, LoadError, PendingSave, Stored, VolumeState, FORMAT_VERSION,
};

/// Where a search looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every indexed volume.
    Everywhere { archives: bool },
    /// Only entries below the absolute path `path` (not the folder itself).
    Under { path: String, archives: bool },
}
