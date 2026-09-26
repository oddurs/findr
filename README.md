# findr

[![ci](https://github.com/oddurs/findr/actions/workflows/ci.yml/badge.svg)](https://github.com/oddurs/findr/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A terminal file browser for developers: Finder's column view, with git status,
fuzzy filtering, and syntax-highlighted previews.

```
 ~/Code/findr/src  main*
 ? src/            │   app.rs                   36 K│   1 //! The preview column, built on a worker…
 ! target/         │ M dir.rs                   12 K│   2
   Cargo.lock      │   fuzzy.rs                3.5 K│   3 use std::fs::{self, File};
   Cargo.toml      │ ? preview.rs              8.2 K│   4 use std::io::Read;
 -rw-r--r--  8.2 K  19m ago                                                 name↓  8/9
```

- **Three columns**: parent, current, preview. Previews are highlighted and built on a
  background thread, so holding `j` never stutters.
- **Git status** on every entry, rolled up to directories, with ignored files dimmed and the
  branch in the header.
- **Fuzzy filter** with `/`, ranked like fzf and smart-case.
- **File operations**: copy, cut, paste, rename, trash, and `a src/new/mod.rs` to create a file with its parents.
- **Hands off to your tools**: `$EDITOR`, a shell in the current directory, the system opener,
  and the clipboard.
- **Cd on quit**, so the shell follows you (see below).
- Watches the current directory and refreshes when it changes.

## Install

```sh
cargo install --git https://github.com/oddurs/findr
```

It runs on macOS and Linux, and needs `git` on your `PATH` for status.

## Usage

```
findr [PATH] [--cwd-file FILE]
```

`PATH` may be a file, which opens its directory with the cursor on it. Press `?` for keys.

| Keys | |
| --- | --- |
| `j k` `↑ ↓` | move |
| `h l` `← →` | parent / open |
| `enter` | enter directory or edit file |
| `gg` `G` `^d` `^u` | top, bottom, half page |
| `gh` `gr` `g/` | home, git root, filesystem root |
| `-` | previous directory |
| `:` | go to a path (`~` expands) |
| `/` | fuzzy filter; `enter` keeps it, `esc` clears |
| `space` | mark (marks apply to y x d e o c) |
| `y` `x` `p` | copy, cut, paste |
| `d` | move to trash (asks first) |
| `r` | rename |
| `a` `A` | new file (a trailing `/` makes a directory), new directory |
| `e` | edit in `$VISUAL` / `$EDITOR` |
| `o` | open with the default app |
| `c` | copy path to clipboard |
| `!` | shell here |
| `J` `K` | scroll preview |
| `.` | show hidden files |
| `s` `S` | cycle sort (name, modified, size, type), reverse |
| `R` | reload |
| `q` `Q` | quit and cd, quit |

Trash goes to `~/.Trash` on macOS and the freedesktop trash elsewhere. findr never deletes outright.

## Cd on quit

`q` writes the final directory to the file given by `--cwd-file`. Wrap findr in a shell
function so your shell ends up in that directory.

fish (`~/.config/fish/functions/f.fish`):

```fish
function f
    set -l tmp (mktemp)
    findr --cwd-file $tmp $argv
    set -l dir (command cat $tmp)
    rm -f $tmp
    test -n "$dir"; and test "$dir" != "$PWD"; and cd $dir
end
```

zsh / bash:

```sh
f() {
    local tmp dir
    tmp=$(mktemp)
    findr --cwd-file "$tmp" "$@"
    dir=$(cat "$tmp")
    rm -f "$tmp"
    [ -n "$dir" ] && [ "$dir" != "$PWD" ] && cd "$dir"
}
```

## Development

```sh
scripts/setup         # once: wires the git hooks
scripts/task check    # format, lint, test, build — what CI runs
```

Work happens one branch per worktree, one pull request per branch, through
`scripts/agent`; `main` only moves by merging a pull request. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
