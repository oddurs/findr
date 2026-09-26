//! Directory listing, sorting, and the small formatting helpers the UI shows.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: u32,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> io::Result<Entry> {
        let own = fs::symlink_metadata(&path)?;
        let is_symlink = own.file_type().is_symlink();
        // Follow links so a link to a directory can be entered; a dangling link keeps its own metadata.
        let meta = if is_symlink {
            fs::metadata(&path).unwrap_or(own)
        } else {
            own
        };
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        Ok(Entry {
            name,
            is_dir: meta.is_dir(),
            is_symlink,
            size: meta.len(),
            modified: meta.modified().ok(),
            mode: meta.permissions().mode(),
            path,
        })
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    pub fn is_executable(&self) -> bool {
        !self.is_dir && self.mode & 0o111 != 0
    }

    pub fn extension(&self) -> &str {
        split_ext(&self.name).1.trim_start_matches('.')
    }
}

/// Splits `name` into stem and extension, the extension keeping its dot.
/// Dotfiles such as `.bashrc` have no extension.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

pub fn list(dir: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir)? {
        let item = item?;
        if !show_hidden && item.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        match Entry::from_path(item.path()) {
            Ok(entry) => out.push(entry),
            // Removed between the listing and the stat: there is nothing left to show.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Modified,
    Size,
    Extension,
}

impl SortKey {
    pub fn next(self) -> SortKey {
        match self {
            SortKey::Name => SortKey::Modified,
            SortKey::Modified => SortKey::Size,
            SortKey::Size => SortKey::Extension,
            SortKey::Extension => SortKey::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Modified => "modified",
            SortKey::Size => "size",
            SortKey::Extension => "type",
        }
    }
}

/// Directories first, then by `key`. Modified and size put the newest and largest first,
/// since that is what one is usually looking for; `reverse` flips it.
pub fn sort(entries: &mut [Entry], key: SortKey, reverse: bool) {
    entries.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => Ordering::Equal,
            SortKey::Modified => b.modified.cmp(&a.modified),
            SortKey::Size if a.is_dir => Ordering::Equal,
            SortKey::Size => b.size.cmp(&a.size),
            SortKey::Extension => natural_cmp(a.extension(), b.extension()),
        }
        .then_with(|| natural_cmp(&a.name, &b.name));
        let ord = if reverse { ord.reverse() } else { ord };
        b.is_dir.cmp(&a.is_dir).then(ord)
    });
}

/// Case-insensitive comparison that orders runs of digits by value, so `file2` sorts before `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    // Works on slices without allocating: sorting a large directory makes ~n log n calls.
    let (mut x, mut y) = (a, b);
    loop {
        let (Some(c), Some(d)) = (x.chars().next(), y.chars().next()) else {
            return x
                .is_empty()
                .cmp(&y.is_empty())
                .reverse()
                .then_with(|| a.cmp(b));
        };
        let ord = if c.is_ascii_digit() && d.is_ascii_digit() {
            let (m, rest_x) = split_number(x);
            let (n, rest_y) = split_number(y);
            (x, y) = (rest_x, rest_y);
            let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
            m.len().cmp(&n.len()).then_with(|| m.cmp(n))
        } else {
            (x, y) = (&x[c.len_utf8()..], &y[d.len_utf8()..]);
            if c.is_ascii() && d.is_ascii() {
                c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase())
            } else {
                c.to_lowercase().cmp(d.to_lowercase())
            }
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

fn split_number(s: &str) -> (&str, &str) {
    s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()))
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while (value >= 1024.0 || value.round() >= 1024.0) && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Relative age for the last week, a date after that.
pub fn fmt_age(t: SystemTime, now: SystemTime) -> String {
    if let Ok(age) = now.duration_since(t) {
        let s = age.as_secs();
        match s {
            0..60 => return "just now".into(),
            60..3600 => return format!("{}m ago", s / 60),
            3600..86400 => return format!("{}h ago", s / 3600),
            86400..604800 => return format!("{}d ago", s / 86400),
            _ => {}
        }
    }
    let (y, m, d, ..) = utc_parts(t);
    format!("{y}-{m:02}-{d:02}")
}

pub fn mode_string(entry: &Entry) -> String {
    let mut s = String::with_capacity(10);
    s.push(match (entry.is_symlink, entry.is_dir) {
        (true, _) => 'l',
        (false, true) => 'd',
        (false, false) => '-',
    });
    for shift in [6, 3, 0] {
        let bits = (entry.mode >> shift) & 0o7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// (year, month, day, hour, minute, second) in UTC.
pub fn utc_parts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

// Howard Hinnant's days-to-civil algorithm; avoids a date crate for one conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
pub mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs, process};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            // A private parent keeps the parent column small; the system temp dir can hold thousands of entries.
            let path = env::temp_dir()
                .join("findr-tests")
                .join(format!("{}-{n}", process::id()));
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(&path).unwrap();
            TempDir(fs::canonicalize(path).unwrap())
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        pub fn file(&self, rel: &str, contents: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Drop cannot report failure; a leftover temp dir is harmless.
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempDir;
    use super::*;
    use std::time::Duration;

    #[test]
    fn natural_order_compares_numbers_by_value() {
        let mut names = vec!["file10", "File2", "file1", "file02", "alpha"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["alpha", "file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn split_ext_leaves_dotfiles_whole() {
        assert_eq!(split_ext("main.rs"), ("main", ".rs"));
        assert_eq!(split_ext("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext("Makefile"), ("Makefile", ""));
    }

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 K");
        assert_eq!(human_size(1536), "1.5 K");
        assert_eq!(human_size(20 * 1024 * 1024), "20 M");
        assert_eq!(human_size(1024 * 1024 - 1), "1.0 M");
    }

    #[test]
    fn age_is_relative_then_a_date() {
        let now = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let ago = |s| fmt_age(now - Duration::from_secs(s), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(300), "5m ago");
        assert_eq!(ago(7200), "2h ago");
        assert_eq!(ago(3 * 86400), "3d ago");
        assert_eq!(fmt_age(UNIX_EPOCH, now), "1970-01-01");
    }

    #[test]
    fn utc_parts_matches_known_instants() {
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29T00:00:00Z
        assert_eq!(utc_parts(t), (2000, 2, 29, 0, 0, 0));
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(utc_parts(t), (2023, 11, 14, 22, 13, 20));
    }

    #[test]
    fn sort_puts_directories_first() {
        let tmp = TempDir::new();
        tmp.file("b.txt", "bb");
        tmp.file("a.txt", "a");
        tmp.file("z/inner", "");
        tmp.file(".hidden", "");
        let mut entries = list(tmp.path(), false).unwrap();
        sort(&mut entries, SortKey::Name, false);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["z", "a.txt", "b.txt"]);

        sort(&mut entries, SortKey::Size, false);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["z", "b.txt", "a.txt"]);

        let all = list(tmp.path(), true).unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn mode_string_renders_permissions() {
        let tmp = TempDir::new();
        let path = tmp.file("run.sh", "");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        let entry = Entry::from_path(path).unwrap();
        assert_eq!(mode_string(&entry), "-rwxr-xr--");
        assert!(entry.is_executable());
    }
}
