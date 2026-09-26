# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Three-column browser: parent, current directory, and a preview built on a
  background thread.
- Nerd Font file-type icons in every listing and in find; `--no-icons` turns them off.
- A screen organised by what each part answers (see `docs/design.md`): location and
  repository state along the top; an inspector that names the selection and gives its facts
  (kind, size, lines, age, mode) above its content; a filter row on the listing it narrows;
  a status bar with the mode and anything pending; and a command line that shows the prompt,
  the latest outcome, or the keys that make sense for the selection.
- A colour system by role, so each colour means one thing, and `--light` for light terminals.
- Settings in `~/.config/findr/config.toml`: `glyphs`, `theme`, `mouse`, `hints`, `position`.
- One quiet bottom bar: a prompt, a question, a message or what is pending on the left, the
  position on the right, `? for keys` when nothing applies. Key hints are an optional row.
- Glyphs in three tiers — Nerd Font, Unicode, ASCII — chosen by what is installed and whether
  the terminal is UTF-8, so the screen degrades instead of showing boxes.
- Narrow terminals drop the parent column, then the preview, before squeezing the list.
- Syntax-highlighted file previews in over 200 languages (bat's grammars), directory
  previews, and notes for binary and empty files.
- Git status on every entry, rolled up to directories, with the branch in the header.
- Fuzzy, smart-case filtering with `/`.
- `D` shows a changed file's diff against `HEAD` in the inspector, syntax-highlighted on a
  green or red wash per changed line,, and stays on while moving
  between files, so reviewing changes is `j` and `k`.
- Find anywhere below the current directory with `f` or `ctrl-p`, honouring `.gitignore`
  in every repository found, and jump to the result.
- Copy, cut, paste, rename, create (with parent directories), and move to trash.
- `u` undoes, most recent first and back to the start of the session: trash is restored,
  renames (single or bulk) are reversed, a cut is moved back, and pasted copies, new files and
  new directories go to the trash. Nothing is overwritten: a name taken since comes back as
  `name copy`, and a rename is not reversed onto a file that has appeared in its place.
- Bulk rename: with entries marked, `r` opens their names in `$EDITOR`; the edited list is
  checked as a whole (count, empty names, duplicates, clashes) before anything moves, and
  swaps and chains work.
- `$EDITOR`, shell, system opener, and clipboard integration.
- `--cwd-file` for changing the shell's directory on quit.
- Mouse support: wheel scrolling, click to select, double-click to open, and click-to-go in
  the parent column and directory previews. `--no-mouse` turns it off.
