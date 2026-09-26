# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Three-column browser: parent, current directory, and a preview built on a
  background thread.
- Nerd Font file-type icons in every listing and in find; `--no-icons` turns them off.
- Syntax-highlighted file previews, directory previews, and notes for binary and
  empty files.
- Git status on every entry, rolled up to directories, with the branch in the header.
- Fuzzy, smart-case filtering with `/`.
- Find anywhere below the current directory with `f` or `ctrl-p`, honouring `.gitignore`
  in every repository found, and jump to the result.
- Copy, cut, paste, rename, create (with parent directories), and move to trash.
- `$EDITOR`, shell, system opener, and clipboard integration.
- `--cwd-file` for changing the shell's directory on quit.
- Mouse support: wheel scrolling, click to select, double-click to open, and click-to-go in
  the parent column and directory previews. `--no-mouse` turns it off.
