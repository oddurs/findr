//! File operations: create, rename, copy, move, trash, and the clipboard.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::dir::{split_ext, utc_parts};

fn occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn name_of(path: &Path) -> io::Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::other(format!("{} has no file name", path.display())))
}

/// A free path for `name` in `dir`, numbered the way Finder does: `a copy.txt`, `a copy 2.txt`.
pub fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !occupied(&first) {
        return first;
    }
    let (stem, ext) = split_ext(name);
    let mut n = 1;
    loop {
        let suffix = if n == 1 {
            " copy".to_string()
        } else {
            format!(" copy {n}")
        };
        let candidate = dir.join(format!("{stem}{suffix}{ext}"));
        if !occupied(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Creates `rel` under `dir` along with any missing parents; a trailing `/` makes a directory.
/// Returns the first component of `rel`, which is the name that appears in `dir`.
pub fn create(dir: &Path, rel: &str, as_dir: bool) -> io::Result<String> {
    let path = dir.join(rel);
    if occupied(&path) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{rel} already exists"),
        ));
    }
    if as_dir || rel.ends_with('/') {
        fs::create_dir_all(&path)?;
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::File::create_new(&path)?;
    }
    Ok(rel.split('/').next().unwrap_or(rel).to_string())
}

pub fn rename(from: &Path, to: &str) -> io::Result<()> {
    if to.contains('/') {
        return Err(io::Error::other("a name cannot contain /"));
    }
    let dest = from.with_file_name(to);
    // A case-only rename finds the same file on a case-insensitive disk; that is not a clash.
    let same = name_of(from)?.eq_ignore_ascii_case(to);
    if occupied(&dest) && !same {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{to} already exists"),
        ));
    }
    fs::rename(from, dest)
}

/// Turns the edited list of names back into renames: line `i` is the new name for
/// `originals[i]`. Unchanged lines are left out. Everything is checked before anything moves,
/// so a mistake in the list renames nothing.
pub fn plan_renames(
    originals: &[PathBuf],
    edited: &str,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let names: Vec<&str> = edited.lines().map(str::trim_end).collect();
    // Editors differ on whether they leave a trailing blank line; one is not a missing name.
    let names = match names.split_last() {
        Some((last, rest)) if last.is_empty() && rest.len() == originals.len() => rest.to_vec(),
        _ => names,
    };
    if names.len() != originals.len() {
        return Err(format!(
            "expected {} lines, found {}; nothing renamed",
            originals.len(),
            names.len()
        ));
    }
    let mut plan = Vec::new();
    for (from, name) in originals.iter().zip(&names) {
        if name.is_empty() {
            return Err(format!("{}: a name cannot be empty", from.display()));
        }
        if name.contains('/') {
            return Err(format!("{name}: a name cannot contain /"));
        }
        let to = from.with_file_name(name);
        if &to != from {
            plan.push((from.clone(), to));
        }
    }
    let mut targets = std::collections::BTreeSet::new();
    for (from, to) in &plan {
        if !targets.insert(to) {
            return Err(format!("{} is named twice; nothing renamed", to.display()));
        }
        // Another item in the list may be moving out of the way; anything else is a clash.
        let vacating = originals.contains(to);
        let case_only = from.parent() == to.parent()
            && name_of(from)
                .ok()
                .zip(name_of(to).ok())
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(&b));
        if occupied(to) && !vacating && !case_only {
            return Err(format!("{} already exists; nothing renamed", to.display()));
        }
    }
    Ok(plan)
}

/// Carries out a plan from `plan_renames`. Every source is first moved to a temporary name
/// beside it, so swaps and chains (`a→b`, `b→a`) cannot collide, then to its final name.
/// Returns how many were renamed.
pub fn apply_renames(plan: &[(PathBuf, PathBuf)]) -> io::Result<usize> {
    let staged: Vec<PathBuf> = plan
        .iter()
        .enumerate()
        .map(|(i, (from, _))| {
            from.with_file_name(format!(".findr-rename-{}-{i}", std::process::id()))
        })
        .collect();
    for ((from, _), temp) in plan.iter().zip(&staged) {
        fs::rename(from, temp)?;
    }
    for (done, ((_, to), temp)) in plan.iter().zip(&staged).enumerate() {
        if let Err(e) = fs::rename(temp, to) {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "renamed {done} of {}; {} is left as {}: {e}",
                    plan.len(),
                    to.display(),
                    temp.display()
                ),
            ));
        }
    }
    Ok(plan.len())
}

