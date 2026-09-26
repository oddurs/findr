# Security Policy

## Supported versions

findr is pre-1.0. Only the latest release receives fixes.

| Version | Supported |
| ------- | --------- |
| 0.1.x   | ✅        |
| < 0.1   | ❌        |

## Reporting a vulnerability

**Please do not open a public issue.**

Report privately through GitHub Security Advisories:

<https://github.com/oddurs/findr/security/advisories/new>

Include the version (`findr --version`), your platform and terminal, and the
smallest directory layout and key sequence that reproduces it.

## What to expect

- **Acknowledgement within 3 days.** If you have not heard back by then, assume
  it went astray and open a public issue saying only that you are waiting on a
  private report — no details.
- **An assessment within 7 days**, with a severity and a plan.
- **A fix or a documented mitigation within 30 days** for anything exploitable.
- Credit in the release notes, unless you would rather not be named.

## Scope

findr browses directories you point it at, which may hold files you did not
write — a cloned repository, a downloaded archive. The boundaries that matter:

- **File contents and file names are untrusted.** They are drawn, never
  interpreted. A name or file that gets an escape sequence through to the
  terminal, or that crashes findr, is a finding.
- **Paths never become shell code.** `git` runs with a fixed argument vector.
  `$EDITOR` runs through `sh -c` so that an editor configured with arguments
  works, but the paths are passed as positional parameters, not spliced into
  the command string. A path that reaches a shell as code is a real finding.
- **Nothing is destroyed without asking.** Delete moves to the trash after a
  confirmation; rename, create and paste refuse to overwrite. A key sequence
  that loses data without a prompt is a finding.
- **The terminal is always handed back**, including on panic.

Out of scope: anything that requires an attacker who can already set your
environment variables (such as `EDITOR` or `SHELL`), run commands as you, or
replace the `git` on your `PATH`.
