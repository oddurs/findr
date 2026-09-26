//! Application state and key handling. Drawing lives in `ui`, terminal I/O in `main`.

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::dir::{self, Entry, SortKey};
use crate::find;
use crate::fuzzy;
use crate::git::{self, Repo};
use crate::ops::{self, Trash};
use crate::preview::{self, Preview};

const MESSAGE_TTL: Duration = Duration::from_secs(4);
/// Two clicks on the same row this close together open it, as in Finder.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const WHEEL_STEP: isize = 3;
const PREVIEW_STEP: usize = 3;

pub struct ViewItem {
    pub idx: usize,
    pub hits: Vec<usize>,
}

pub enum Mode {
    Normal,
    Filter,
    Input(Input),
    Confirm(Confirm),
    Help,
    Goto,
    Find(Finder),
}

/// Rows the finder ranks and keeps; more than fit on any screen.
pub const FIND_LIMIT: usize = 500;
/// Indexes kept for reopening find instantly; each is the paths below one directory.
const FIND_CACHE: usize = 8;

/// The find-anywhere prompt: a query over an index built on a worker.
pub struct Finder {
    pub query: String,
    pub index: Option<Arc<find::Index>>,
    pub matches: Vec<find::Match>,
    pub selected: usize,
    rx: Option<Receiver<io::Result<find::Index>>>,
}

impl Finder {
    /// Opens on `cached` if there is one, so the list is there at once, and rebuilds the index
    /// behind it either way: files come and go between visits.
    fn open(root: PathBuf, show_hidden: bool, cached: Option<Arc<find::Index>>) -> Finder {
        let mut finder = Finder {
            query: String::new(),
            index: cached,
            matches: Vec::new(),
            selected: 0,
            rx: Some(find::spawn(root, show_hidden)),
        };
        finder.rerank();
        finder
    }

    /// An index is being built, fresh or to replace the cached one.
    pub fn indexing(&self) -> bool {
        self.rx.is_some()
    }

    fn rerank(&mut self) {
        if let Some(index) = &self.index {
            self.matches = find::rank(index, &self.query, FIND_LIMIT);
        }
        self.selected = 0;
    }

    fn set_query(&mut self, query: String) {
        self.query = query;
        self.rerank();
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    fn chosen(&self) -> Option<PathBuf> {
        let index = self.index.as_ref()?;
        Some(index.root.join(self.chosen_path()?))
    }

    fn chosen_path(&self) -> Option<&str> {
        let index = self.index.as_ref()?;
        let m = self.matches.get(self.selected)?;
        Some(&index.paths[m.idx])
    }

    /// Takes a newer index, keeping the query and, where it still exists, the selection.
    fn replace_index(&mut self, index: Arc<find::Index>) {
        let keep = self.chosen_path().map(str::to_string);
        self.index = Some(index);
        self.rerank();
        if let (Some(keep), Some(index)) = (keep, &self.index) {
            let at = self.matches.iter().position(|m| index.paths[m.idx] == keep);
            self.selected = at.unwrap_or(0);
        }
    }
}

pub enum Confirm {
    Trash(Vec<PathBuf>),
    /// Quitting mid-paste would leave a half-copied tree behind.
    Quit {
        write_cwd: bool,
    },
}

/// A paste running on a worker thread.
pub struct Job {
    pub dest: PathBuf,
    /// A cut moves its items, a copy makes new ones; undo differs.
    pub cut: bool,
    pub total: usize,
    pub done: usize,
    rx: Receiver<JobEvent>,
}

enum JobEvent {
    Progress(usize),
    Finished(ops::Pasted),
}

pub enum InputKind {
    Rename(PathBuf),
    NewFile,
    NewDir,
    Jump,
}

impl InputKind {
    pub fn prompt(&self) -> &'static str {
        match self {
            InputKind::Rename(_) => "rename",
            InputKind::NewFile => "new file",
            InputKind::NewDir => "new directory",
            InputKind::Jump => "go to",
        }
    }
}

pub struct Input {
    pub kind: InputKind,
    pub text: String,
    /// In chars, not bytes.
    pub cursor: usize,
}

impl Input {
    fn new(kind: InputKind, text: &str, cursor: usize) -> Input {
        Input {
            kind,
            text: text.to_string(),
            cursor,
        }
    }

    pub fn byte(&self, cursor: usize) -> usize {
        self.text
            .char_indices()
            .nth(cursor)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn edit(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.byte(self.cursor));
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let end = self.byte(self.cursor);
                let start = word_start(&self.text[..end]);
                self.cursor = self.text[..start].chars().count();
                self.text.drain(start..end);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.byte(self.cursor));
            }
            KeyCode::Delete if self.cursor < len => {
                self.text.remove(self.byte(self.cursor));
            }
            KeyCode::Char(c) if !ctrl => {
                self.text.insert(self.byte(self.cursor), c);
                self.cursor += 1;
            }
            _ => {}
        }
    }
}

/// Byte index where the last word of `s` starts, for ctrl-w.
fn word_start(s: &str) -> usize {
    let sep = |c: char| matches!(c, ' ' | '/' | '.' | '-' | '_');
    s.trim_end_matches(sep).rfind(sep).map_or(0, |i| i + 1)
}

pub struct Clip {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Something to know: a toggle's new state, a cancelled prompt.
    Info,
    /// Something that was asked for and happened.
    Success,
    Error,
}

pub struct Message {
    pub text: String,
    pub kind: MessageKind,
    at: Instant,
}

/// Where the last draw put each column, so a click can be mapped to a row.
#[derive(Default, Clone, Copy)]
pub struct Areas {
    pub parent: Rect,
    pub current: Rect,
    pub preview: Rect,
}

/// The first parent-column row shown: the current directory kept near the middle.
pub fn parent_offset(selected: usize, len: usize, height: usize) -> usize {
    selected
        .saturating_sub(height / 2)
        .min(len.saturating_sub(height))
}

/// Something `u` can take back, recorded as it happens.
enum Undo {
    /// (where it was, where it went in the trash)
    Trash(Vec<(PathBuf, PathBuf)>),
    /// (old path, new path), for one rename or a bulk one.
    Rename(Vec<(PathBuf, PathBuf)>),
    /// A cut and paste: (where it came from, where it went).
    Move(Vec<(PathBuf, PathBuf)>),
    /// Things that did not exist before: pasted copies, new files and directories. Undone by
    /// moving them to the trash, so the undo can itself be recovered.
    Create(Vec<PathBuf>),
}

/// A rename of several entries, waiting for the editor to return with the new names.
struct BulkRename {
    list: PathBuf,
    originals: Vec<PathBuf>,
}

/// Work that needs the terminal, which `main` owns.
pub enum Effect {
    Quit { write_cwd: bool },
    Run(Command),
}

pub struct App {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub view: Vec<ViewItem>,
    pub selected: usize,
    pub offset: usize,
    pub parent: Vec<Entry>,
    pub parent_selected: Option<usize>,
    pub show_hidden: bool,
    pub sort: SortKey,
    pub reverse: bool,
    pub filter: String,
    pub mode: Mode,
    pub marked: BTreeSet<PathBuf>,
    pub clip: Option<Clip>,
    pub job: Option<Job>,
    bulk: Option<BulkRename>,
    /// What `u` can take back, newest last.
    undo: Vec<Undo>,
    /// Recent find indexes by (show_hidden, root), newest first.
    find_cache: Vec<(bool, Arc<find::Index>)>,
    pub git: Option<Repo>,
    pub message: Option<Message>,
    pub preview: Option<(preview::Key, Preview)>,
    /// Show a changed file's diff in the inspector instead of its content. Sticky, so the
    /// changes can be reviewed file by file with j and k.
    pub diff: bool,
    pub preview_scroll: usize,
    /// Rows in the file list, recorded by the last draw; drives page movement.
    pub page: usize,
    pub areas: Areas,
    last_click: Option<(Instant, PathBuf)>,
    back: Vec<PathBuf>,
    cursors: HashMap<PathBuf, String>,
    cwd_mtime: Option<SystemTime>,
    trash: Option<Trash>,
    requested: Option<preview::Key>,
    preview_tx: Sender<preview::Request>,
    preview_rx: Receiver<preview::Response>,
    git_wanted: Option<PathBuf>,
    git_pending: bool,
    git_tx: Sender<PathBuf>,
    git_rx: Receiver<git::Response>,
}

