//! Test builders that write archives of every supported kind.
//!
//! Items are `(path, contents)`; `None` contents make a directory. Names are written
//! verbatim (no validation), so tests can create unsafe names such as `../evil`.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use super::ArchiveKind;

pub type Item<'a> = (&'a str, Option<&'a [u8]>);

pub const TEST_MTIME: u64 = 1_700_000_000;

pub fn build(kind: ArchiveKind, path: &Path, items: &[Item]) {
    match kind {
        ArchiveKind::Zip => make_zip(path, items),
        ArchiveKind::Tar => make_tar(path, items),
        ArchiveKind::TarGz => make_tgz(path, items),
        k @ (ArchiveKind::TarBz2 | ArchiveKind::TarXz) => {
            let sink = super::TarSink::new(k, File::create(path).unwrap()).unwrap();
            tar_into(sink, items).finish().unwrap();
        }
        ArchiveKind::SevenZ => make_7z(path, items),
        ArchiveKind::Gz => {
            let (name, data) = items[0];
            make_gz(path, Some(name), data.unwrap_or_default());
        }
    }
}

pub fn make_zip(path: &Path, items: &[Item]) {
    write_zip(path, items, None);
}

/// Every file entry is AES-256 encrypted with `password`.
pub fn make_zip_aes(path: &Path, items: &[Item], password: &str) {
    write_zip(path, items, Some(password));
}

fn write_zip(path: &Path, items: &[Item], password: Option<&str>) {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(File::create(path).unwrap());
    let opts = SimpleFileOptions::default().unix_permissions(0o644);
    for (name, data) in items {
        match data {
            None => w
                .add_directory(*name, opts.unix_permissions(0o755))
                .unwrap(),
            Some(d) => {
                let o = match password {
                    Some(p) => opts.with_aes_encryption(zip::AesMode::Aes256, p),
                    None => opts,
                };
                w.start_file(*name, o).unwrap();
                w.write_all(d).unwrap();
            }
        }
    }
    w.finish().unwrap();
}

fn tar_into<W: Write>(w: W, items: &[Item]) -> W {
    tar_links_into(w, items, &[])
}

fn tar_links_into<W: Write>(w: W, items: &[Item], links: &[(&str, &str)]) -> W {
    let mut b = tar::Builder::new(w);
    for (name, data) in items {
        let mut h = tar::Header::new_gnu();
        assert!(name.len() < 100, "test names must fit the tar name field");
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_mtime(TEST_MTIME);
        match data {
            None => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
            }
            Some(d) => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(0o644);
                h.set_size(d.len() as u64);
            }
        }
        h.set_cksum();
        b.append(&h, data.unwrap_or_default()).unwrap();
    }
    for (name, target) in links {
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_link_name(target).unwrap();
        h.set_mtime(TEST_MTIME);
        h.set_mode(0o777);
        h.set_size(0);
        h.set_cksum();
        b.append(&h, std::io::empty()).unwrap();
    }
    b.into_inner().unwrap()
}

pub fn make_tar(path: &Path, items: &[Item]) {
    tar_into(File::create(path).unwrap(), items);
}

/// A tar with the regular `items` followed by symlink entries `(name, target)`.
pub fn make_tar_links(path: &Path, items: &[Item], links: &[(&str, &str)]) {
    tar_links_into(File::create(path).unwrap(), items, links);
}

pub fn make_tgz(path: &Path, items: &[Item]) {
    let gz =
        flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
    tar_into(gz, items).finish().unwrap();
}

/// A gzip file; `name` goes to the header (none if `None`).
pub fn make_gz(path: &Path, name: Option<&str>, data: &[u8]) {
    let mut b = flate2::GzBuilder::new().mtime(TEST_MTIME as u32);
    if let Some(n) = name {
        b = b.filename(n);
    }
    let mut w = b.write(File::create(path).unwrap(), flate2::Compression::fast());
    w.write_all(data).unwrap();
    w.finish().unwrap();
}

pub fn make_7z(path: &Path, items: &[Item]) {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = ArchiveWriter::create(path).unwrap();
    for (name, data) in items {
        match data {
            None => {
                w.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory(name), None)
                    .unwrap();
            }
            Some(d) => {
                w.push_archive_entry(ArchiveEntry::new_file(name), Some(*d))
                    .unwrap();
            }
        }
    }
    w.finish().unwrap();
}

/// Archive `dir/<outer>` holding `docs/x.txt` and the archive `docs/<inner>` made of `items`
/// (kinds by name). The nested archive's location is `<dir>/<outer>/docs/<inner>`.
pub fn make_nested(dir: &Path, outer: &str, inner: &str, items: &[Item]) {
    let src = tempfile::tempdir().unwrap();
    let ip = src.path().join(inner);
    build(ArchiveKind::from_name(inner).unwrap(), &ip, items);
    let data = std::fs::read(&ip).unwrap();
    let name = format!("docs/{inner}");
    build(
        ArchiveKind::from_name(outer).unwrap(),
        &dir.join(outer),
        &[("docs/x.txt", Some(b"x")), (&name, Some(&data))],
    );
}
