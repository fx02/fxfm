//! Zip entry times, as Info-ZIP, Finder, 7-Zip and Keka read them.
//!
//! The DOS date/time field holds LOCAL time (2-second steps, no zone). Writers add the
//! Info-ZIP extended timestamp (extra field 0x5455) with the UTC modification time; readers
//! prefer it and fall back to the DOS field taken as local time.

use std::io::Read;

/// Info-ZIP extended timestamp.
pub(crate) const EXT_TIMESTAMP: u16 = 0x5455;

/// Unix seconds to a DOS date/time in local time; the zip default (1980) when out of range.
pub(crate) fn dos_local(secs: i64) -> zip::DateTime {
    let Some(tm) = local_tm(secs) else {
        return zip::DateTime::default();
    };
    let Ok(year) = u16::try_from(i64::from(tm.tm_year) + 1900) else {
        return zip::DateTime::default();
    };
    zip::DateTime::from_date_and_time(
        year,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
        tm.tm_hour as u8,
        tm.tm_min as u8,
        tm.tm_sec as u8,
    )
    .unwrap_or_default()
}

fn local_tm(secs: i64) -> Option<libc::tm> {
    let t = libc::time_t::try_from(secs).ok()?;
    // SAFETY: an all-zero `tm` is valid; localtime_r fills it and reads only `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call.
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    (!r.is_null()).then_some(tm)
}

/// A DOS date/time taken as local time, to Unix seconds.
pub(crate) fn dos_to_unix_local(t: zip::DateTime) -> i64 {
    // SAFETY: an all-zero `tm` is valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = i32::from(t.year()) - 1900;
    tm.tm_mon = i32::from(t.month()) - 1;
    tm.tm_mday = i32::from(t.day());
    tm.tm_hour = i32::from(t.hour());
    tm.tm_min = i32::from(t.minute());
    tm.tm_sec = i32::from(t.second());
    // Let the C library decide whether daylight saving time applies.
    tm.tm_isdst = -1;
    // SAFETY: `tm` is a valid, initialized struct.
    let secs = unsafe { libc::mktime(&mut tm) };
    if secs == -1 {
        0
    } else {
        // `time_t` is narrower than i64 on some platforms.
        #[allow(clippy::useless_conversion)]
        i64::from(secs)
    }
}

/// The local-header payload of an extended timestamp with only the modification time, or
/// None when `secs` does not fit its unsigned 32-bit field.
pub(crate) fn ext_timestamp(secs: i64) -> Option<[u8; 5]> {
    let t = u32::try_from(secs).ok()?.to_le_bytes();
    // Flags: bit 0 = modification time present.
    Some([1, t[0], t[1], t[2], t[3]])
}

/// Options for a new entry: Deflate, `mode`'s permission bits, `mtime` as local DOS time and
/// as an extended timestamp.
pub(crate) fn entry_options(
    mode: u32,
    mtime: i64,
) -> zip::write::FullFileOptions<'static, 'static> {
    let mut opts = zip::write::FullFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(mode & 0o7777)
        .last_modified_time(dos_local(mtime));
    if let Some(ts) = ext_timestamp(mtime) {
        // Five bytes never exceed the extra field limit, so this cannot fail.
        let _ = opts.add_extra_field(EXT_TIMESTAMP, ts, false);
    }
    opts
}

/// An entry's modification time: the extended timestamp when present, else the DOS field as
/// local time, else 0.
pub(crate) fn entry_mtime<R: Read>(e: &zip::read::ZipFile<'_, R>) -> i64 {
    let ext = e.extra_data_fields().find_map(|f| match f {
        zip::ExtraField::ExtendedTimestamp(ts) => ts.mod_time(),
        _ => None,
    });
    match ext {
        Some(t) => i64::from(t),
        None => e.last_modified().map_or(0, dos_to_unix_local),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_local_roundtrips_through_mktime() {
        for secs in [1_700_000_000_i64, 1_720_000_000, 946_684_800] {
            // DOS time has 2-second steps.
            let even = secs - secs.rem_euclid(2);
            assert_eq!(dos_to_unix_local(dos_local(even)), even, "{secs}");
        }
    }

    #[test]
    fn extended_timestamp_payload() {
        assert_eq!(
            ext_timestamp(0x0102_0304),
            Some([1, 0x04, 0x03, 0x02, 0x01])
        );
        assert_eq!(ext_timestamp(-1), None);
        assert_eq!(ext_timestamp(1 << 33), None);
    }

    /// Archives from tools without the extended timestamp: the DOS field is local time.
    #[test]
    fn reader_takes_a_plain_dos_time_as_local() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("plain.zip");
        let secs = 1_700_000_000;
        let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        let opts = zip::write::SimpleFileOptions::default().last_modified_time(dos_local(secs));
        w.start_file("x.txt", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        let idx = super::super::read::read_index(&p, super::super::ArchiveKind::Zip).unwrap();
        let e = idx.entries.iter().find(|e| e.path == "x.txt").unwrap();
        assert_eq!(e.mtime, secs);
        // And the DOS field really is the local wall clock, not UTC.
        let tm = local_tm(secs).unwrap();
        let d = dos_local(secs);
        assert_eq!(
            (
                i32::from(d.hour()),
                i32::from(d.minute()),
                i32::from(d.day())
            ),
            (tm.tm_hour, tm.tm_min, tm.tm_mday)
        );
    }
}
