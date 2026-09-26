//! Git status for whatever repository the current directory is in, read from `git status`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Ordered by how much it matters, so a directory shows the most pressing status inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ignored,
    Untracked,
    Deleted,
    Renamed,
    Added,
    Modified,
    Conflicted,
}

impl Status {
    pub fn symbol(self) -> char {
        match self {
            Status::Ignored => '!',
            Status::Untracked => '?',
            Status::Deleted => 'D',
            Status::Renamed => 'R',
            Status::Added => 'A',
            Status::Modified => 'M',
            Status::Conflicted => 'U',
        }
    }
}

#[derive(Debug)]
pub struct Repo {
    pub root: PathBuf,
    pub branch: Option<String>,
    statuses: Vec<(String, Status)>,
}

/// The nearest ancestor of `dir` holding a `.git` (a directory, or a file in a worktree).
pub fn find_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

pub struct Response {
    pub root: PathBuf,
    pub repo: io::Result<Repo>,
}

/// A worker that runs `git status` for repository roots sent to it. In a large repository
/// that takes long enough to stall the cursor, so it never runs on the event loop.
pub fn spawn() -> (Sender<PathBuf>, Receiver<Response>) {
    let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
    let (resp_tx, resp_rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(mut root) = req_rx.recv() {
            // Only the newest request matters; the rest are for places already left behind.
            while let Ok(newer) = req_rx.try_recv() {
                root = newer;
            }
            let repo = Repo::load(&root);
            if resp_tx.send(Response { root, repo }).is_err() {
                break;
            }
        }
    });
    (req_tx, resp_rx)
}

impl Repo {
    pub fn load(root: &Path) -> io::Result<Repo> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--ignored",
                "--untracked-files=normal",
            ])
            .stdin(Stdio::null())
            .output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(io::Error::other(err.trim().to_string()));
        }
        let (branch, statuses) = parse_porcelain(&out.stdout);
        Ok(Repo {
            root: root.to_path_buf(),
            branch,
            statuses,
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.statuses.iter().any(|(_, s)| *s != Status::Ignored)
    }

    /// The status of `path`: its own, the most pressing one beneath it for a directory,
    /// or the status of an untracked or ignored directory it sits inside.
    pub fn status_of(&self, path: &Path) -> Option<Status> {
        let rel = path.strip_prefix(&self.root).ok()?.to_str()?;
        let mut best = None;
        for (p, s) in &self.statuses {
            let hit = if rel.is_empty() {
                *s != Status::Ignored
            } else if p == rel {
                true
            } else if is_under(p, rel) {
                // Ignored files do not make their directory look ignored.
                *s != Status::Ignored
            } else {
                is_under(rel, p) && matches!(s, Status::Ignored | Status::Untracked)
            };
            if hit {
                best = best.max(Some(*s));
            }
        }
        best
    }
}

fn is_under(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn parse_porcelain(raw: &[u8]) -> (Option<String>, Vec<(String, Status)>) {
    let mut branch = None;
    let mut statuses = Vec::new();
    let mut fields = raw.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(field) = fields.next() {
        let line = String::from_utf8_lossy(field);
        if let Some(head) = line.strip_prefix("## ") {
            let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
            let name = head.split("...").next().unwrap_or(head);
            branch = name.split(' ').next().map(str::to_string);
            continue;
        }
        if field.len() < 4 {
            continue;
        }
        let (x, y, path) = (field[0], field[1], &line[3..]);
        let status = match (x, y) {
            (b'!', b'!') => Status::Ignored,
            (b'?', b'?') => Status::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Status::Conflicted,
            (b'R' | b'C', _) => {
                // A rename carries its source path as the next field.
                fields.next();
                Status::Renamed
            }
            _ if x == b'M' || y == b'M' || x == b'T' || y == b'T' => Status::Modified,
            (b'A', _) => Status::Added,
            _ if x == b'D' || y == b'D' => Status::Deleted,
            _ => Status::Modified,
        };
        statuses.push((path.trim_end_matches('/').to_string(), status));
    }
    (branch, statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Repo {
        let raw = b"## main...origin/main [ahead 1]\0 M src/app.rs\0?? notes/\0!! target/\0R  new.rs\0old.rs\0A  added.rs\0!! src/gen.rs\0UU both.rs\0";
        let (branch, statuses) = parse_porcelain(raw);
        Repo {
            root: PathBuf::from("/r"),
            branch,
            statuses,
        }
    }

    #[test]
    fn parses_branch_and_entries() {
        let r = repo();
        assert_eq!(r.branch.as_deref(), Some("main"));
        assert_eq!(r.statuses.len(), 7);
        assert!(r.statuses.contains(&("new.rs".into(), Status::Renamed)));
        assert!(!r.statuses.iter().any(|(p, _)| p == "old.rs"));
    }

    #[test]
    fn branch_before_first_commit() {
        let (branch, _) = parse_porcelain(b"## No commits yet on trunk\0");
        assert_eq!(branch.as_deref(), Some("trunk"));
    }

    #[test]
    fn status_lookup() {
        let r = repo();
        let st = |p: &str| r.status_of(Path::new(p));
        assert_eq!(st("/r/src/app.rs"), Some(Status::Modified));
        assert_eq!(st("/r/src"), Some(Status::Modified));
        assert_eq!(st("/r/src/gen.rs"), Some(Status::Ignored));
        assert_eq!(st("/r/notes"), Some(Status::Untracked));
        assert_eq!(st("/r/notes/deep/a.md"), Some(Status::Untracked));
        assert_eq!(st("/r/target"), Some(Status::Ignored));
        assert_eq!(st("/r/target/debug"), Some(Status::Ignored));
        assert_eq!(st("/r/new.rs"), Some(Status::Renamed));
        assert_eq!(st("/r/both.rs"), Some(Status::Conflicted));
        assert_eq!(st("/r/clean.rs"), None);
        assert_eq!(st("/r/sr"), None);
        assert_eq!(st("/r"), Some(Status::Conflicted));
        assert_eq!(st("/elsewhere"), None);
    }
}