/// Copies files, directories and symlinks (as links) from `src` to `dst`, which must not exist.
pub fn copy_all(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        symlink(fs::read_link(src)?, dst)
    } else if meta.is_dir() {
        fs::create_dir(dst)?;
        for item in fs::read_dir(src)? {
            let item = item?;
            copy_all(&item.path(), &dst.join(item.file_name()))?;
        }
        fs::set_permissions(dst, meta.permissions())
    } else {
        fs::copy(src, dst).map(drop)
    }
}

pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_all(src, dst)?;
            if fs::symlink_metadata(src)?.is_dir() {
                fs::remove_dir_all(src)
            } else {
                fs::remove_file(src)
            }
        }
        result => result,
    }
}

pub struct Pasted {
    pub done: usize,
    /// The name the last item landed under, for putting the cursor on it.
    pub last: Option<String>,
    pub failure: Option<String>,
}

/// Copies, or with `cut` moves, `paths` into `dest` under free names, stopping at the first
/// failure. `progress` hears the running count after each item.
pub fn paste(paths: &[PathBuf], dest: &Path, cut: bool, mut progress: impl FnMut(usize)) -> Pasted {
    let mut pasted = Pasted {
        done: 0,
        last: None,
        failure: None,
    };
    for src in paths {
        if cut && src.parent() == Some(dest) {
            continue;
        }
        if src.is_dir() && dest.starts_with(src) {
            pasted.failure = Some(format!("cannot paste {} into itself", src.display()));
            break;
        }
        let result = name_of(src).and_then(|name| {
            let dst = unique_dest(dest, &name);
            if cut {
                move_path(src, &dst)
            } else {
                copy_all(src, &dst)
            }
            .map(|()| dst)
        });
        match result {
            Ok(dst) => {
                pasted.done += 1;
                pasted.last = dst.file_name().map(|n| n.to_string_lossy().into_owned());
                progress(pasted.done);
            }
            Err(e) => {
                pasted.failure = Some(format!("{}: {e}", src.display()));
                break;
            }
        }
    }
    pasted
}

#[derive(Debug, Clone)]
pub enum Trash {
    /// A plain directory that items are moved into, as macOS's `~/.Trash`.
    Dir(PathBuf),
    /// A freedesktop.org trash, with `files/` and `info/`.
    Xdg(PathBuf),
}

impl Trash {
    pub fn detect() -> Option<Trash> {
        let home = PathBuf::from(env::var_os("HOME")?);
        if cfg!(target_os = "macos") {
            return Some(Trash::Dir(home.join(".Trash")));
        }
        let data = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Some(Trash::Xdg(data.join("Trash")))
    }

    pub fn put(&self, path: &Path) -> io::Result<PathBuf> {
        let name = name_of(path)?;
        match self {
            Trash::Dir(dir) => {
                fs::create_dir_all(dir)?;
                let dst = unique_dest(dir, &name);
                move_path(path, &dst)?;
                Ok(dst)
            }
            Trash::Xdg(root) => {
                let (files, info) = (root.join("files"), root.join("info"));
                fs::create_dir_all(&files)?;
                fs::create_dir_all(&info)?;
                let dst = unique_dest(&files, &name);
                let info_file = info.join(format!("{}.trashinfo", name_of(&dst)?));
                let (y, mo, d, h, mi, s) = utc_parts(SystemTime::now());
                fs::write(
                    &info_file,
                    format!(
                        "[Trash Info]\nPath={}\nDeletionDate={y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}\n",
                        percent_encode(path)
                    ),
                )?;
                if let Err(e) = move_path(path, &dst) {
                    // The move failed, so the info file describes nothing; the move error is the one to report.
                    let _ = fs::remove_file(&info_file);
                    return Err(e);
                }
                Ok(dst)
            }
        }
    }
}

