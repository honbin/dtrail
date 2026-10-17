# ccscope

A local TUI for browsing Claude Code and Codex session histories in one place.

ccscope reads your local Claude Code and Codex session files and lets you
re-read past work from the terminal — the conversation plus tool calls and
their results, in chronological order, with Vim-like keyboard navigation. The
layout is tig-like: a session list on the left (each row tagged `Claude` or
`Codex` when both are present) and the selected session's conversation on the
right, which follows the list cursor. It is a viewer, not a session manager: it
never modifies your files and leaves nothing behind.

## Install

Requires a Rust toolchain.

```sh
# Install the `ccs` command straight from GitHub
cargo install --git https://github.com/honbin/ccscope

# Or, from a local clone:
cargo install --path .
```

## Usage

The installed command is `ccs`.

```sh
# Browse all local sessions, newest first
ccs

# Browse every session under a directory (recursively)
ccs path/to/dir

# Open a single session file directly
ccs path/to/session.jsonl
```

With no argument, sessions are discovered from both agents and merged, newest
first:

- Claude Code: `$CLAUDE_CONFIG_DIR/projects` if set, else `~/.claude/projects`.
- Codex: `$CODEX_HOME/sessions` if set, else `~/.codex/sessions`.

A directory argument browses every `.jsonl` under it (recursively), and a file
argument opens that file — both independent of the default locations, with each
file's agent detected from its content (Claude Code and Codex may be mixed). A
missing path is reported as an error rather than falling back to discovery.

## Keys

Browsing and reading are distinct: the list pane keeps the list and preview
side by side so you can compare sessions, and diving into a session reads it
full-width. `O` overrides the layout manually, independent of diving.

### List pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up (the preview follows) |
| `gg` / `G` | Jump to first / last |
| `a` | Cycle the agent filter: all → Claude → Codex |
| `/` | Filter by project or preview text |
| `Enter` / `l` | Dive into the session and read it full-width |
| `O` | Toggle full-width / split |
| `q` | Quit |

### Conversation pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move the line cursor down / up |
| `Ctrl-d` / `Ctrl-u` | Half page down / up |
| `gg` / `G` | Jump to top / bottom |
| `o` / `Space` / `Enter` | Fold / unfold the item under the cursor (`▸`/`▾`) |
| `/` | Search within the session |
| `n` / `N` | Next / previous match |
| `O` | Toggle full-width / split |
| `Tab` / `h` / `q` | Back to the split list (or quit if opened directly) |

`Ctrl-l` forces a full redraw from any screen.

Opening a session from a filtered list carries the query into the conversation
as a body search, jumping to the first occurrence so `n`/`N` step through it.
Search covers the whole session — messages, tool input/output, and thinking —
and a hit inside a folded item reveals just that item. The conversation's
status line shows the session's start time (as recorded, UTC).

While editing a query, `Up`/`Down` (or `Ctrl-p`/`Ctrl-n`) move the selection
(list) or jump between matches (conversation), `Esc` cancels, and `Enter`
commits — opening the highlighted session in the list, or keeping the current
match in a conversation. On terminals narrower than 90 columns only the
focused pane is shown.

## Privacy

- Reads the local session JSONL files only; opens no network connections.
- Treats session files as read-only; never writes to them.
- Creates no database, cache, or search index, and writes no logs.

## Development

```sh
cargo test                   # unit and integration tests
cargo run -- <file>          # run against a real session to check rendering
```

## License

MIT — see [LICENSE](LICENSE).