impl App {
    /// Opens `start`; a file opens its directory with the cursor on it.
    pub fn new(start: &Path, trash: Option<Trash>) -> io::Result<App> {
        let start = std::fs::canonicalize(start)?;
        let (dir, select) = match start.parent() {
            Some(parent) if !start.is_dir() => (
                parent.to_path_buf(),
                start.file_name().map(|n| n.to_string_lossy().into_owned()),
            ),
            _ => (start, None),
        };
        let (preview_tx, preview_rx) = preview::spawn();
        let (git_tx, git_rx) = git::spawn();
        let mut app = App {
            cwd: dir.clone(),
            entries: Vec::new(),
            view: Vec::new(),
            selected: 0,
            offset: 0,
            parent: Vec::new(),
            parent_selected: None,
            show_hidden: false,
            sort: SortKey::Name,
            reverse: false,
            filter: String::new(),
            mode: Mode::Normal,
            marked: BTreeSet::new(),
            clip: None,
            job: None,
            bulk: None,
            undo: Vec::new(),
            find_cache: Vec::new(),
            git: None,
            message: None,
            preview: None,
            preview_scroll: 0,
            page: 20,
            areas: Areas::default(),
            last_click: None,
            back: Vec::new(),
            cursors: HashMap::new(),
            cwd_mtime: None,
            trash,
            requested: None,
            diff: false,
            preview_tx,
            preview_rx,
            git_wanted: None,
            git_pending: false,
            git_tx,
            git_rx,
        };
        app.load(dir, select)?;
        Ok(app)
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.view.get(self.selected).map(|v| &self.entries[v.idx])
    }

    pub fn message(&self) -> Option<&Message> {
        self.message
            .as_ref()
            .filter(|m| m.at.elapsed() < MESSAGE_TTL)
    }

    fn say(&mut self, kind: MessageKind, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }

    fn info(&mut self, text: impl Into<String>) {
        self.say(MessageKind::Info, text);
    }

    fn success(&mut self, text: impl Into<String>) {
        self.say(MessageKind::Success, text);
    }

    fn error(&mut self, text: impl Into<String>) {
        self.say(MessageKind::Error, text);
    }

    /// Lists `dir` and makes it current, with the cursor on `select` or wherever it last was there.
    fn load(&mut self, dir: PathBuf, select: Option<String>) -> io::Result<()> {
        let mut entries = dir::list(&dir, self.show_hidden)?;
        dir::sort(&mut entries, self.sort, self.reverse);
        if let Some(name) = self.selected().map(|e| e.name.clone()) {
            self.cursors.insert(self.cwd.clone(), name);
        }
        if dir != self.cwd {
            self.filter.clear();
            self.offset = 0;
        }
        self.cwd_mtime = dir::mtime(&dir);
        self.cwd = dir;
        self.entries = entries;
        self.load_parent();
        self.sync_git(false);
        let select = select.or_else(|| self.cursors.get(&self.cwd).cloned());
        self.rebuild_view(select.as_deref());
        Ok(())
    }

    fn load_parent(&mut self) {
        self.parent.clear();
        self.parent_selected = None;
        let Some(parent) = self.cwd.parent() else {
            return;
        };
        // An unreadable parent shows as an empty column; the current directory still works.
        let Ok(mut entries) = dir::list(parent, true) else {
            return;
        };
        entries.retain(|e| self.show_hidden || !e.is_hidden() || e.path == self.cwd);
        dir::sort(&mut entries, self.sort, self.reverse);
        self.parent_selected = entries.iter().position(|e| e.path == self.cwd);
        self.parent = entries;
    }

    /// Asks for git status when the repository changed, or always with `force`. The answer
    /// arrives through `poll_git`.
    fn sync_git(&mut self, force: bool) {
        let root = git::find_root(&self.cwd);
        // Another repository's status would be wrong here; the old one stays up only while it
        // is still the right repository, so a refresh does not flicker.
        if self.git.as_ref().map(|r| &r.root) != root.as_ref() {
            self.git = None;
        }
        let Some(root) = root else {
            self.git_wanted = None;
            return;
        };
        if !force && (self.git.is_some() || self.git_wanted.as_ref() == Some(&root)) {
            return;
        }
        self.git_wanted = Some(root.clone());
        self.git_pending = true;
        if self.git_tx.send(root).is_err() {
            self.error("git worker stopped");
        }
    }

    /// Takes finished git status. Returns whether anything visible changed.
    pub fn poll_git(&mut self) -> bool {
        let mut changed = false;
        while let Ok(resp) = self.git_rx.try_recv() {
            // An answer for a repository already left behind.
            if Some(&resp.root) != self.git_wanted.as_ref() {
                continue;
            }
            self.git_pending = false;
            changed = true;
            match resp.repo {
                Ok(repo) => self.git = Some(repo),
                // No git installed: browse without status.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => self.error(format!("git: {e}")),
            }
        }
        changed
    }

