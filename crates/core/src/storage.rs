//! Data location (portable beside the app, or system) and atomic JSON storage.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const DATA_DIR_NAME: &str = "fxfm-data";
const WRITE_TEST: &str = ".fxfm-write-test";
/// The search index folder: a cache of up to ~190 MB that the app saves into the new data
/// folder itself after a move, so `migrate` neither copies it nor keeps it in a set-aside copy.
const SEARCH_INDEX_DIR: &str = "index";

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("target folder is not empty: {0}")]
    TargetNotEmpty(PathBuf),
    #[error("no app folder available for portable mode")]
    NoAppFolder,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StorageMode {
    Portable,
    System,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DataLocation {
    pub mode: StorageMode,
    pub dir: PathBuf,
}

/// The folder the app is in, as a user sees it: the folder holding `fxfm.app`
/// for an executable inside a macOS bundle, else the executable's folder.
pub fn app_folder(exe: &Path) -> Option<&Path> {
    let dir = exe.parent()?;
    let bundle = dir
        .parent()
        .filter(|contents| dir.ends_with("Contents/MacOS") && contents.ends_with("Contents"))
        .and_then(Path::parent)
        .filter(|app| app.extension().is_some_and(|e| e == "app"));
    match bundle {
        Some(app) => app.parent(),
        None => Some(dir),
    }
}

/// `~/Library/Application Support/fxfm` on macOS, else `$XDG_DATA_HOME/fxfm`
/// or `~/.local/share/fxfm`.
pub fn system_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".").join(".fxfm"))
        .join("fxfm")
}

/// `dir` exists and a file can be created in it.
pub fn is_writable_dir(dir: &Path) -> bool {
    let probe = dir.join(WRITE_TEST);
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Portable if `app_folder/fxfm-data` is an existing, writable folder; otherwise
/// the (created) system folder.
pub fn resolve(app_folder: Option<&Path>, system_dir: &Path) -> DataLocation {
    if let Some(dir) = app_folder
        .map(|a| a.join(DATA_DIR_NAME))
        .filter(|d| d.is_dir() && is_writable_dir(d))
    {
        return DataLocation {
            mode: StorageMode::Portable,
            dir,
        };
    }
    if let Err(e) = std::fs::create_dir_all(system_dir) {
        log::warn!("cannot create {}: {e}", system_dir.display());
    }
    DataLocation {
        mode: StorageMode::System,
        dir: system_dir.to_path_buf(),
    }
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    copy_dir_skipping(from, to, &[])
}

/// [`copy_dir`] without the top-level entries named in `skip`.
fn copy_dir_skipping(from: &Path, to: &Path, skip: &[&str]) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if skip.iter().any(|s| entry.file_name() == *s) {
            continue;
        }
        let dest = to.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            copy_symlink(&entry.path(), &dest)?;
        } else if ty.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    // Last, so a read-only source dir can still be filled.
    std::fs::set_permissions(to, std::fs::metadata(from)?.permissions())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::copy(from, to).map(|_| ())
}

/// `base` + `suffix`, then `suffix-2`, `suffix-3`, ... until unused.
fn free_name(base: &Path, suffix: &str) -> PathBuf {
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = base.parent().unwrap_or(Path::new("."));
    let mut p = parent.join(format!("{name}{suffix}"));
    let mut n = 2;
    while p.symlink_metadata().is_ok() {
        p = parent.join(format!("{name}{suffix}-{n}"));
        n += 1;
    }
    p
}

fn is_nonempty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

