# Design

How findr's screen is organised, why, and the system it is built from. Read this before
changing anything a user sees; change this when the reasoning changes.

## Who it is for, and what they come to do

A developer who lives in a terminal and wants Finder's column view without leaving it. Every
session is some mix of five jobs, usually in this order:

| Job | The question in their head | What serves it |
| --- | --- | --- |
| **Orient** | Where am I, and what state is this place in? | Path, repository, branch, what changed |
| **Locate** | Where is the thing I want? | The listing, the filter, find |
| **Inspect** | Is this it? What is in it? | The preview and the facts about the selection |
| **Act** | Do something to it. | Open, edit, copy, move, trash, hand off to a shell |
| **Leave** | Take me there. | Quit into the directory |

Locating and inspecting are most of the time spent, so the listing and the preview get most of
the screen. Orientation is glanced at, so it is compact and stable. Acting is brief and must be
unmistakable.

## Principles

1. **The listing is the product.** The current column carries the emphasis; everything else
   supports it and is quieter than it.
2. **Show state where it applies.** A filter narrows the listing, so it is shown on the
   listing. Facts about the selection belong beside the selection's preview. What belongs to the
   session (marks, clipboard, a paste in progress) belongs to the status bar.
3. **Say what can be done next, not everything that can be done.** The bottom line offers the
   three or four keys that make sense for the selection and the current state. The full list is
   one `?` away.
4. **Quiet by default, loud on change.** Stable facts are muted. Colour is spent on what differs
   from normal — a modified file, a mark, an error, the cursor.
5. **One meaning per colour.** A colour that means two things means nothing. Every colour is a
   role from the token table below, never a literal at the point of use.
6. **Degrade, don't break.** A narrow terminal loses context columns before the listing loses
   width. No Nerd Font means no icons, not boxes. A light terminal gets a light palette.

## Information architecture

The screen has six regions. Each answers one question and owns the information that answers
it; nothing appears in two regions.

```
 location ─ where am I                                           repository ─ what state
 ┌───────────┬──────────────────────────┬──────────────────────────────────────────────┐
 │ context   │ listing                  │ inspector                                    │
 │ where I   │ what is here; the cursor │ title: what the selection is                 │
 │ came from │                          │ facts: kind · size · lines · age · mode      │
 │           │                          │ content: text, or what is inside             │
 │           │                          │                                              │
 │           │ filter: how it narrows   │                                              │
 └───────────┴──────────────────────────┴──────────────────────────────────────────────┘
 status ─ what mode, what is pending, where in the list
 command ─ type here · what just happened · what I can do next
```

| Region | Answers | Holds | Emphasis |
| --- | --- | --- | --- |
| Location | Where am I? | Breadcrumb, current directory brightest | Secondary |
| Repository | What state is this checkout in? | Branch, dirty, ahead/behind | Secondary |
| Context | Where did I come from? | Parent listing, the current directory marked | Muted |
| Listing | What is here, and which one am I on? | Entries, git status, marks, sizes, cursor | **Primary** |
| Filter | How is the listing narrowed? | The query and the match count, on the listing | Accent while active |
| Inspector | Is this the one? What changed? | Title, a line of facts, then the content — or, with `D` on a changed file, its diff | Primary content, muted facts |
| Status | What mode am I in, what is pending? | Mode, marks, clipboard, paste progress, hidden, sort, position | Muted except pending state |
| Command | What do I type, what happened, what next? | A prompt, or a message, or contextual key hints | Changes by moment |

The command line holds one thing at a time, in this order of precedence: a **prompt** while
input is expected, a **message** for a few seconds after something happens, otherwise
**hints** for the current selection and state.

### Modes

| Mode | Entered by | Input goes to | Shown as |
| --- | --- | --- | --- |
| Browse | default | keys act on the selection | badge, hints |
| Filter | `/` | the filter row on the listing | badge, cursor in the filter row |
| Find | `f` `^p` | the find overlay | badge, overlay |
| Prompt | `r` `a` `A` `:` | the command line | badge, prompt |
| Confirm | `d`, quitting mid-paste | a single `y` | badge, a question with its answers |
| Go | `g` | the next key | badge, the keys it accepts |
| Help | `?` | any key closes it | badge, overlay |

## Emphasis

Four levels of text, used consistently:

| Level | Used for |
| --- | --- |
| **Strong** | The current directory in the breadcrumb, the cursor row, the inspector title |
| **Normal** | Entry names, file content |
| **Muted** | Sizes, ages, sort, position, the facts line, separators, the context column |
| **Faint** | Structure: column rules, scrollbar, borders |

## Tokens