    /// Recomputes the visible entries from the filter and puts the cursor on `select`, else the top.
    fn rebuild_view(&mut self, select: Option<&str>) {
        self.view = if self.filter.is_empty() {
            (0..self.entries.len())
                .map(|idx| ViewItem {
                    idx,
                    hits: Vec::new(),
                })
                .collect()
        } else {
            let mut scored: Vec<(i64, ViewItem)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, e)| {
                    fuzzy::score(&self.filter, &e.name).map(|(s, hits)| (s, ViewItem { idx, hits }))
                })
                .collect();
            scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
            scored.into_iter().map(|(_, v)| v).collect()
        };
        self.selected = select
            .and_then(|name| {
                self.view
                    .iter()
                    .position(|v| self.entries[v.idx].name == name)
            })
            .unwrap_or(0);
        self.clamp();
    }

    fn clamp(&mut self) {
        self.selected = self.selected.min(self.view.len().saturating_sub(1));
    }

    /// Re-reads the current directory, keeping the cursor on the same name where it still exists.
    pub fn reload(&mut self) {
        let keep = self.selected().map(|e| e.name.clone());
        self.reload_selecting(keep);
    }

    fn reload_selecting(&mut self, select: Option<String>) {
        // Contents may have changed under the same path, so ask for a fresh preview.
        self.requested = None;
        let cwd = self.cwd.clone();
        let mut failure = None;
        // If the directory was removed from under us, fall back to the nearest one that exists.
        for (i, dir) in cwd.ancestors().enumerate() {
            let select = if i == 0 { select.clone() } else { None };
            match self.load(dir.to_path_buf(), select) {
                Ok(()) => break,
                Err(e) => {
                    failure.get_or_insert(e);
                }
            }
        }
        if let Some(e) = failure {
            self.error(format!("{}: {e}", cwd.display()));
        }
    }

    /// Whether git has a diff to show for `entry`: a tracked file with changes.
    pub fn has_changes(&self, entry: &Entry) -> bool {
        use git::Status::{Added, Conflicted, Modified, Renamed};
        !entry.is_dir
            && self
                .git
                .as_ref()
                .and_then(|r| r.status_of(&entry.path))
                .is_some_and(|s| matches!(s, Modified | Added | Renamed | Conflicted))
    }

    /// Asks the worker for the selected entry's preview if it is not already the one requested.
    pub fn sync_preview(&mut self) {
        let want = self.selected().map(|e| preview::Key {
            path: e.path.clone(),
            diff: self.diff && self.has_changes(e),
        });
        if want == self.requested {
            return;
        }
        self.requested = want.clone();
        self.preview_scroll = 0;
        if let Some(key) = want {
            let req = preview::Request {
                key,
                show_hidden: self.show_hidden,
            };
            if self.preview_tx.send(req).is_err() {
                self.error("preview worker stopped");
            }
        }
    }

    /// Whether a worker owes us an answer, so the event loop should poll it soon.
    pub fn pending(&self) -> bool {
        let preview = self.requested.is_some()
            && self.preview.as_ref().map(|(k, _)| k) != self.requested.as_ref();
        let indexing = matches!(&self.mode, Mode::Find(f) if f.indexing());
        preview || (self.git_pending && self.git_wanted.is_some()) || self.job.is_some() || indexing
    }

    /// Takes finished previews. Returns whether one worth showing arrived.
    pub fn poll_preview(&mut self) -> bool {
        let mut fresh = false;
        while let Ok(resp) = self.preview_rx.try_recv() {
            if Some(&resp.key) == self.requested.as_ref() {
                self.preview = Some((resp.key, resp.preview));
                fresh = true;
            }
        }
        fresh
    }

    /// Periodic work while idle. Returns whether anything visible changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if self
            .message
            .as_ref()
            .is_some_and(|m| m.at.elapsed() >= MESSAGE_TTL)
        {
            self.message = None;
            changed = true;
        }
        if dir::mtime(&self.cwd) != self.cwd_mtime {
            self.reload();
            self.sync_git(true);
            changed = true;
        }
        changed
    }

    /// After an editor or shell returns: whatever ran may have changed files, contents, or the index.
    pub fn after_run(&mut self, status: io::Result<ExitStatus>) {
        let bulk = self.bulk.take();
        match (&status, bulk) {
            (Ok(status), Some(bulk)) => self.finish_bulk_rename(bulk, *status),
            (Err(_), Some(bulk)) => {
                // The editor never ran, so the list was never read; it is only a scratch file.
                let _ = std::fs::remove_file(&bulk.list);
            }
            (_, None) => {}
        }
        self.reload();
        self.sync_git(true);
        if let Err(e) = status {
            self.error(format!("could not start: {e}"));
        }
    }

    /// Writes the marked names to a list, one per line, for the editor to change.
    fn bulk_rename(&mut self) -> Option<Effect> {
        let originals: Vec<PathBuf> = self.marked.iter().cloned().collect();
        // Unique per rename, not just per process: two renames must never share a list.
        static RENAMES: AtomicUsize = AtomicUsize::new(0);
        let n = RENAMES.fetch_add(1, Ordering::Relaxed);
        let list = env::temp_dir().join(format!("findr-rename-{}-{n}.txt", std::process::id()));
        let names: String = originals
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| format!("{}\n", n.to_string_lossy()))
            .collect();
        if let Err(e) = std::fs::write(&list, names) {
            self.error(format!("{}: {e}", list.display()));
            return None;
        }
        self.bulk = Some(BulkRename {
            list: list.clone(),
            originals,
        });
        Some(self.editor(&[list]))
    }

    fn finish_bulk_rename(&mut self, bulk: BulkRename, status: ExitStatus) {
        let edited = std::fs::read_to_string(&bulk.list);
        // The list was a scratch file; if it cannot be removed, a small file stays in the temp dir.
        let _ = std::fs::remove_file(&bulk.list);
        if !status.success() {
            return self.error("the editor exited with an error; nothing renamed");
        }
        let edited = match edited {
            Ok(text) => text,
            Err(e) => return self.error(format!("{}: {e}", bulk.list.display())),
        };
        match ops::plan_renames(&bulk.originals, &edited) {
            Err(e) => self.error(e),
            Ok(plan) if plan.is_empty() => self.info("no names changed"),
            Ok(plan) => match ops::apply_renames(&plan) {
                Ok(n) => {
                    // The marks named the old paths.
                    self.marked.clear();
                    self.undo.push(Undo::Rename(plan));
                    self.success(format!("renamed {} · u to undo", count(n)));
                }
                Err(e) => self.error(e.to_string()),
            },
        }
    }

    /// Marked paths if any, otherwise the one under the cursor.
    fn targets(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.selected()
                .map(|e| e.path.clone())
                .into_iter()
                .collect()
        } else {
            self.marked.iter().cloned().collect()
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Effect::Quit { write_cwd: false });
        }
        // Nothing is bound to Alt, and treating Alt-x as x would act on a key the user did not mean.
        if key.modifiers.contains(KeyModifiers::ALT) {
            return None;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => return self.normal_key(key),
            Mode::Filter => self.filter_key(key),
            Mode::Input(input) => self.input_key(input, key),
            Mode::Confirm(confirm) => return self.confirm_key(confirm, key),
            Mode::Goto => self.goto_key(key),
            Mode::Find(finder) => self.find_key(finder, key),
            Mode::Help => {}
        }
        None
    }

    /// Wheel scrolls the list (or the preview under the pointer), a click selects, a double
    /// click opens. The parent column and a directory preview navigate on a single click,
    /// since what they show is somewhere to go rather than something to select.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<Effect> {
        if !matches!(self.mode, Mode::Normal) {
            return None;
        }
        let at = Position::new(mouse.column, mouse.row);
        let row = |area: Rect| (mouse.row - area.y) as usize;
        let areas = self.areas;
        match mouse.kind {
            MouseEventKind::ScrollDown if areas.preview.contains(at) => {
                self.scroll_preview(WHEEL_STEP)
            }
            MouseEventKind::ScrollUp if areas.preview.contains(at) => {
                self.scroll_preview(-WHEEL_STEP)
            }
            MouseEventKind::ScrollDown => self.move_by(WHEEL_STEP),
            MouseEventKind::ScrollUp => self.move_by(-WHEEL_STEP),
            MouseEventKind::Down(MouseButton::Left) if areas.current.contains(at) => {
                let index = self.offset + row(areas.current);
                let path = self
                    .view
                    .get(index)
                    .map(|v| self.entries[v.idx].path.clone())?;
                let now = Instant::now();
                let double = self
                    .last_click
                    .take()
                    .is_some_and(|(then, last)| last == path && now - then < DOUBLE_CLICK);
                self.selected = index;
                if double {
                    return self.open();
                }
                self.last_click = Some((now, path));
            }
            MouseEventKind::Down(MouseButton::Left) if areas.parent.contains(at) => {
                let height = areas.parent.height as usize;
                let selected = self.parent_selected.unwrap_or(0);
                let offset = parent_offset(selected, self.parent.len(), height);
                let entry = self.parent.get(offset + row(areas.parent))?;
                let (path, name, is_dir) = (entry.path.clone(), entry.name.clone(), entry.is_dir);
                if is_dir {
                    self.enter(path, None);
                } else if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
                    self.enter(parent, Some(name));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if areas.preview.contains(at) => {
                let Some((key, Preview::Dir(entries))) = &self.preview else {
                    return None;
                };
                let entry = entries.get(self.preview_scroll + row(areas.preview))?;
                let (dir, name) = (key.path.clone(), entry.name.clone());
                self.enter(dir, Some(name));
            }
            _ => {}
        }
        None
    }

    fn normal_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.page / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.move_by(half),
            KeyCode::Char('p') if ctrl => self.open_finder(),
            KeyCode::Char('f') => self.open_finder(),
            KeyCode::Char('u') if ctrl => self.move_by(-half),
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(self.page as isize),
            KeyCode::PageUp => self.move_by(-(self.page as isize)),
            KeyCode::Home => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Char('g') => self.mode = Mode::Goto,
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.up(),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => return self.open(),
            KeyCode::Char('e') => return self.edit(),
            KeyCode::Char('o') => self.open_external(),
            KeyCode::Char('!') => return Some(self.shell()),
            KeyCode::Char('-') => self.go_back(),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char(':') => self.mode = Mode::Input(Input::new(InputKind::Jump, "", 0)),
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.reload();
                self.info(if self.show_hidden {
                    "showing hidden files"
                } else {
                    "hiding hidden files"
                });
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.resort();
            }
            KeyCode::Char('S') => {
                self.reverse = !self.reverse;
                self.resort();
            }
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('y') => self.yank(false),
            KeyCode::Char('x') => self.yank(true),
            KeyCode::Char('p') => self.paste(),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('d') => {
                let targets = self.targets();
                if !targets.is_empty() {
                    self.mode = Mode::Confirm(Confirm::Trash(targets));
                }
            }
            // With marks, rename them all at once in the editor.
            KeyCode::Char('r') if !self.marked.is_empty() => return self.bulk_rename(),
            KeyCode::Char('r') => {
                if let Some(e) = self.selected() {
                    let stem = dir::split_ext(&e.name).0.chars().count();
                    let cursor = if e.is_dir {
                        e.name.chars().count()
                    } else {
                        stem
                    };
                    self.mode = Mode::Input(Input::new(
                        InputKind::Rename(e.path.clone()),
                        &e.name,
                        cursor,
                    ));
                }
            }
            KeyCode::Char('a') => self.mode = Mode::Input(Input::new(InputKind::NewFile, "", 0)),
            KeyCode::Char('A') => self.mode = Mode::Input(Input::new(InputKind::NewDir, "", 0)),
            KeyCode::Char('c') => self.copy_paths(),
            KeyCode::Char('R') => {
                self.reload();
                self.sync_git(true);
                self.info("reloaded");
            }
            KeyCode::Char('D') => {
                self.diff = !self.diff;
                let changed = self.selected().is_some_and(|e| self.has_changes(e));
                match (self.diff, changed) {
                    (true, false) => self.info("diffs on: shown for changed files"),
                    (false, false) => self.info("diffs off"),
                    _ => {}
                }
            }
            KeyCode::Char('J') => self.scroll_preview(PREVIEW_STEP as isize),
            KeyCode::Char('K') => self.scroll_preview(-(PREVIEW_STEP as isize)),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('q') => return self.quit(true),
            KeyCode::Char('Q') => return self.quit(false),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            KeyCode::Esc => self.marked.clear(),
            _ => {}
        }
        None
    }

    fn filter_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return self.clear_filter(),
            KeyCode::Enter => return,
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Char('n') if ctrl => self.move_by(1),
            KeyCode::Char('p') if ctrl => self.move_by(-1),
            KeyCode::Char('u') if ctrl => self.set_filter(String::new()),
            KeyCode::Char('w') if ctrl => {
                let keep = self.filter[..word_start(&self.filter)].to_string();
                self.set_filter(keep);
            }
            // Backspace on an empty filter leaves filtering, as in a shell prompt.
            KeyCode::Backspace if self.filter.is_empty() => return,
            KeyCode::Backspace => {
                let mut f = self.filter.clone();
                f.pop();
                self.set_filter(f);
            }
            KeyCode::Char(c) if !ctrl => {
                let f = format!("{}{c}", self.filter);
                self.set_filter(f);
            }
            _ => {}
        }
        self.mode = Mode::Filter;
    }

    fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.offset = 0;
        self.rebuild_view(None);
    }

    fn clear_filter(&mut self) {
        let keep = self.selected().map(|e| e.name.clone());
        self.filter.clear();
        self.rebuild_view(keep.as_deref());
    }

    fn input_key(&mut self, mut input: Input, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => self.submit(input),
            _ => {
                input.edit(key);
                self.mode = Mode::Input(input);
            }
        }
    }

    fn submit(&mut self, input: Input) {
        let text = input.text.trim();
        if text.is_empty() {
            return;
        }
        let (created, done, undo) = match input.kind {
            InputKind::Rename(from) => {
                let to = from.with_file_name(text);
                (
                    ops::rename(&from, text).map(|()| text.to_string()),
                    format!("renamed to {text}"),
                    Undo::Rename(vec![(from, to)]),
                )
            }
            InputKind::NewFile => (
                ops::create(&self.cwd, text, false),
                format!("created {text}"),
                Undo::Create(vec![self.cwd.join(text)]),
            ),
            InputKind::NewDir => (
                ops::create(&self.cwd, text, true),
                format!("created {text}/"),
                Undo::Create(vec![self.cwd.join(text)]),
            ),
            InputKind::Jump => return self.jump(text),
        };
        match created {
            Ok(name) => {
                self.undo.push(undo);
                self.reload_selecting(Some(name));
                self.sync_git(true);
                self.success(done);
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    fn jump(&mut self, text: &str) {
        let expanded = match (text.strip_prefix('~'), env::var_os("HOME")) {
            (Some(rest), Some(home)) => format!("{}{rest}", home.to_string_lossy()),
            _ => text.to_string(),
        };
        let path = self.cwd.join(expanded);
        if let Err(e) = self.reveal(&path) {
            self.error(format!("{text}: {e}"));
        }
    }

    /// Goes to `path`: into it if it is a directory, else to its directory with the cursor on it.
    fn reveal(&mut self, path: &Path) -> io::Result<()> {
        let path = std::fs::canonicalize(path)?;
        if path.is_dir() {
            self.enter(path, None);
        } else {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let parent = path.parent().unwrap_or(&path).to_path_buf();
            self.enter(parent, name);
        }
        Ok(())
    }

    fn open_finder(&mut self) {
        let cached = self
            .find_cache
            .iter()
            .find(|(hidden, index)| *hidden == self.show_hidden && index.root == self.cwd)
            .map(|(_, index)| Arc::clone(index));
        self.mode = Mode::Find(Finder::open(self.cwd.clone(), self.show_hidden, cached));
    }

    fn find_key(&mut self, mut finder: Finder, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                if let Some(path) = finder.chosen()
                    && let Err(e) = self.reveal(&path)
                {
                    self.error(format!("{}: {e}", path.display()));
                }
                return;
            }
            KeyCode::Down => finder.move_by(1),
            KeyCode::Up => finder.move_by(-1),
            KeyCode::PageDown => finder.move_by(self.page as isize),
            KeyCode::PageUp => finder.move_by(-(self.page as isize)),
            KeyCode::Char('n' | 'j') if ctrl => finder.move_by(1),
            KeyCode::Char('p' | 'k') if ctrl => finder.move_by(-1),
            KeyCode::Char('u') if ctrl => finder.set_query(String::new()),
            KeyCode::Char('w') if ctrl => {
                let keep = finder.query[..word_start(&finder.query)].to_string();
                finder.set_query(keep);
            }
            KeyCode::Backspace => {
                let mut query = finder.query.clone();
                query.pop();
                finder.set_query(query);
            }
            KeyCode::Char(c) if !ctrl => {
                let query = format!("{}{c}", finder.query);
                finder.set_query(query);
            }
            _ => {}
        }
        self.mode = Mode::Find(finder);
    }

    /// Takes the finder's index once the worker has built it. Returns whether it arrived.
    pub fn poll_find(&mut self) -> bool {
        let Mode::Find(finder) = &mut self.mode else {
            return false;
        };
        let Some(rx) = &finder.rx else {
            return false;
        };
        let failure = match rx.try_recv() {
            Ok(Ok(index)) => {
                let index = Arc::new(index);
                finder.rx = None;
                finder.replace_index(Arc::clone(&index));
                let key = (self.show_hidden, index);
                self.find_cache
                    .retain(|(hidden, old)| (*hidden, &old.root) != (key.0, &key.1.root));
                self.find_cache.insert(0, key);
                self.find_cache.truncate(FIND_CACHE);
                return true;
            }
            Ok(Err(e)) => format!("find: {e}"),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => "find: the index stopped unexpectedly".into(),
        };
        self.mode = Mode::Normal;
        self.error(failure);
        true
    }

    fn confirm_key(&mut self, confirm: Confirm, key: KeyEvent) -> Option<Effect> {
        if !matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.info("cancelled");
            return None;
        }
        match confirm {
            Confirm::Trash(paths) => self.trash(paths),
            Confirm::Quit { write_cwd } => return Some(Effect::Quit { write_cwd }),
        }
        None
    }

    fn quit(&mut self, write_cwd: bool) -> Option<Effect> {
        if self.job.is_some() {
            self.mode = Mode::Confirm(Confirm::Quit { write_cwd });
            return None;
        }
        Some(Effect::Quit { write_cwd })
    }

    fn goto_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('g') => self.move_to(0),
            KeyCode::Char('h' | '~') => match env::var_os("HOME") {
                Some(home) => self.enter(home.into(), None),
                None => self.error("HOME is not set"),
            },
            KeyCode::Char('r') => match self.git.as_ref().map(|r| r.root.clone()) {
                Some(root) => self.enter(root, None),
                None => self.error("not in a git repository"),
            },
            KeyCode::Char('/') => self.enter(PathBuf::from("/"), None),
            _ => {}
        }
    }

    fn move_by(&mut self, delta: isize) {
        let next = self.selected.saturating_add_signed(delta);
        self.move_to(next);
    }

    fn move_to(&mut self, index: usize) {
        self.selected = index;
        self.clamp();
    }

    fn scroll_preview(&mut self, delta: isize) {
        let max = match &self.preview {
            Some((_, Preview::Text { lines, .. })) => lines.len().saturating_sub(1),
            Some((_, Preview::Diff { lines, .. })) => lines.len().saturating_sub(1),
            Some((_, Preview::Dir(entries))) => entries.len().saturating_sub(1),
            _ => 0,
        };
        self.preview_scroll = self.preview_scroll.saturating_add_signed(delta).min(max);
    }

    fn enter(&mut self, dir: PathBuf, select: Option<String>) {
        self.go(dir, select, true);
    }

    fn go(&mut self, dir: PathBuf, select: Option<String>, record: bool) {
        let from = self.cwd.clone();
        match std::fs::canonicalize(&dir).and_then(|d| self.load(d, select)) {
            Ok(()) if record && from != self.cwd => self.back.push(from),
            Ok(()) => {}
            Err(e) => self.error(format!("{}: {e}", dir.display())),
        }
    }

    fn up(&mut self) {
        if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
            let name = self
                .cwd
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            self.enter(parent, name);
        }
    }

    fn go_back(&mut self) {
        match self.back.pop() {
            Some(dir) => self.go(dir, None, false),
            None => self.info("no previous directory"),
        }
    }

    fn open(&mut self) -> Option<Effect> {
        let entry = self.selected()?;
        if entry.is_dir {
            let path = entry.path.clone();
            self.enter(path, None);
            None
        } else {
            self.edit()
        }
    }

    fn edit(&mut self) -> Option<Effect> {
        let files: Vec<PathBuf> = self.targets().into_iter().filter(|p| !p.is_dir()).collect();
        if files.is_empty() {
            return None;
        }
        Some(self.editor(&files))
    }

    fn editor(&self, files: &[PathBuf]) -> Effect {
        let editor = env::var("VISUAL")
            .or_else(|_| env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".into());
        let mut cmd = Command::new("sh");
        // Through sh so an editor set as `code -w` or with quoted arguments works as in a shell.
        cmd.arg("-c")
            .arg(format!("{editor} \"$@\""))
            .arg("sh")
            .args(files)
            .current_dir(&self.cwd);
        Effect::Run(cmd)
    }

    fn shell(&self) -> Effect {
        let shell = env::var_os("SHELL").unwrap_or_else(|| "sh".into());
        let mut cmd = Command::new(shell);
        cmd.current_dir(&self.cwd);
        Effect::Run(cmd)
    }

    fn open_external(&mut self) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        for path in self.targets() {
            // Null stdio: a launched app inheriting our pipes would otherwise keep us waiting.
            let status = Command::new(opener)
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => return self.error(format!("{opener} {}: {s}", path.display())),
                Err(e) => return self.error(format!("{opener}: {e}")),
            }
        }
    }

    fn copy_paths(&mut self) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let text: Vec<_> = paths.iter().map(|p| p.to_string_lossy()).collect();
        match ops::copy_to_clipboard(&text.join("\n")) {
            Ok(()) if paths.len() == 1 => self.success(format!("copied {}", text[0])),
            Ok(()) => self.success(format!("copied {} paths", paths.len())),
            Err(e) => self.error(e.to_string()),
        }
    }

    fn resort(&mut self) {
        let keep = self.selected().map(|e| e.name.clone());
        dir::sort(&mut self.entries, self.sort, self.reverse);
        dir::sort(&mut self.parent, self.sort, self.reverse);
        self.parent_selected = self.parent.iter().position(|e| e.path == self.cwd);
        self.rebuild_view(keep.as_deref());
        let order = if self.reverse { "reversed" } else { "" };
        self.info(
            format!("sort by {} {order}", self.sort.label())
                .trim_end()
                .to_string(),
        );
    }

    fn toggle_mark(&mut self) {
        if let Some(path) = self.selected().map(|e| e.path.clone()) {
            if !self.marked.remove(&path) {
                self.marked.insert(path);
            }
            self.move_by(1);
        }
    }

    fn yank(&mut self, cut: bool) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let verb = if cut { "cut" } else { "copied" };
        self.info(format!("{verb} {} — p to paste", count(paths.len())));
        self.clip = Some(Clip { paths, cut });
        self.marked.clear();
    }

    /// Starts copying or moving the clipboard here on a worker; `poll_job` sees it finish.
    fn paste(&mut self) {
        if self.job.is_some() {
            return self.info("a paste is already running");
        }
        let Some(clip) = self.clip.take() else {
            return self.info("nothing to paste");
        };
        let (tx, rx) = mpsc::channel();
        let (paths, dest, cut) = (clip.paths.clone(), self.cwd.clone(), clip.cut);
        thread::spawn(move || {
            let pasted = ops::paste(&paths, &dest, cut, |n| {
                // Sends fail only once findr has quit, and then nobody is left to tell.
                let _ = tx.send(JobEvent::Progress(n));
            });
            let _ = tx.send(JobEvent::Finished(pasted));
        });
        self.job = Some(Job {
            dest: self.cwd.clone(),
            cut: clip.cut,
            total: clip.paths.len(),
            done: 0,
            rx,
        });
        // A copy can be pasted again; a cut is spent once its files have moved.
        if !clip.cut {
            self.clip = Some(clip);
        }
    }

    /// Takes progress from a running paste. Returns whether anything visible changed.
    pub fn poll_job(&mut self) -> bool {
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut finished = None;
        let mut vanished = false;
        loop {
            match job.rx.try_recv() {
                Ok(JobEvent::Progress(n)) => job.done = n,
                Ok(JobEvent::Finished(pasted)) => finished = Some(pasted),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    vanished = finished.is_none();
                    break;
                }
            }
            changed = true;
        }
        let (dest, total, cut) = (job.dest.clone(), job.total, job.cut);
        if let Some(pasted) = finished {
            self.job = None;
            self.finish_paste(&dest, total, cut, pasted);
            return true;
        }
        if vanished {
            self.job = None;
            self.reload();
            self.error("the paste stopped unexpectedly; check what arrived");
            return true;
        }
        changed
    }

    fn finish_paste(&mut self, dest: &Path, total: usize, cut: bool, pasted: ops::Pasted) {
        if !pasted.moved.is_empty() {
            self.undo.push(if cut {
                Undo::Move(pasted.moved.clone())
            } else {
                Undo::Create(pasted.moved.iter().map(|(_, to)| to.clone()).collect())
            });
        }
        // The cursor follows the pasted item only if we are still looking at where it went.
        let select = pasted.last.filter(|_| dest == self.cwd);
        self.reload_selecting(select.or_else(|| self.selected().map(|e| e.name.clone())));
        self.sync_git(true);
        let done = pasted.done;
        match pasted.failure {
            Some(f) => self.error(format!("pasted {done} of {total}: {f}")),
            None => self.success(format!("pasted {} · u to undo", count(done))),
        }
    }

    fn trash(&mut self, paths: Vec<PathBuf>) {
        let Some(trash) = self.trash.clone() else {
            return self.error("no trash directory: HOME is not set");
        };
        let mut batch = Vec::new();
        let mut failure = None;
        for path in &paths {
            match trash.put(path) {
                Ok(trashed) => {
                    batch.push((path.clone(), trashed));
                    self.marked.remove(path);
                }
                Err(e) => {
                    failure = Some(format!("{}: {e}", path.display()));
                    break;
                }
            }
        }
        let done = batch.len();
        if !batch.is_empty() {
            self.undo.push(Undo::Trash(batch));
        }
        // The item under the cursor is gone; stay at the same row rather than jumping to the top.
        let row = self.selected;
        self.reload();
        self.move_to(row);
        self.sync_git(true);
        match failure {
            Some(f) => self.error(format!("trashed {done} of {}: {f}", paths.len())),
            None => self.success(format!("moved {} to the trash · u to undo", count(done))),
        }
    }

    /// Takes back the most recent trash, rename, paste or creation.
    fn undo(&mut self) {
        let Some(action) = self.undo.pop() else {
            return self.info("nothing to undo");
        };
        let outcome = match action {
            Undo::Trash(batch) => self.undo_trash(&batch),
            Undo::Rename(plan) => ops::reverse_renames(&plan)
                .map(|n| {
                    (
                        format!("renamed {} back", count(n)),
                        plan.first().map(|(old, _)| old.clone()),
                    )
                })
                .map_err(|e| e.to_string()),
            Undo::Move(moved) => {
                let mut back = None;
                let mut result = Ok(());
                for (from, to) in &moved {
                    let (Some(dir), Some(name)) = (from.parent(), from.file_name()) else {
                        continue;
                    };
                    // Never over something that has taken the old name since.
                    let dest = ops::unique_dest(dir, &name.to_string_lossy());
                    if let Err(e) = ops::move_path(to, &dest) {
                        result = Err(format!("{}: {e}", to.display()));
                        break;
                    }
                    back.get_or_insert(dest);
                }
                result.map(|()| (format!("moved {} back", count(moved.len())), back))
            }
            Undo::Create(paths) => match self.trash.clone() {
                None => Err("no trash directory: HOME is not set".into()),
                Some(trash) => paths
                    .iter()
                    .try_for_each(|p| {
                        trash
                            .put(p)
                            .map(drop)
                            .map_err(|e| format!("{}: {e}", p.display()))
                    })
                    .map(|()| (format!("moved {} to the trash", count(paths.len())), None)),
            },
        };
        match outcome {
            Ok((message, select)) => {
                let select = select
                    .filter(|p| p.parent() == Some(self.cwd.as_path()))
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                self.reload_selecting(select.or_else(|| self.selected().map(|e| e.name.clone())));
                self.sync_git(true);
                self.success(message);
            }
            Err(e) => {
                self.reload();
                self.error(format!("undo: {e}"));
            }
        }
    }

    fn undo_trash(
        &self,
        batch: &[(PathBuf, PathBuf)],
    ) -> Result<(String, Option<PathBuf>), String> {
        let trash = self
            .trash
            .clone()
            .ok_or("no trash directory: HOME is not set")?;
        let mut first = None;
        for (done, (original, trashed)) in batch.iter().enumerate() {
            match trash.restore(trashed, original) {
                Ok(back) => {
                    first.get_or_insert(back);
                }
                Err(e) => {
                    return Err(format!(
                        "restored {done} of {}: {}: {e}",
                        batch.len(),
                        original.display()
                    ));
                }
            }
        }
        Ok((format!("restored {}", count(batch.len())), first))
    }
}