/// Copies all data to the other location. The copy is staged in a sibling
/// folder and renamed into place only when complete, so a failure leaves the
/// target absent and the source untouched. Portable -> System sets the old
/// `fxfm-data` aside (`.old`, `.old-2`, ...) before publishing the copy (so
/// portable does not win again) and rolls that back if publishing fails.
pub fn migrate(
    from: &DataLocation,
    to: StorageMode,
    app_folder: Option<&Path>,
    system_dir: &Path,
) -> Result<DataLocation, StorageError> {
    let target = match to {
        StorageMode::Portable => app_folder
            .ok_or(StorageError::NoAppFolder)?
            .join(DATA_DIR_NAME),
        StorageMode::System => system_dir.to_path_buf(),
    };
    // A non-empty portable target would already have won `resolve()`, so it is
    // someone else's data: refuse. A non-empty system folder is a stale copy
    // from an earlier System run; it is set aside (`fxfm.old`, ...) below.
    let stale_target = is_nonempty_dir(&target);
    if stale_target && to == StorageMode::Portable {
        return Err(StorageError::TargetNotEmpty(target));
    }
    let staging = {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        target.with_file_name(format!("{name}.tmp-migrate"))
    };
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Leftover of an earlier crashed attempt; ours to remove.
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(e) = copy_dir_skipping(&from.dir, &staging, &[SEARCH_INDEX_DIR]) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.into());
    }
    let set_aside = if from.mode == StorageMode::Portable && to == StorageMode::System {
        let old = free_name(&from.dir, ".old");
        if let Err(e) = std::fs::rename(&from.dir, &old) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }
        Some(old)
    } else {
        None
    };
    let restore_source = |set_aside: &Option<PathBuf>| {
        if let Some(old) = set_aside {
            let _ = std::fs::rename(old, &from.dir);
        }
    };
    let stale_aside = if stale_target {
        let old = free_name(&target, ".old");
        if let Err(e) = std::fs::rename(&target, &old) {
            restore_source(&set_aside);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }
        Some(old)
    } else {
        // An empty existing target dir is fine to replace.
        let _ = std::fs::remove_dir(&target);
        None
    };
    if let Err(e) = std::fs::rename(&staging, &target) {
        if let Some(old) = stale_aside {
            let _ = std::fs::rename(&old, &target);
        }
        restore_source(&set_aside);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.into());
    }
    if let Some(old) = &set_aside {
        // Nothing would ever delete it there (the app re-saves the index in the new folder).
        let _ = std::fs::remove_dir_all(old.join(SEARCH_INDEX_DIR));
    }
    Ok(DataLocation {
        mode: to,
        dir: target,
    })
}

/// Missing file gives `Default`. A corrupt or unreadable one is renamed to
/// `name.broken` (never overwriting an earlier one) and also gives `Default`.
pub fn load_json<T: DeserializeOwned + Default>(dir: &Path, name: &str) -> T {
    let path = dir.join(name);
    let problem = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => return v,
            Err(e) => e.to_string(),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => return T::default(),
        Err(e) => e.to_string(),
    };
    let broken = free_name(&path, ".broken");
    log::warn!(
        "{} is unusable ({problem}); moving to {}",
        path.display(),
        broken.display()
    );
    if let Err(e) = std::fs::rename(&path, &broken) {
        log::warn!("cannot set aside {}: {e}", path.display());
    }
    T::default()
}