Colours are roles, defined once in `src/ui/theme.rs`. Named ANSI colours follow the terminal's
own palette, so findr matches whatever scheme the user runs; only the surfaces use fixed greys,
and those have a light variant.

| Role | Dark | Light | Means |
| --- | --- | --- | --- |
| `accent` | blue | blue | Focus: the cursor bar, the active mode, headings |
| `directory` | blue | blue | A directory's name and icon |
| `link` | cyan | cyan | A symbolic link |
| `executable` | green | green | An executable file |
| `match` | yellow | yellow | Characters a filter or find query matched |
| `success` | green | green | Added, ahead, a completed action |
| `warning` | yellow | yellow | Modified, dirty, pending clipboard |
| `danger` | red | red | Deleted, conflicted, behind, errors, trash |
| `info` | cyan | cyan | Renamed, work in progress |
| `mark` | magenta | magenta | Marked entries |
| `branch` | magenta | magenta | The branch name |
| `muted` | dark grey | dark grey | See *Emphasis* |
| `surface.selected` | grey 237 | grey 253 | The cursor row |
| `surface.context` | grey 236 | grey 254 | The current directory in the context column |

Git status maps onto the semantic roles: modified → warning, added → success, deleted and
conflicted → danger, renamed → info, untracked → danger (lighter), ignored → muted. A diff uses
the same roles: added lines success, removed lines danger, hunk headers info.

## Components

Everything on screen is built from these, in `src/ui/components.rs`:

| Component | Looks like | Used for |
| --- | --- | --- |
| **Badge** | ` BROWSE ` on accent; ` CONFIRM ` on danger | The mode |
| **Chip** | coloured text, spaced apart: `2 marked  1 item copied` | Pending state in the status bar |
| **Hint** | `y` copy | Keys, in the command line and prompts |
| **Facts** | `Rust · 12 K · 314 lines · 3m ago` | The inspector's second line |
| **Row** | mark, status, icon, name, size | Every listing: context, listing, inspector, find |
| **Prompt** | `rename ›` text, with `enter ok   esc cancel` flush right | Any text input |
| **Question** | `Move 3 items to the trash?   y yes   n no` | Confirmations |
| **Message** | `✓ pasted 3 items` / `✗ …` | Outcomes, success and error |
| **Empty** | a muted sentence | An empty directory, no matches |
| **Panel** | rounded border, accent title | Overlays: help, find |

## Layout

- Columns take 18 / 34 / 48 per cent. Below 90 columns the context column goes (0 / 42 / 58);
  below 60 the inspector goes too. The listing never shrinks to make room for context.
- Rows are one line. Names truncate with `…` before sizes do; paths in find truncate from the
  left, since the end of a path is what tells files apart.
- A scrollbar appears only when the listing overflows.

## Where it lives

`src/ui/theme.rs` holds the tokens (`Theme::DARK`, `Theme::LIGHT`), `src/ui/components.rs` the
components, `src/ui/rows.rs` the row, and `src/ui/mod.rs` one function per region. A new view
takes a `&Theme` and composes components; it does not name a colour.

## Copy

- Lower case, short, verb first: `pasted 3 items`, `moved 1 item to the trash`.
- Counts are exact and pluralised (`1 item`, `3 items`).
- Keys are written as typed: `y`, `^p`, `enter`, `esc`.
- Errors say what failed and on what: `rename: notes.md already exists`.

## Audit: the screen before this design

What the first versions did, and what changes.

| Where | Before | Problem | After |
| --- | --- | --- | --- |
| Status bar | Selection facts (mode, size, age, link target) beside session state | Two regions' information in one; size shown twice (status and preview title) | Facts move to the inspector's facts line; the status bar keeps mode and pending state |
| Command line | A fixed `? help f find / filter q quit` | The same four keys whatever is selected; nothing about marks or a clipboard waiting | Hints follow the selection and state (`p paste here` with a clipboard, `d trash 3` with marks) |
| Header | The active filter on the far right | Far from the listing it narrows | A filter row on the listing itself, with the match count |
| Filter input | Typed in the command line | Input and its effect in different places | Typed in the filter row |
| Context column | Same weight as the listing | Competes with the listing for attention | Muted |
| Colour | 37 literal styles; yellow meant five things | No colour could be trusted to mean one thing | Tokens by role; one meaning per colour |
| Confirm | Red bold sentence, `[y/N]` | Answers hidden in notation | A question with its answers as hints |
| Messages | Errors red, everything else plain | Success and information look alike | A success tone and a mark for each |
| Light terminals | Dark grey bars | Unreadable selection on a light background | A light palette (`--light`) |
