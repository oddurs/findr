# Contributing

Thank you for looking. This is a small project with a strict workflow — the
strictness is what lets an agent and a person work in it without stepping on
each other.

## Once, after cloning

```sh
scripts/setup
```

That wires `core.hooksPath` to `.githooks/` and checks your toolchain. The hooks
are the workflow; without them you find out about a problem in CI instead of in
your terminal.

## The loop

One unit of work is one branch, in one worktree, with one pull request.

```sh
scripts/agent doctor                      # is this checkout ready?
scripts/agent start fix/preview-tabs      # branch + worktree, prints the path
cd ../.worktrees/findr/fix/preview-tabs

# ... work ...

scripts/agent check                       # fmt, lint, test, build
scripts/agent commit "fix(preview): expand tabs to the next stop"
scripts/agent pr                          # checks, pushes, opens the PR
scripts/agent sync                        # rebase onto main when it moves
# ... after the PR is merged ...
cd -                                      # back to the primary checkout
scripts/agent done fix/preview-tabs       # remove the worktree and the branch
```

`scripts/agent list` shows every worktree, its branch, and its pull request.

Worktrees live in `../.worktrees/findr/<branch>/`, outside the repository, so two
branches never share an index or a `target/` directory.

## `main` only advances through a merged pull request

Not by convention: the local `pre-push` hook refuses a push to `main`, and a
repository ruleset refuses it again on the server. The ruleset has no bypass
actors, so this holds for the maintainer too.

Required approvals are set to **0**, deliberately: this is a solo project, and a
review requirement would deadlock the only person who can review. Everything
else — a pull request, a green `required` check, an up-to-date branch, resolved
conversations — is required. If the project gains a second maintainer, that
number becomes 1.

## Checks

Everything goes through one seam, so CI and your machine cannot disagree:

```sh
scripts/task fmt        # format in place
scripts/task fmt:check  # verify formatting
scripts/task lint       # clippy, warnings denied
scripts/task test       # the full suite
scripts/task build      # compile everything
scripts/task check      # all of the above — what CI runs
```

The hooks run these for you: `pre-commit` checks formatting and lint, `pre-push`
runs the lot. Never reach for `--no-verify`. If a hook is wrong, fix the hook in
its own pull request.

The main view is held to a recorded screen with
[insta](https://insta.rs). If your change moves the layout on purpose, re-record
with `cargo insta review` (or `INSTA_UPDATE=always cargo test`) and **read the
diff** — it is the review of your change to the interface.

## Commits

[Conventional Commits](https://www.conventionalcommits.org), imperative mood,
subject under 72 characters, no trailing period:

```
fix(preview): expand tabs to the next stop

Tabs were replaced with four spaces wherever they fell, which misaligned
tables in Makefiles and Go files.
```

Types: `feat` `fix` `chore` `docs` `perf` `refactor` `test` `build` `ci`
`style` `revert`.

The body explains **why**; the diff already says what.

No commit, pull request, comment, or line of documentation attributes work to an
assistant, a model, or a tool. The `commit-msg` hook enforces it.

## Code

- Comments explain **why**, not what.
- `App` holds state and handles keys; anything that needs the terminal comes
  back to `main.rs` as an `Effect`. Keep the terminal out of `app.rs`.
- A bug fix arrives with the test that would have caught it, named after the
  behaviour it protects.