/// Pretty JSON (2 spaces), written to `name.tmp` and renamed into place.
pub fn save_json<T: Serialize>(dir: &Path, name: &str, v: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(v).map_err(io::Error::other)?;
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::fs;

    #[test]
    fn app_folder_beside_exe_or_bundle() {
        assert_eq!(
            app_folder(Path::new("/Volumes/USB/fxfm.app/Contents/MacOS/fxfm")),
            Some(Path::new("/Volumes/USB"))
        );
        assert_eq!(
            app_folder(Path::new("/Volumes/USB/fxfm/fxfm")),
            Some(Path::new("/Volumes/USB/fxfm"))
        );
        assert_eq!(
            app_folder(Path::new("/x/Contents/MacOS/fxfm")),
            Some(Path::new("/x/Contents/MacOS"))
        );
    }

    #[test]
    fn resolve_portable_only_with_writable_dir() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let sys = t.path().join("sys");
        fs::create_dir(&app).unwrap();
        assert_eq!(resolve(Some(&app), &sys).mode, StorageMode::System);
        fs::write(app.join(DATA_DIR_NAME), "").unwrap();
        assert_eq!(resolve(Some(&app), &sys).mode, StorageMode::System);
        fs::remove_file(app.join(DATA_DIR_NAME)).unwrap();
        fs::create_dir(app.join(DATA_DIR_NAME)).unwrap();
        let r = resolve(Some(&app), &sys);
        assert_eq!(r.mode, StorageMode::Portable);
        assert_eq!(r.dir, app.join(DATA_DIR_NAME));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let d = app.join(DATA_DIR_NAME);
            fs::set_permissions(&d, fs::Permissions::from_mode(0o555)).unwrap();
            let is_root = fs::write(d.join("x"), "").is_ok();
            let r = resolve(Some(&app), &sys);
            fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
            if !is_root {
                assert_eq!(r.mode, StorageMode::System);
                assert!(sys.is_dir());
            }
        }
    }

    fn sys_with_data(t: &Path) -> DataLocation {
        let sys = t.join("sys");
        fs::create_dir_all(sys.join("keys")).unwrap();
        fs::write(sys.join("settings.json"), "{}").unwrap();
        fs::write(sys.join("keys/a.key"), "k").unwrap();
        DataLocation {
            mode: StorageMode::System,
            dir: sys,
        }
    }

    #[test]
    fn migrate_system_to_portable_copies_everything() {
        let t = tempfile::tempdir().unwrap();
        let from = sys_with_data(t.path());
        let app = t.path().join("app");
        fs::create_dir(&app).unwrap();
        let to = migrate(&from, StorageMode::Portable, Some(&app), &from.dir).unwrap();
        assert_eq!(to.mode, StorageMode::Portable);
        assert_eq!(to.dir, app.join(DATA_DIR_NAME));
        assert!(to.dir.join("settings.json").is_file());
        assert!(to.dir.join("keys/a.key").is_file());
        assert!(from.dir.join("settings.json").is_file());
    }

    #[test]
    fn migrate_portable_to_system_renames_old() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let sys = t.path().join("sys");
        let pdir = app.join(DATA_DIR_NAME);
        fs::create_dir_all(pdir.join("keys")).unwrap();
        fs::write(pdir.join("settings.json"), "{}").unwrap();
        fs::write(pdir.join("keys/a.key"), "k").unwrap();
        let from = resolve(Some(&app), &sys);
        assert_eq!(from.mode, StorageMode::Portable);
        let to = migrate(&from, StorageMode::System, Some(&app), &sys).unwrap();
        assert_eq!(to.mode, StorageMode::System);
        assert!(to.dir.join("keys/a.key").is_file());
        assert!(!pdir.exists());
        assert!(app.join("fxfm-data.old").exists());
        assert_eq!(resolve(Some(&app), &sys).mode, StorageMode::System);
        // second round: .old taken, uses .old-2
        fs::create_dir_all(&pdir).unwrap();
        fs::write(pdir.join("settings.json"), "{}").unwrap();
        let from = DataLocation {
            mode: StorageMode::Portable,
            dir: pdir.clone(),
        };
        let sys2 = t.path().join("sys2");
        migrate(&from, StorageMode::System, Some(&app), &sys2).unwrap();
        assert!(app.join("fxfm-data.old-2").exists());
    }

    #[test]
    fn migrate_leaves_the_search_index_behind() {
        let t = tempfile::tempdir().unwrap();
        // System -> Portable: not copied (the app saves it into the new folder itself).
        let from = sys_with_data(t.path());
        fs::create_dir_all(from.dir.join("index")).unwrap();
        fs::write(from.dir.join("index/index.bin"), "big").unwrap();
        fs::write(from.dir.join("index/index.bin.tmp"), "half").unwrap();
        let app = t.path().join("app");
        fs::create_dir(&app).unwrap();
        let to = migrate(&from, StorageMode::Portable, Some(&app), &from.dir).unwrap();
        assert!(to.dir.join("settings.json").is_file());
        assert!(!to.dir.join("index").exists());
        // Portable -> System: the set-aside portable folder does not keep it either.
        fs::create_dir_all(to.dir.join("index")).unwrap();
        fs::write(to.dir.join("index/index.bin"), "big").unwrap();
        let sys = t.path().join("sys2");
        let back = migrate(&to, StorageMode::System, Some(&app), &sys).unwrap();
        assert!(!back.dir.join("index").exists());
        assert!(app.join("fxfm-data.old/settings.json").is_file());
        assert!(!app.join("fxfm-data.old/index").exists());
    }

    #[test]
    fn migrate_round_trip_system_portable_system() {
        let t = tempfile::tempdir().unwrap();
        let sys_loc = sys_with_data(t.path());
        let sys = sys_loc.dir.clone();
        let app = t.path().join("app");
        fs::create_dir(&app).unwrap();
        let port = migrate(&sys_loc, StorageMode::Portable, Some(&app), &sys).unwrap();
        fs::write(port.dir.join("settings.json"), "{\"new\":1}").unwrap();
        // the system folder still holds the old copy
        assert!(sys.join("settings.json").is_file());
        let back = migrate(&port, StorageMode::System, Some(&app), &sys).unwrap();
        assert_eq!(back.dir, sys);
        assert_eq!(
            fs::read_to_string(sys.join("settings.json")).unwrap(),
            "{\"new\":1}"
        );
        // stale system copy and the portable folder are both kept aside
        assert_eq!(
            fs::read_to_string(t.path().join("sys.old/settings.json")).unwrap(),
            "{}"
        );
        assert!(app.join("fxfm-data.old").exists());
        assert_eq!(resolve(Some(&app), &sys).mode, StorageMode::System);
    }

    #[test]
    fn migrate_refuses_nonempty_target() {
        let t = tempfile::tempdir().unwrap();
        let from = sys_with_data(t.path());
        let app = t.path().join("app");
        fs::create_dir_all(app.join(DATA_DIR_NAME)).unwrap();
        fs::write(app.join(DATA_DIR_NAME).join("keep"), "1").unwrap();
        let r = migrate(&from, StorageMode::Portable, Some(&app), &from.dir);
        assert!(matches!(r, Err(StorageError::TargetNotEmpty(_))));
        assert_eq!(
            fs::read_to_string(app.join(DATA_DIR_NAME).join("keep")).unwrap(),
            "1"
        );
    }

    #[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
    struct S {
        a: u32,
        b: String,
    }

    #[test]
    fn save_then_load_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let v = S {
            a: 3,
            b: "x".into(),
        };
        save_json(t.path(), "settings.json", &v).unwrap();
        assert!(!t.path().join("settings.json.tmp").exists());
        let text = fs::read_to_string(t.path().join("settings.json")).unwrap();
        assert!(text.contains("\n  \"a\": 3"));
        assert_eq!(load_json::<S>(t.path(), "settings.json"), v);
        assert_eq!(load_json::<S>(t.path(), "missing.json"), S::default());
    }

    #[test]
    fn broken_json_is_set_aside() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("settings.json"), "{not json").unwrap();
        assert_eq!(load_json::<S>(t.path(), "settings.json"), S::default());
        assert!(t.path().join("settings.json.broken").exists());
        assert!(!t.path().join("settings.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn migrate_midcopy_failure_leaves_nothing_and_retry_works() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let from = sys_with_data(t.path());
        let bad = from.dir.join("keys/a.key");
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&bad).is_ok() {
            return; // root
        }
        let app = t.path().join("app");
        fs::create_dir(&app).unwrap();
        let r = migrate(&from, StorageMode::Portable, Some(&app), &from.dir);
        assert!(matches!(r, Err(StorageError::Io(_))));
        assert!(!app.join(DATA_DIR_NAME).exists());
        assert!(!app.join("fxfm-data.tmp-migrate").exists());
        assert_eq!(resolve(Some(&app), &from.dir).mode, StorageMode::System);
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o600)).unwrap();
        let to = migrate(&from, StorageMode::Portable, Some(&app), &from.dir).unwrap();
        assert!(to.dir.join("keys/a.key").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn migrate_publish_failure_rolls_back_set_aside() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let pdir = app.join(DATA_DIR_NAME);
        fs::create_dir_all(&pdir).unwrap();
        fs::write(pdir.join("settings.json"), "{}").unwrap();
        // A plain file as system target: publishing the staged dir fails.
        let sys = t.path().join("sysfile");
        fs::write(&sys, "x").unwrap();
        let from = DataLocation {
            mode: StorageMode::Portable,
            dir: pdir.clone(),
        };
        assert!(migrate(&from, StorageMode::System, Some(&app), &sys).is_err());
        assert!(pdir.join("settings.json").is_file());
        assert!(!app.join("fxfm-data.old").exists());
        assert!(!sys.with_file_name("sysfile.tmp-migrate").exists());
        assert_eq!(
            resolve(Some(&app), &t.path().join("s")).mode,
            StorageMode::Portable
        );
        // Retry once the cause is gone.
        fs::remove_file(&sys).unwrap();
        migrate(&from, StorageMode::System, Some(&app), &sys).unwrap();
        assert!(sys.join("settings.json").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn migrate_preserves_dir_perms_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let from = sys_with_data(t.path());
        fs::set_permissions(from.dir.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("settings.json", from.dir.join("link")).unwrap();
        std::os::unix::fs::symlink("nowhere", from.dir.join("dangling")).unwrap();
        let app = t.path().join("app");
        fs::create_dir(&app).unwrap();
        let to = migrate(&from, StorageMode::Portable, Some(&app), &from.dir).unwrap();
        let mode = fs::metadata(to.dir.join("keys"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(
            fs::read_link(to.dir.join("link")).unwrap(),
            Path::new("settings.json")
        );
        assert_eq!(
            fs::read_link(to.dir.join("dangling")).unwrap(),
            Path::new("nowhere")
        );
    }

    #[test]
    fn unreadable_json_is_set_aside_without_overwriting_earlier_broken() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("settings.json.broken"), "old").unwrap();
        fs::write(t.path().join("settings.json"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(load_json::<S>(t.path(), "settings.json"), S::default());
        assert!(!t.path().join("settings.json").exists());
        assert_eq!(
            fs::read_to_string(t.path().join("settings.json.broken")).unwrap(),
            "old"
        );
        assert!(t.path().join("settings.json.broken-2").exists());
    }
}
