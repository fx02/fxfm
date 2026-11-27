//! fxfm-core: platform-independent logic (no Tauri dependency).

pub mod archive;
pub mod audio;
pub mod branch;
pub mod config;
pub mod hotlist;
pub mod keymap;
pub mod listing;
pub mod multirename;
pub mod ops;
pub mod process;
pub mod remote;
pub mod search;
pub mod secrets;
pub mod servers;
pub mod storage;
pub mod sync;
pub mod undo;
pub mod vfs;
pub mod viewer;
pub mod volumes;

/// Crate version, as set in Cargo.toml.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!version().is_empty());
    }
}