pub(crate) fn count(n: usize) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;
    use ratatui::text::Line;
    use std::fs;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\x1b' => KeyCode::Esc,
                c => KeyCode::Char(c),
            };
            assert!(
                app.handle_key(key(code)).is_none(),
                "{c:?} should not need the terminal"
            );
        }
    }

    fn names(app: &App) -> Vec<&str> {
        app.view
            .iter()
            .map(|v| app.entries[v.idx].name.as_str())
            .collect()
    }

    fn setup() -> (TempDir, App) {
        let tmp = TempDir::new();
        tmp.file("src/main.rs", "fn main() {}\n");
        tmp.file("src/app.rs", "");
        tmp.file("README.md", "# hi\n");
        tmp.file("Cargo.toml", "");
        tmp.file(".env", "");
        let app = App::new(tmp.path(), Some(Trash::Dir(tmp.path().join(".trash")))).unwrap();
        (tmp, app)
    }

    #[test]
    fn lists_directories_first_and_hides_dotfiles() {
        let (_tmp, app) = setup();
        assert_eq!(names(&app), ["src", "Cargo.toml", "README.md"]);
        assert_eq!(app.selected().unwrap().name, "src");
    }

    #[test]
    fn opening_a_file_path_selects_it() {
        let (tmp, _) = setup();
        let app = App::new(&tmp.path().join("README.md"), None).unwrap();
        assert_eq!(app.cwd, tmp.path());
        assert_eq!(app.selected().unwrap().name, "README.md");
    }

    #[test]
    fn enter_and_leave_remember_position() {
        let (tmp, mut app) = setup();
        press(&mut app, "l");
        assert_eq!(app.cwd, tmp.path().join("src"));
        press(&mut app, "j");
        assert_eq!(app.selected().unwrap().name, "main.rs");
        press(&mut app, "h");
        assert_eq!(app.cwd, tmp.path());
        assert_eq!(app.selected().unwrap().name, "src");
        press(&mut app, "l");
        assert_eq!(app.selected().unwrap().name, "main.rs");
        press(&mut app, "-");
        assert_eq!(app.cwd, tmp.path());
    }

    #[test]
    fn fuzzy_filter_narrows_and_esc_restores() {
        let (_tmp, mut app) = setup();
        press(&mut app, "/rdm");
        assert!(matches!(app.mode, Mode::Filter));
        assert_eq!(names(&app), ["README.md"]);
        press(&mut app, "\n");
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(names(&app), ["README.md"]);
        press(&mut app, "\x1b");
        assert_eq!(names(&app).len(), 3);
        assert_eq!(app.selected().unwrap().name, "README.md");
    }

    #[test]
    fn toggles_hidden_files() {
        let (_tmp, mut app) = setup();
        press(&mut app, ".");
        assert!(names(&app).contains(&".env"));
    }

    #[test]
    fn creates_files_and_directories() {
        let (tmp, mut app) = setup();
        press(&mut app, "adocs/notes.md\n");
        assert!(tmp.path().join("docs/notes.md").is_file());
        assert_eq!(app.selected().unwrap().name, "docs");
        press(&mut app, "Abuild\n");
        assert!(tmp.path().join("build").is_dir());
        press(&mut app, "aREADME.md\n");
        assert_eq!(app.message().unwrap().kind, MessageKind::Error);
    }

    #[test]
    fn rename_starts_with_cursor_before_extension() {
        let (tmp, mut app) = setup();
        press(&mut app, "G");
        assert_eq!(app.selected().unwrap().name, "README.md");
        press(&mut app, "r");
        let Mode::Input(input) = &app.mode else {
            panic!("expected input")
        };
        assert_eq!(input.cursor, "README".len());
        press(&mut app, "-old\n");
        assert!(tmp.path().join("README-old.md").exists());
        assert_eq!(app.selected().unwrap().name, "README-old.md");
    }

    #[test]
    fn copy_paste_and_cut_paste() {
        let (tmp, mut app) = setup();
        press(&mut app, "Gyp");
        assert!(app.job.is_some(), "a paste runs off the event loop");
        wait(&mut app, |a| a.job.is_none());
        assert!(tmp.path().join("README copy.md").exists());
        assert_eq!(app.selected().unwrap().name, "README copy.md");
        press(&mut app, "k");
        assert_eq!(app.selected().unwrap().name, "Cargo.toml");
        press(&mut app, "xggl");
        press(&mut app, "p");
        assert!(app.clip.is_none(), "a cut is spent once pasted");
        wait(&mut app, |a| a.job.is_none());
        assert!(tmp.path().join("src/Cargo.toml").exists());
        assert!(!tmp.path().join("Cargo.toml").exists());
        assert!(app.clip.is_none());
    }

    #[test]
    fn quitting_mid_paste_asks_first() {
        let (_tmp, mut app) = setup();
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            dest: app.cwd.clone(),
            cut: false,
            total: 1,
            done: 0,
            rx,
        });
        assert!(app.handle_key(key(KeyCode::Char('q'))).is_none());
        assert!(matches!(
            app.mode,
            Mode::Confirm(Confirm::Quit { write_cwd: true })
        ));
        press(&mut app, "n");
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.handle_key(key(KeyCode::Char('q'))).is_none());
        let quit = app.handle_key(key(KeyCode::Char('y')));
        assert!(matches!(quit, Some(Effect::Quit { write_cwd: true })));
    }

    #[test]
    fn a_second_paste_waits_for_the_first() {
        let (_tmp, mut app) = setup();
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            dest: app.cwd.clone(),
            cut: false,
            total: 1,
            done: 0,
            rx,
        });
        press(&mut app, "Gyp");
        assert_eq!(app.message().unwrap().text, "a paste is already running");
    }

    #[test]
    fn pasting_a_directory_into_itself_is_refused() {
        let (tmp, mut app) = setup();
        press(&mut app, "ylp");
        wait(&mut app, |a| a.job.is_none());
        assert_eq!(app.message().unwrap().kind, MessageKind::Error);
        assert!(!tmp.path().join("src/src").exists());
    }

    #[test]
    fn marks_then_trash_after_confirmation() {
        let (tmp, mut app) = setup();
        press(&mut app, "j  ");
        assert_eq!(app.marked.len(), 2);
        press(&mut app, "dn");
        assert!(tmp.path().join("Cargo.toml").exists());
        press(&mut app, "dy");
        assert!(!tmp.path().join("Cargo.toml").exists());
        assert!(!tmp.path().join("README.md").exists());
        assert!(tmp.path().join(".trash/README.md").exists());
        assert!(app.marked.is_empty());
        assert_eq!(names(&app), ["src"]);
    }

    #[test]
    fn u_undoes_trash_one_batch_at_a_time() {
        let (tmp, mut app) = setup();
        press(&mut app, "j");
        press(&mut app, "dy");
        press(&mut app, "dy");
        assert!(!tmp.path().join("Cargo.toml").exists());
        assert!(!tmp.path().join("README.md").exists());
        assert!(app.message().unwrap().text.ends_with("u to undo"));

        press(&mut app, "u");
        assert!(
            tmp.path().join("README.md").exists(),
            "the last batch comes back first"
        );
        assert!(!tmp.path().join("Cargo.toml").exists());
        assert_eq!(app.selected().unwrap().name, "README.md");
        press(&mut app, "u");
        assert!(tmp.path().join("Cargo.toml").exists());
        press(&mut app, "u");
        assert_eq!(app.message().unwrap().text, "nothing to undo");
    }

    #[test]
    fn u_takes_back_a_rename() {
        let (tmp, mut app) = setup();
        press(&mut app, "G");
        press(&mut app, "r");
        let Mode::Input(input) = &mut app.mode else {
            panic!("expected the prompt")
        };
        input.text = "NOTES.md".into();
        press(&mut app, "\n");
        assert!(tmp.path().join("NOTES.md").exists());
        press(&mut app, "u");
        assert!(tmp.path().join("README.md").exists());
        assert!(!tmp.path().join("NOTES.md").exists());
        assert_eq!(app.selected().unwrap().name, "README.md");
    }

    #[test]
    fn u_refuses_a_rename_back_onto_a_new_file() {
        let (tmp, mut app) = setup();
        press(&mut app, "G");
        press(&mut app, "r");
        let Mode::Input(input) = &mut app.mode else {
            panic!("expected the prompt")
        };
        input.text = "OLD.md".into();
        press(&mut app, "\n");
        tmp.file("README.md", "a new readme");
        press(&mut app, "u");
        assert_eq!(app.message().unwrap().kind, MessageKind::Error);
        assert!(tmp.path().join("OLD.md").exists(), "nothing moved");
        assert_eq!(
            fs::read_to_string(tmp.path().join("README.md")).unwrap(),
            "a new readme"
        );
    }

    #[test]
    fn u_moves_a_cut_back_and_trashes_a_copy() {
        let (tmp, mut app) = setup();
        press(&mut app, "Gx");
        press(&mut app, "ggl");
        press(&mut app, "p");
        wait(&mut app, |a| a.job.is_none());
        assert!(tmp.path().join("src/README.md").exists());
        press(&mut app, "u");
        assert!(tmp.path().join("README.md").exists(), "moved back");
        assert!(!tmp.path().join("src/README.md").exists());

        // Up to the top, where the cursor lands on src, and copy it beside itself.
        press(&mut app, "hy");
        press(&mut app, "p");
        wait(&mut app, |a| a.job.is_none());
        let copy = tmp.path().join("src copy");
        assert!(copy.exists());
        press(&mut app, "u");
        assert!(!copy.exists(), "the copy went to the trash");
        assert!(tmp.path().join(".trash/src copy").exists());
        assert!(
            tmp.path().join("src/main.rs").exists(),
            "the original is untouched"
        );
    }

    #[test]
    fn u_trashes_what_was_created() {
        let (tmp, mut app) = setup();
        press(&mut app, "adraft.md\n");
        assert!(tmp.path().join("draft.md").exists());
        press(&mut app, "u");
        assert!(!tmp.path().join("draft.md").exists());
        assert!(tmp.path().join(".trash/draft.md").exists());
        press(&mut app, "u");
        assert_eq!(app.message().unwrap().text, "nothing to undo");
    }

    #[test]
    fn jump_goes_to_a_path() {
        let (tmp, mut app) = setup();
        press(&mut app, ":src/app.rs\n");
        assert_eq!(app.cwd, tmp.path().join("src"));
        assert_eq!(app.selected().unwrap().name, "app.rs");
    }

    #[test]
    fn find_jumps_to_a_file_anywhere_below() {
        let (tmp, mut app) = setup();
        press(&mut app, "f");
        assert!(matches!(&app.mode, Mode::Find(f) if f.indexing()));
        assert!(app.pending());
        wait(
            &mut app,
            |a| matches!(&a.mode, Mode::Find(f) if !f.indexing()),
        );
        press(&mut app, "mainrs");
        let Mode::Find(finder) = &app.mode else {
            panic!("expected the finder")
        };
        let index = finder.index.as_ref().unwrap();
        assert_eq!(index.paths[finder.matches[0].idx], "src/main.rs");
        press(&mut app, "\n");
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.cwd, tmp.path().join("src"));
        assert_eq!(app.selected().unwrap().name, "main.rs");
    }

    #[test]
    fn reopening_find_uses_the_last_index_then_refreshes_it() {
        let (tmp, mut app) = setup();
        let ready = |a: &App| matches!(&a.mode, Mode::Find(f) if !f.indexing());
        press(&mut app, "f");
        wait(&mut app, ready);
        press(&mut app, "\x1b");

        tmp.file("src/new.rs", "");
        press(&mut app, "f");
        let Mode::Find(finder) = &app.mode else {
            panic!("expected the finder")
        };
        assert!(finder.index.is_some(), "the cached index is there at once");
        assert!(finder.indexing(), "and a fresh one is on its way");
        assert!(!finder.matches.is_empty());
        press(&mut app, "newrs");
        wait(&mut app, ready);
        let Mode::Find(finder) = &app.mode else {
            panic!("expected the finder")
        };
        let index = finder.index.as_ref().unwrap();
        assert_eq!(
            index.paths[finder.matches[0].idx], "src/new.rs",
            "the refresh found it"
        );
    }

    #[test]
    fn find_can_jump_into_a_directory_or_be_cancelled() {
        let (tmp, mut app) = setup();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        wait(
            &mut app,
            |a| matches!(&a.mode, Mode::Find(f) if !f.indexing()),
        );
        press(&mut app, "\x1b");
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.cwd, tmp.path());
        press(&mut app, "f");
        wait(
            &mut app,
            |a| matches!(&a.mode, Mode::Find(f) if !f.indexing()),
        );
        press(&mut app, "src/\n");
        assert_eq!(app.cwd, tmp.path().join("src"));
    }

    /// Polls the workers until `done` holds.
    fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(app) {
            assert!(Instant::now() < deadline, "a worker never answered");
            app.poll_preview();
            app.poll_git();
            app.poll_job();
            app.poll_find();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn preview_arrives_from_the_worker() {
        let (_tmp, mut app) = setup();
        app.sync_preview();
        wait(&mut app, |a| a.preview.is_some());
        assert!(
            matches!(&app.preview, Some((k, Preview::Dir(entries))) if k.path.ends_with("src") && entries.len() == 2)
        );
    }

    #[test]
    fn git_status_arrives_from_the_worker_and_clears_outside_the_repo() {
        let tmp = TempDir::new();
        let repo = tmp.path().join("repo");
        tmp.file("repo/new.txt", "");
        // Run from a git hook, the suite inherits GIT_DIR, which would make `git init`
        // reinitialise this repository instead of creating the one under test.
        let mut init = Command::new("git");
        for var in git::REPO_ENV {
            init.env_remove(var);
        }
        let init = init.arg("init").arg("-q").arg(&repo).output().unwrap();
        assert!(init.status.success(), "git init: {init:?}");

        let mut app = App::new(&repo, None).unwrap();
        assert!(
            app.git.is_none(),
            "status must not be read on the event loop"
        );
        assert!(app.pending());
        wait(&mut app, |a| a.git.is_some());
        let status = app.git.as_ref().unwrap().status_of(&repo.join("new.txt"));
        assert_eq!(status, Some(git::Status::Untracked));
        // The whole repository rolls up to untracked; a status taken from the wrong
        // repository (an inherited GIT_DIR) shows that one's files as deleted.
        let whole = app.git.as_ref().unwrap().status_of(&repo);
        assert_eq!(whole, Some(git::Status::Untracked));

        press(&mut app, "h");
        assert!(app.git.is_none());
        assert!(!app.pending());
    }

    #[test]
    fn notices_changes_on_disk() {
        let (tmp, mut app) = setup();
        // mtime granularity can be a second on some filesystems; wait it out.
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(tmp.path().join("new.txt"), "").unwrap();
        assert!(app.tick());
        assert!(names(&app).contains(&"new.txt"));
    }

    #[test]
    fn alt_chords_do_nothing() {
        let (_tmp, mut app) = setup();
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(app.selected, 0);
    }

    fn click(app: &mut App, column: u16, row: u16) -> Option<Effect> {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// A 60x10 screen: parent at columns 0–9, current 10–29, preview 30–59, rows 1–8.
    fn with_areas(app: &mut App) {
        app.areas = Areas {
            parent: Rect::new(0, 1, 10, 8),
            current: Rect::new(11, 1, 19, 8),
            preview: Rect::new(32, 1, 28, 8),
        };
    }

    #[test]
    fn click_selects_and_double_click_opens() {
        let (tmp, mut app) = setup();
        with_areas(&mut app);
        assert!(click(&mut app, 15, 2).is_none());
        assert_eq!(app.selected().unwrap().name, "Cargo.toml");
        click(&mut app, 15, 1);
        assert_eq!(app.selected().unwrap().name, "src");
        assert_eq!(app.cwd, tmp.path(), "one click only selects");
        click(&mut app, 15, 1);
        assert_eq!(
            app.cwd,
            tmp.path().join("src"),
            "a second click on the same row opens"
        );
        // Below the last entry: nothing to select.
        click(&mut app, 15, 7);
        assert_eq!(app.selected().unwrap().name, "app.rs");
    }

    #[test]
    fn a_double_click_on_a_file_edits_it() {
        let (_tmp, mut app) = setup();
        with_areas(&mut app);
        click(&mut app, 15, 2);
        assert!(matches!(click(&mut app, 15, 2), Some(Effect::Run(_))));
    }

    #[test]
    fn wheel_moves_the_list_or_scrolls_the_preview() {
        let (_tmp, mut app) = setup();
        with_areas(&mut app);
        let wheel = |app: &mut App, kind, column| {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row: 2,
                modifiers: KeyModifiers::NONE,
            })
        };
        wheel(&mut app, MouseEventKind::ScrollDown, 15);
        assert_eq!(app.selected, 2, "clamped at the last entry");
        wheel(&mut app, MouseEventKind::ScrollUp, 15);
        assert_eq!(app.selected, 0);
        let lines = vec![Line::raw("x"); 10];
        let text = Preview::Text {
            lines,
            syntax: "Plain Text".into(),
            truncated: false,
        };
        let key = preview::Key {
            path: app.cwd.clone(),
            diff: false,
        };
        app.preview = Some((key, text));
        wheel(&mut app, MouseEventKind::ScrollDown, 40);
        assert_eq!(app.preview_scroll, WHEEL_STEP as usize);
        assert_eq!(app.selected, 0, "the list stays put under a preview scroll");
    }

    #[test]
    fn clicking_the_parent_or_a_directory_preview_navigates() {
        let (tmp, mut app) = setup();
        with_areas(&mut app);
        app.sync_preview();
        wait(&mut app, |a| a.preview.is_some());
        // The preview of src lists app.rs then main.rs.
        click(&mut app, 40, 2);
        assert_eq!(app.cwd, tmp.path().join("src"));
        assert_eq!(app.selected().unwrap().name, "main.rs");
        // In the parent column the top row is src's parent's first entry: src itself.
        click(&mut app, 3, 1);
        assert_eq!(app.cwd, tmp.path().join("src"));
        let row = app
            .parent
            .iter()
            .position(|e| e.name == "README.md")
            .unwrap() as u16;
        click(&mut app, 3, 1 + row);
        assert_eq!(app.cwd, tmp.path());
        assert_eq!(app.selected().unwrap().name, "README.md");
    }

    #[test]
    fn d_shows_the_diff_only_for_changed_files() {
        let tmp = TempDir::new();
        let git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            for var in git::REPO_ENV {
                cmd.env_remove(var);
            }
            let out = cmd
                .arg("-C")
                .arg(tmp.path())
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q"]);
        tmp.file("changed.rs", "a\n");
        tmp.file("clean.rs", "b\n");
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        fs::write(tmp.path().join("changed.rs"), "a\nb\n").unwrap();

        let mut app = App::new(tmp.path(), None).unwrap();
        wait(&mut app, |a| a.git.is_some());
        press(&mut app, "D");
        app.sync_preview();
        wait(&mut app, |a| a.preview.is_some() && !a.pending());
        let (key, preview) = app.preview.as_ref().unwrap();
        assert!(key.diff && key.path.ends_with("changed.rs"));
        assert!(matches!(
            preview,
            Preview::Diff {
                added: 1,
                removed: 0,
                ..
            }
        ));

        press(&mut app, "j");
        app.sync_preview();
        wait(&mut app, |a| !a.pending());
        let (key, _) = app.preview.as_ref().unwrap();
        assert!(
            !key.diff,
            "a clean file shows its content even with diffs on"
        );
    }

    #[test]
    fn marks_rename_together_in_the_editor() {
        use std::os::unix::process::ExitStatusExt;
        let (tmp, mut app) = setup();
        press(&mut app, "j  ");
        let Some(Effect::Run(_)) = app.handle_key(key(KeyCode::Char('r'))) else {
            panic!("r with marks should open the editor");
        };
        let list = app.bulk.as_ref().unwrap().list.clone();
        assert_eq!(
            fs::read_to_string(&list).unwrap(),
            "Cargo.toml\nREADME.md\n"
        );
        fs::write(&list, "Cargo.toml\nREAD_ME.md\n").unwrap();
        app.after_run(Ok(ExitStatus::from_raw(0)));
        assert!(tmp.path().join("READ_ME.md").exists());
        assert!(!tmp.path().join("README.md").exists());
        assert!(app.marked.is_empty());
        assert_eq!(app.message().unwrap().text, "renamed 1 item · u to undo");
        assert!(!list.exists(), "the list is cleaned up");
    }

    #[test]
    fn each_bulk_rename_gets_its_own_list() {
        let (_one, mut a) = setup();
        let (_two, mut b) = setup();
        for app in [&mut a, &mut b] {
            press(app, "j  ");
            app.handle_key(key(KeyCode::Char('r')));
        }
        let list = |app: &App| app.bulk.as_ref().unwrap().list.clone();
        assert_ne!(
            list(&a),
            list(&b),
            "a shared list lets one rename read the other's names"
        );
        for app in [&a, &b] {
            fs::remove_file(list(app)).unwrap();
        }
    }

    #[test]
    fn a_failed_editor_renames_nothing() {
        use std::os::unix::process::ExitStatusExt;
        let (tmp, mut app) = setup();
        press(&mut app, "j  ");
        app.handle_key(key(KeyCode::Char('r')));
        let list = app.bulk.as_ref().unwrap().list.clone();
        fs::write(&list, "x\ny\n").unwrap();
        app.after_run(Ok(ExitStatus::from_raw(1 << 8)));
        assert!(tmp.path().join("README.md").exists());
        assert_eq!(app.message().unwrap().kind, MessageKind::Error);
    }

    #[test]
    fn word_start_splits_on_separators() {
        assert_eq!(word_start("foo bar"), 4);
        assert_eq!(word_start("src/app/"), 4);
        assert_eq!(word_start("word"), 0);
    }
}