fn percent_encode(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    const TOOLS: [(&str, &[&str]); 4] = [
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (tool, args) in TOOLS {
        let mut child = match Command::new(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let status = child.wait()?;
        return if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("{tool} exited with {status}")))
        };
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no clipboard tool found (pbcopy, wl-copy, xclip, xsel)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;

    #[test]
    fn unique_dest_numbers_like_finder() {
        let tmp = TempDir::new();
        assert_eq!(unique_dest(tmp.path(), "a.txt"), tmp.path().join("a.txt"));
        tmp.file("a.txt", "");
        assert_eq!(
            unique_dest(tmp.path(), "a.txt"),
            tmp.path().join("a copy.txt")
        );
        tmp.file("a copy.txt", "");
        assert_eq!(
            unique_dest(tmp.path(), "a.txt"),
            tmp.path().join("a copy 2.txt")
        );
        tmp.file(".env", "");
        assert_eq!(
            unique_dest(tmp.path(), ".env"),
            tmp.path().join(".env copy")
        );
    }

    #[test]
    fn create_makes_parents_and_refuses_clobbering() {
        let tmp = TempDir::new();
        assert_eq!(create(tmp.path(), "src/lib.rs", false).unwrap(), "src");
        assert!(tmp.path().join("src/lib.rs").is_file());
        assert_eq!(create(tmp.path(), "a/b/", false).unwrap(), "a");
        assert!(tmp.path().join("a/b").is_dir());
        assert_eq!(create(tmp.path(), "d", true).unwrap(), "d");
        assert!(tmp.path().join("d").is_dir());
        let err = create(tmp.path(), "src/lib.rs", false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn rename_refuses_to_overwrite() {
        let tmp = TempDir::new();
        let a = tmp.file("a", "1");
        tmp.file("b", "2");
        assert_eq!(
            rename(&a, "b").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(rename(&a, "x/y").is_err());
        rename(&a, "c").unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("c")).unwrap(), "1");
    }

    #[test]
    fn a_rename_plan_is_checked_before_anything_moves() {
        let tmp = TempDir::new();
        let a = tmp.file("a.txt", "a");
        let b = tmp.file("b.txt", "b");
        tmp.file("taken.txt", "");
        let originals = vec![a.clone(), b.clone()];
        assert!(
            plan_renames(&originals, "a.txt\n")
                .unwrap_err()
                .contains("expected 2 lines")
        );
        assert!(
            plan_renames(&originals, "x\n\n")
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            plan_renames(&originals, "x/y\nb.txt\n")
                .unwrap_err()
                .contains("/")
        );
        assert!(
            plan_renames(&originals, "c\nc\n")
                .unwrap_err()
                .contains("twice")
        );
        assert!(
            plan_renames(&originals, "taken.txt\nb.txt\n")
                .unwrap_err()
                .contains("exists")
        );
        // Unchanged lines are left out; a trailing blank line is not a missing name.
        let plan = plan_renames(&originals, "a.txt\nc.txt\n\n").unwrap();
        assert_eq!(plan, [(b.clone(), tmp.path().join("c.txt"))]);
        // Taking a name another item is vacating is allowed.
        assert_eq!(plan_renames(&originals, "b.txt\na.txt").unwrap().len(), 2);
    }

    #[test]
    fn renames_can_swap_and_chain() {
        let tmp = TempDir::new();
        let a = tmp.file("a", "was a");
        let b = tmp.file("b", "was b");
        let c = tmp.file("c", "was c");
        let plan = plan_renames(&[a.clone(), b.clone(), c.clone()], "b\nc\na\n").unwrap();
        assert_eq!(apply_renames(&plan).unwrap(), 3);
        assert_eq!(fs::read_to_string(&b).unwrap(), "was a");
        assert_eq!(fs::read_to_string(&c).unwrap(), "was b");
        assert_eq!(fs::read_to_string(&a).unwrap(), "was c");
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            3,
            "no temporary names left"
        );
    }

    #[test]
    fn copy_all_recurses_and_keeps_links() {
        let tmp = TempDir::new();
        tmp.file("src/one.txt", "1");
        tmp.file("src/deep/two.txt", "2");
        symlink("one.txt", tmp.path().join("src/link")).unwrap();
        copy_all(&tmp.path().join("src"), &tmp.path().join("dst")).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("dst/deep/two.txt")).unwrap(),
            "2"
        );
        assert_eq!(
            fs::read_link(tmp.path().join("dst/link")).unwrap(),
            Path::new("one.txt")
        );
        assert!(tmp.path().join("src/one.txt").exists());
    }

    #[test]
    fn trash_dir_moves_and_avoids_collisions() {
        let tmp = TempDir::new();
        let trash = Trash::Dir(tmp.path().join("trash"));
        let first = trash.put(&tmp.file("x.txt", "1")).unwrap();
        let second = trash.put(&tmp.file("x.txt", "2")).unwrap();
        assert_eq!(first, tmp.path().join("trash/x.txt"));
        assert_eq!(second, tmp.path().join("trash/x copy.txt"));
        assert!(!tmp.path().join("x.txt").exists());
    }

    #[test]
    fn trash_xdg_writes_info() {
        let tmp = TempDir::new();
        let trash = Trash::Xdg(tmp.path().join("Trash"));
        let src = tmp.file("my file.txt", "");
        trash.put(&src).unwrap();
        assert!(tmp.path().join("Trash/files/my file.txt").exists());
        let info = fs::read_to_string(tmp.path().join("Trash/info/my file.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"));
        assert!(info.contains("my%20file.txt\nDeletionDate="));
    }
}
