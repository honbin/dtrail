# dtrail

A local TUI to retrace your Claude Code, Codex, and Cursor sessions.

dtrail reads your local Claude Code, Codex, and Cursor session files and lets you
re-read past work from the terminal — the conversation plus tool calls and
their results, in chronological order, with Vim-like keyboard navigation. The
layout is tig-like: a session list on the left (each row tagged `Claude`,
`Codex`, or `Cursor` when more than one is present) and the selected session's conversation on the
right, which follows the list cursor. It is a viewer, not a session manager: it
never modifies your files and leaves nothing behind.

## Install

Requires a Rust toolchain.

```sh
# Install the `dtrail` command straight from GitHub
cargo install --git https://github.com/honbin/dtrail

# Or, from a local clone:
cargo install --path .
```

## Usage

The installed command is `dtrail`.

```sh
# Browse all local sessions, newest first
dtrail

# Browse every session under a directory (recursively)
dtrail path/to/dir

# Open a single session file directly
dtrail path/to/session.jsonl
```

With no argument, sessions are discovered from both agents and merged, newest
first:

- Claude Code: `$CLAUDE_CONFIG_DIR/projects` if set, else `~/.claude/projects`.
- Codex: `$CODEX_HOME/sessions` if set, else `~/.codex/sessions`.
- Cursor: `~/.cursor/projects` (agent transcripts).

A directory argument browses every `.jsonl` under it (recursively), and a file
argument opens that file — both independent of the default locations, with each
file's agent detected from its content (the agents may be mixed). A
missing path is reported as an error rather than falling back to discovery.

## Keys

Browsing and reading are distinct: the list pane keeps the list and preview
side by side so you can compare sessions. `l` / `Tab` moves into the body to
scroll it with the list still visible; `Enter` dives in and reads full-width.
`z` overrides the layout manually, independent of diving.

### List pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up (the preview follows) |
| `Ctrl-d` / `Ctrl-u` | Half page down / up |
| `Ctrl-f` / `Ctrl-b` | Full page down / up |
| `Space` | Page down |
| `gg` / `G` | Jump to first / last |
| `a` | Cycle the agent filter: all → Claude → Codex → Cursor |
| `p` | Toggle hiding sessions with empty or command-only previews |
| `/` | Filter by project or preview text |
| `l` / `→` / `Tab` | Move into the body, keeping the split (scroll with `j`/`k`) |
| `Enter` | Dive into the session and read it full-width |
| `z` | Toggle full-width / split (zoom) |
| `q` | Quit |

### Conversation pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move the line cursor down / up |
| `Ctrl-d` / `Ctrl-u` | Half page down / up |
| `Ctrl-f` / `Ctrl-b` | Full page down / up |
| `Space` | Page down |
| `gg` / `G` | Jump to top / bottom |
| `o` / `Enter` | Fold / unfold the item under the cursor (`▸`/`▾`) |
| `/` | Search within the session |
| `n` / `N` | Next / previous match |
| `v` / `V` | Start a line-wise visual selection (movement extends it) |
| `y` | Yank the selection to the clipboard (then `Esc` cancels) |
| `z` | Toggle full-width / split (zoom) |
| `h` / `←` / `Tab` / `q` | Back to the list (or quit if opened directly) |

`Ctrl-l` forces a full redraw from any screen. Yank copies via OSC 52, so it
works over SSH on terminals that support it (tmux needs `set-clipboard on`).

Opening a session from a filtered list carries the query into the conversation
as a body search, jumping to the first occurrence so `n`/`N` step through it.
Search covers the whole session — messages, tool input/output, and thinking —
and a hit inside a folded item reveals just that item. The conversation's
status line shows the session's start time (as recorded, UTC). An active search
or filter shows on its own line below it.

While editing a query, `Up`/`Down` (or `Ctrl-p`/`Ctrl-n`) move the selection
(list) or jump between matches (conversation), `Esc` cancels, and `Enter`
commits — opening the highlighted session in the list, or keeping the current
match in a conversation. On terminals narrower than 90 columns only the
focused pane is shown.

## Privacy

- Reads the local session JSONL files only; opens no network connections.
- Treats session files as read-only; never writes to them.
- Creates no on-disk database, cache, search index, or log files.

## Development

```sh
cargo test                   # unit and integration tests
cargo run -- <file>          # run against a real session to check rendering
```

## License

MIT — see [LICENSE](LICENSE).
