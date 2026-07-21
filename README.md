# redline

A local email client + library for teaching coding agents (pi, Claude Code, Cursor, …) to write emails in your voice, by learning from **(draft → final)** revision pairs.

The whole point is the loop: the agent drafts, you edit, the diff is captured and searchable, and every revision is restorable from one place. The *reasoning* (deriving voice rules from a diff) stays in the agent session — no LLM call leaves your agent.

## Three surfaces, one library, one DB

```
src/lib.rs                 shared library: schema, diff, pairs, lessons, drafts, revisions
src/mcp.rs                 MCP server (stdio) — every CLI feature as tools
src/main.rs (redline)  CLI — learning loop, agent ingest, and `mcp` subcommand
redline-app/                 Tauri 2 desktop client (React + TS + Vite)
~/.redline/emails.db   one shared SQLite DB (WAL) — CLI, MCP server, and app all read/write it
```

The CLI and the Tauri app both call into `redline` (the library), so there is exactly one implementation of the data model and the diffing. Overriding the DB path: `REDLINE_DB=/abs/path/emails.db`.

## Data model

- **`pairs`** — a completed (draft, final, diff) with context + tags. The learning corpus.
- **`lessons`** — concrete voice rules an agent derived from a pair's diff.
- **`drafts`** — an in-flight draft the agent created, not yet finalized.
- **`draft_revisions`** — append-only content history per draft. This is "restore in one place": every save is a row, restore appends a new revision copying an old one, and history is never destroyed.

`finalize` = latest revision becomes the final, the first revision is the agent's original, the diff is computed, a `pairs` row is written, and the draft is linked to it.

## Run the app

```bash
cd redline-app
bun install          # frontend deps
bun run tauri dev    # launches the desktop window (builds the Rust shell on first run)
```

The app has three views:
- **Drafts** — inbox of agent-pushed drafts on the left, editor in the middle (autosaves a revision ~1.2s after you stop typing), and a right pane with tabs for the **live diff** (original → current), **revisions** (restore any version), and **lessons**.
- **Library** — finalized pairs with side-by-side draft/final/diff, plus the lessons list.
- **Search** — across every draft, revision, pair, and lesson.

## Agent ingest

The agent pushes a draft two ways:

**CLI** (one-shot):
```bash
redline draft body.txt --context "cold intro to investor" --tags pitch,external
```

**MCP server** (preferred for agents) — connect once and call tools directly (no subprocess per call). See [MCP server](#mcp-server) below.

Either way the draft appears in the app's Drafts inbox for you to edit.

## CLI

```
# learning loop
redline add <draft> <final> --context "<one line>" --tags a,b      # store a pair → prints id
redline show <id>                                                  # draft + final + diff
redline recent [N]                                                 # N most recent pairs
redline lessons [--tags a,b]                                       # stored voice lessons
redline add-lesson <pair_id> "<lesson>" --tags a,b                 # record a derived rule
redline query "<needle>"                                           # LIKE search pairs + lessons
redline export                                                     # everything as markdown
redline summarize                                                  # optional LLM seam (noop stub today)

# drafting surface (agent ingest)
redline draft <file|-> --context "<one line>" --tags a,b [--source agent]   # → draft id
redline finalize <draft_id>                                                # → pair id
redline drafts [--all]
redline delete-draft <draft_id>                                            # remove a draft (keeps any finalized pair)

# MCP server (stdio) — agents connect and call tools
redline mcp                                                                # speaks JSON-RPC over stdio
```

Install the CLI on its own: `cargo install --path .` (puts `redline` on `$PATH`).

## MCP server

`redline mcp` runs an MCP server over stdio exposing the full surface as tools — so a coding agent (pi, Claude, Cursor) can read pairs/diffs/lessons, record derived lessons, push and edit drafts, and search, all over one connection instead of shelling out per call. Built on [rmcp](https://crates.io/crates/rmcp) (the official Rust MCP SDK).

**17 tools:** `add_pair`, `show_pair`, `recent_pairs`, `list_lessons`, `add_lesson`, `query`, `search`, `export`, `summarize`, `create_draft`, `get_draft`, `list_drafts`, `save_revision`, `restore_revision`, `finalize_draft`, `delete_draft`, `update_draft_meta`. Diffs come back as plain unified text; tool-level failures return caller-visible errors (the agent sees the message).

Add it to an agent config (e.g. pi's `~/.pi/config.*` or Claude Desktop's `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "email": {
      "command": "redline",
      "args": ["mcp"]
    }
  }
}
```

The same `REDLINE_DB` override applies (point multiple agents at the same DB). Lesson *derivation* still happens in the agent session — the server only stores and retrieves.

## Use it as an agent skill

The skill lives in `skills/redline/SKILL.md`. Symlink it into your agent's skills dir:

```bash
ln -s "$PWD/skills/redline" ~/.pi/agent/skills/redline
```

Then any pi agent can load it by name (`redline`) and follow its draft → diff → lesson workflow.

## Roadmap

- **LLM provider** behind `REDLINE_LLM` — wire the `LessonSummarizer` seam to ollama / openai / an MCP provider for lesson summarization and `/style-review`-style audits. Lesson *derivation* stays in the agent session either way.

## License

MIT
