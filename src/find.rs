//! Finding a file anywhere below a directory: building the candidate list, and ranking it.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use crate::dir::natural_cmp;
use crate::fuzzy;
use crate::git;

/// A walk outside git stops here, so pointing findr at `/` or `~` stays usable.
const MAX_ENTRIES: usize = 200_000;
const BASENAME_BONUS: i64 = 24;

/// Every path below `root`, relative to it and `/`-separated. Directories end in `/`.
pub struct Index {
    pub root: PathBuf,
    pub paths: Vec<String>,
    /// The walk hit `MAX_ENTRIES` and stopped.
    pub truncated: bool,
}

pub struct Match {
    pub idx: usize,
    pub hits: Vec<usize>,
}

pub fn spawn(root: PathBuf, show_hidden: bool) -> Receiver<io::Result<Index>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // A send fails only if the finder was closed before the index was ready.
        let _ = tx.send(build(&root, show_hidden));
    });
    rx
}

pub fn build(root: &Path, show_hidden: bool) -> io::Result<Index> {
    // Inside a repository git already knows what is worth finding: tracked and untracked
    // files, minus everything .gitignore excludes (target/, node_modules/, …).
    let (files, truncated) = match git::find_root(root) {
        Some(_) => (git_files(root)?, false),
        None => walk(root, show_hidden)?,
    };
    let mut paths = BTreeSet::new();
    for file in files {
        if !show_hidden && file.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        // The directories on the way are worth jumping to as well.
        let mut end = 0;
        while let Some(slash) = file[end..].find('/') {
            end += slash + 1;
            paths.insert(file[..end].to_string());
        }
        paths.insert(file);
    }
    let mut paths: Vec<String> = paths.into_iter().collect();
    // Shallow paths first, so an empty query shows the top of the tree.
    paths.sort_by(|a, b| depth(a).cmp(&depth(b)).then_with(|| natural_cmp(a, b)));
    Ok(Index {
        root: root.to_path_buf(),
        paths,
        truncated,
    })
}

fn depth(path: &str) -> usize {
    path.trim_end_matches('/').matches('/').count()
}

fn git_files(root: &Path) -> io::Result<Vec<String>> {
    let mut git = Command::new("git");
    for var in git::REPO_ENV {
        git.env_remove(var);
    }
    // One call rather than two: -t tags each entry, which is how a tracked file deleted from
    // the work tree (listed once as H, cached, and once as R, removed) is told apart.
    let out = git
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "-t",
            "--cached",
            "--deleted",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(format!("git ls-files: {}", err.trim())));
    }
    let mut files = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    for field in out.stdout.split(|b| *b == 0) {
        let Some((&tag, rest)) = field.split_first() else {
            continue;
        };
        let path = String::from_utf8_lossy(rest.strip_prefix(b" ").unwrap_or(rest)).into_owned();
        if tag == b'R' {
            deleted.insert(path);
        } else {
            files.insert(path);
        }
    }
    Ok(files.difference(&deleted).cloned().collect())
}

