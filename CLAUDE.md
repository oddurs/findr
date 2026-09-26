# Working in this repository

Instructions for an agent working on findr, a terminal file browser in Rust.

## Rules

- **Never commit to `main`.** It advances only through a merged pull request.
  The `pre-push` hook refuses a push to it, and a server-side ruleset refuses it
  again with no bypass actors. Do not look for a way around either.
- **One unit of work, one worktree, one branch, one pull request.** Never share a
  checkout with another agent.
- **Never use `--no-verify`**, `continue-on-error`, or `|| true` to make a check
  pass. If a check is wrong, fix the check in its own pull request.
- **Never attribute work to an assistant, a model, or a tool** — not in commits,
  trailers, pull requests, comments, docs, or release notes. The work is
  published under its author's name. The `commit-msg` hook rejects it.

## The loop

```sh
scripts/agent doctor                    # checkout ready?
scripts/agent start <type>/<slug>       # prints the worktree path; cd there yourself
scripts/agent check                     # scripts/task check
scripts/agent commit "type(scope): subject"
scripts/agent pr [--draft]              # check, push, open the PR
scripts/agent sync                      # rebase onto the default branch
scripts/agent done <branch>             # after merge, from the primary checkout
scripts/agent list
```

Branches are `<type>/<slug>` with type in `feat fix chore docs perf refactor test`.
Worktrees live in `../.worktrees/findr/<branch>/`.

## The seam

All automation goes through `scripts/task`. Do not put a `cargo` invocation in
CI, a hook, or another script — put it here, once:

```sh
scripts/task fmt        # format in place
scripts/task fmt:check  # verify formatting
scripts/task lint       # clippy, warnings denied
scripts/task test       # the full suite
scripts/task build      # compile everything
scripts/task check      # all of the above; what CI runs
```

A branch is green under `scripts/task check` before it becomes a pull request.

## Commits

Conventional Commits, imperative, subject under 72 characters, no trailing
period. The body explains why; the diff already says what.

Types: `feat` `fix` `chore` `docs` `perf` `refactor` `test` `build` `ci`
`style` `revert`.

In the pull request description, fill in every heading of the template —
especially *Look at this sceptically*: name the weakest part of the change.

## Architecture, and what must stay true

- **The screen follows `docs/design.md`.** Read it before changing anything a user sees: each
  piece of information has one region, colours are roles from the theme, and views are built
  from the shared components. If a change needs a new rule, change the document with it.

- `src/app.rs` owns state and key handling. Work that needs the terminal (an
  editor, a shell, quitting) is returned as an `Effect` and carried out by
  `src/main.rs`. Do not touch the terminal from `app.rs` or `src/ui/`.
- `src/ui/` draws from `App` and writes back only the viewport (`offset`,
  `page`). No filesystem writes, no processes while drawing.
- Previews are built on the worker thread in `src/preview.rs`. Anything slow
  that depends on the selection belongs there, never in the event loop. Only
  the newest request is honoured; keep it that way.
- Nothing destroys data without a confirmation. Delete moves to the trash;
  rename, create and paste refuse to overwrite (`ops::unique_dest`).
- Paths never become shell code. Pass them as arguments, as `edit()` does with
  `sh -c '... "$@"'`.
- Dependencies earn their place. Prefer the standard library; git is read by
  running `git`, not by linking a library.

## Tests

```sh
cargo test                         # everything
INSTA_UPDATE=always cargo test     # accept a deliberate screen change, then read the diff
```

Tests live beside the code in `#[cfg(test)] mod tests`. Use
`dir::testutil::TempDir` for anything touching the filesystem. Name a test after
the behaviour it protects. A bug fix arrives with the test that would have
caught it. Never re-record a snapshot to make a failure go away without reading
what moved.

## Comments

Explain **why**, not what. Match the density and voice of the surrounding code.
No `TODO` stubs, no commented-out code.
