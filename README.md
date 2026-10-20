# dtrail

A local TUI to retrace your Claude Code, Codex, and Cursor sessions.

dtrail reads local session files from Claude Code and Codex, and agent
transcripts from Cursor. It lets you retrace past work from the terminal —
conversations, tool calls, and their results in chronological order, with
Vim-like keyboard navigation. The layout is tig-like: a session list on the
left, with each row tagged `Claude`, `Codex`, or `Cursor` when multiple agents
are present, and the selected session's conversation on the right. It is a
viewer, not a session manager: it never modifies your files or leaves anything
behind.

## Install

Requires a Rust toolchain.

```sh
# Install the `dtrail` command straight from GitHub
cargo install --git https://github.com/honbin/dtrail --locked

# Or, from a local clone:
cargo install --locked --path .
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

With no argument, sessions are discovered from all supported agents and merged,
newest first:

- Claude Code: `$CLAUDE_CONFIG_DIR/projects` if set, else `~/.claude/projects`.
- Codex: `$CODEX_HOME/sessions` if set, else `~/.codex/sessions`.
- Cursor: `~/.cursor/projects` (agent transcripts).

A directory argument browses every `.jsonl` under it (recursively), and a file
argument opens that file — both independent of the default locations, with each
file's agent detected from its content (the agents may be mixed). A
missing path is reported as an error rather than falling back to discovery.

Content of a kind dtrail doesn't recognize (e.g. images, or new item types an
agent adds) is shown as an `[unknown block: <type>]` placeholder rather than
dropped (a run of the same type shares one row with a count), and lines that
fail to parse are counted in the status line. Multi-line system text, such as
injected instructions, starts folded.

## Keys

Browsing and reading are distinct: the list pane keeps the list and body
side by side so you can compare sessions. `l` / `Tab` moves into the body to
scroll it with the list still visible; `Enter` dives in and reads full-width.
`z` overrides the layout manually, independent of diving.

### List pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up (the body follows) |
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
| `h` / `←` / `Tab` | Back to the list |
| `q` | Back to the list (or quit if opened directly) |

`Ctrl-l` forces a full redraw from any screen. Yank copies via OSC 52, so it
works over SSH on terminals that support it (tmux needs `set-clipboard on`).

Opening a session from a filtered list carries the query into a body search;
`n`/`N` step through matches. Search covers the entire session — messages, tool
input/output, and thinking — and reveals a folded item containing a match. On
terminals narrower than 90 columns, only the focused pane is shown.

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