fn walk(root: &Path, show_hidden: bool) -> io::Result<(Vec<String>, bool)> {
    let mut files = Vec::new();
    let mut repos = Vec::new();
    let mut truncated = false;
    let mut stack = vec![PathBuf::new()];
    'walk: while let Some(rel) = stack.pop() {
        let entries = match fs::read_dir(root.join(&rel)) {
            Ok(entries) => entries,
            // Unreadable directories below the root are skipped; the root itself must read.
            Err(e)
                if !rel.as_os_str().is_empty() && e.kind() == io::ErrorKind::PermissionDenied =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            // Skipped here rather than filtered afterwards, so hidden trees (a .git with
            // hundreds of thousands of objects) cost nothing and do not count toward the cap.
            // Git's own directory is never worth finding.
            let hidden = file_name.as_encoded_bytes().starts_with(b".");
            if (hidden && !show_hidden) || file_name == ".git" {
                continue;
            }
            let path = rel.join(file_name);
            let name = path.to_string_lossy().into_owned();
            // file_type does not follow links, so a symlinked directory cannot loop the walk.
            if entry.file_type()?.is_dir() {
                files.push(format!("{name}/"));
                // A repository below the root: git says what is in it, so its .gitignore
                // keeps target/ and node_modules/ out, as it does at the top.
                if root.join(&path).join(".git").exists() {
                    repos.push(name);
                } else {
                    stack.push(path);
                }
            } else {
                files.push(name);
            }
            if files.len() >= MAX_ENTRIES {
                truncated = true;
                break 'walk;
            }
        }
    }
    for (name, listed) in repos.iter().zip(list_repos(root, &repos)) {
        match listed {
            Ok(inner) => files.extend(inner.into_iter().map(|f| format!("{name}/{f}"))),
            // A .git that git cannot read (broken, or not a repository at all) is walked
            // like any other directory.
            Err(_) => {
                let (inner, cut) = walk(&root.join(name), show_hidden)?;
                files.extend(inner.into_iter().map(|f| format!("{name}/{f}")));
                truncated |= cut;
            }
        }
    }
    if files.len() > MAX_ENTRIES {
        files.truncate(MAX_ENTRIES);
        truncated = true;
    }
    Ok((files, truncated))
}

/// Lists each repository with git, several at once: the cost is mostly starting processes,
/// and a directory of checkouts can hold dozens.
fn list_repos(root: &Path, repos: &[String]) -> Vec<io::Result<Vec<String>>> {
    // Without a CPU count, a few at a time is still far better than one.
    let workers = thread::available_parallelism().map_or(4, |n| n.get());
    let mut listed = Vec::with_capacity(repos.len());
    for chunk in repos.chunks(workers) {
        thread::scope(|scope| {
            let running: Vec<_> = chunk
                .iter()
                .map(|name| scope.spawn(move || git_files(&root.join(name))))
                .collect();
            for handle in running {
                let result = handle
                    .join()
                    .unwrap_or_else(|_| Err(io::Error::other("git ls-files panicked")));
                listed.push(result);
            }
        });
    }
    listed
}

/// The best `limit` matches for `query`, best first. An empty query keeps index order.
pub fn rank(index: &Index, query: &str, limit: usize) -> Vec<Match> {
    if query.is_empty() {
        return (0..index.paths.len().min(limit))
            .map(|idx| Match {
                idx,
                hits: Vec::new(),
            })
            .collect();
    }
    let case_sensitive = query.chars().any(char::is_uppercase);
    let needle: Vec<char> = if case_sensitive {
        query.chars().collect()
    } else {
        query.chars().flat_map(char::to_lowercase).collect()
    };
    let mut scored: Vec<(i64, Match)> = index
        .paths
        .iter()
        .enumerate()
        // Most paths fail a plain subsequence test, which is far cheaper than scoring them.
        .filter(|(_, path)| is_subsequence(&needle, path, case_sensitive))
        .filter_map(|(idx, path)| {
            let (score, hits) = fuzzy::score(query, path)?;
            let name_start = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
            let name_start = path[..name_start].chars().count();
            let in_name = hits.last().is_some_and(|&h| h >= name_start);
            let bonus = if in_name { BASENAME_BONUS } else { 0 };
            Some((score + bonus, Match { idx, hits }))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.truncate(limit);
    scored.into_iter().map(|(_, m)| m).collect()
}

fn is_subsequence(needle: &[char], haystack: &str, case_sensitive: bool) -> bool {
    let mut want = needle.iter().peekable();
    for c in haystack.chars() {
        let Some(&&next) = want.peek() else {
            break;
        };
        let c = if case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        };
        if c == next {
            want.next();
        }
    }
    want.peek().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;

    fn found(index: &Index, query: &str) -> Vec<String> {
        rank(index, query, 10)
            .into_iter()
            .map(|m| index.paths[m.idx].clone())
            .collect()
    }

    #[test]
    fn walks_a_plain_directory_with_its_subdirectories() {
        let tmp = TempDir::new();
        tmp.file("src/ui/list.rs", "");
        tmp.file("README.md", "");
        tmp.file(".hidden/secret", "");
        let index = build(tmp.path(), false).unwrap();
        assert_eq!(
            index.paths,
            ["README.md", "src/", "src/ui/", "src/ui/list.rs"]
        );
        assert!(!index.truncated);
        let all = build(tmp.path(), true).unwrap();
        assert!(all.paths.contains(&".hidden/secret".to_string()));
        tmp.file("vendor/.git/objects/ab", "");
        let all = build(tmp.path(), true).unwrap();
        assert!(all.paths.contains(&"vendor/".to_string()));
        assert!(
            !all.paths.iter().any(|p| p.contains(".git")),
            "{:?}",
            all.paths
        );
    }

    #[test]
    fn deleted_tracked_files_are_not_found() {
        let tmp = TempDir::new();
        let git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            for var in git::REPO_ENV {
                cmd.env_remove(var);
            }
            let out = cmd.arg("-C").arg(tmp.path()).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q"]);
        tmp.file("kept.rs", "");
        tmp.file("gone.rs", "");
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        fs::remove_file(tmp.path().join("gone.rs")).unwrap();
        tmp.file("new.rs", "");
        assert_eq!(
            build(tmp.path(), false).unwrap().paths,
            ["kept.rs", "new.rs"]
        );
    }

    #[test]
    fn inside_git_it_honours_gitignore() {
        let tmp = TempDir::new();
        let mut init = Command::new("git");
        for var in git::REPO_ENV {
            init.env_remove(var);
        }
        let out = init.arg("init").arg("-q").arg(tmp.path()).output().unwrap();
        assert!(out.status.success(), "git init: {out:?}");
        tmp.file(".gitignore", "target/\n");
        tmp.file("target/debug/big.bin", "");
        tmp.file("src/main.rs", "");
        let index = build(tmp.path(), false).unwrap();
        assert_eq!(index.paths, ["src/", "src/main.rs"]);
        let sub = build(&tmp.path().join("src"), false).unwrap();
        assert_eq!(
            sub.paths,
            ["main.rs"],
            "paths are relative to where the search starts"
        );
    }

    #[test]
    fn repositories_below_a_plain_directory_honour_their_gitignore() {
        let tmp = TempDir::new();
        let mut init = Command::new("git");
        for var in git::REPO_ENV {
            init.env_remove(var);
        }
        let repo = tmp.path().join("proj");
        let out = init.arg("init").arg("-q").arg(&repo).output().unwrap();
        assert!(out.status.success(), "git init: {out:?}");
        tmp.file("proj/.gitignore", "node_modules/\n");
        tmp.file("proj/node_modules/dep/index.js", "");
        tmp.file("proj/app.js", "");
        tmp.file("notes.txt", "");
        let index = build(tmp.path(), false).unwrap();
        assert_eq!(index.paths, ["notes.txt", "proj/", "proj/app.js"]);
    }

    #[test]
    fn ranks_file_name_matches_first() {
        let index = Index {
            root: PathBuf::from("/r"),
            paths: vec![
                "app/models/user.rb".into(),
                "src/app.rs".into(),
                "apps/".into(),
            ],
            truncated: false,
        };
        assert_eq!(
            found(&index, "app"),
            ["apps/", "src/app.rs", "app/models/user.rb"]
        );
        assert_eq!(found(&index, "user"), ["app/models/user.rb"]);
        assert!(found(&index, "zzz").is_empty());
        assert_eq!(found(&index, "").len(), 3);
    }

    #[test]
    fn subsequence_prefilter_agrees_with_the_scorer() {
        for (needle, hay) in [
            ("mr", "main.rs"),
            ("MR", "main.rs"),
            ("rs", "sr"),
            ("", "x"),
        ] {
            let case_sensitive = needle.chars().any(char::is_uppercase);
            let chars: Vec<char> = if case_sensitive {
                needle.chars().collect()
            } else {
                needle.chars().flat_map(char::to_lowercase).collect()
            };
            assert_eq!(
                is_subsequence(&chars, hay, case_sensitive),
                fuzzy::score(needle, hay).is_some(),
                "{needle:?} in {hay:?}"
            );
        }
    }
}
