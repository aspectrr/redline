# email-for-agents

A local email client + library for teaching coding agents (pi, Claude Code, Cursor, …) to write emails in your voice, by learning from **(draft → final)** revision pairs.

The whole point is the loop: the agent drafts, you edit, the diff is captured and searchable, and every revision is restorable from one place. The *reasoning* (deriving voice rules from a diff) stays in the agent session — no LLM call leaves your agent.

## Two surfaces, one library, one DB

```
src/lib.rs                 shared library: schema, diff, pairs, lessons, drafts, revisions
src/main.rs (email-learn)  CLI — learning loop + agent ingest
email-app/                 Tauri 2 desktop client (React + TS + Vite)
~/.email-learn/emails.db   one shared SQLite DB (WAL) — CLI and app read/write the same file
```

The CLI and the Tauri app both call into `email_learn` (the library), so there is exactly one implementation of the data model and the diffing. Overriding the DB path: `EMAIL_LEARN_DB=/abs/path/emails.db`.

## Data model

- **`pairs`** — a completed (draft, final, diff) with context + tags. The learning corpus.
- **`lessons`** — concrete voice rules an agent derived from a pair's diff.
- **`drafts`** — an in-flight draft the agent created, not yet finalized.
- **`draft_revisions`** — append-only content history per draft. This is "restore in one place": every save is a row, restore appends a new revision copying an old one, and history is never destroyed.

`finalize` = latest revision becomes the final, the first revision is the agent's original, the diff is computed, a `pairs` row is written, and the draft is linked to it.

## Run the app

```bash
cd email-app
bun install          # frontend deps
bun run tauri dev    # launches the desktop window (builds the Rust shell on first run)
```

The app has three views:
- **Drafts** — inbox of agent-pushed drafts on the left, editor in the middle (autosaves a revision ~1.2s after you stop typing), and a right pane with tabs for the **live diff** (original → current), **revisions** (restore any version), and **lessons**.
- **Library** — finalized pairs with side-by-side draft/final/diff, plus the lessons list.
- **Search** — across every draft, revision, pair, and lesson.

## Agent ingest

The agent pushes a draft into the app via the CLI:

```bash
email-learn draft body.txt --context "cold intro to investor" --tags pitch,external
```

It then appears in the app's Drafts inbox for you to edit. (A small MCP server exposing `create_draft` / `finalize` / `query` is the planned next step — see Roadmap.)

## CLI

```
# learning loop
email-learn add <draft> <final> --context "<one line>" --tags a,b      # store a pair → prints id
email-learn show <id>                                                  # draft + final + diff
email-learn recent [N]                                                 # N most recent pairs
email-learn lessons [--tags a,b]                                       # stored voice lessons
email-learn add-lesson <pair_id> "<lesson>" --tags a,b                 # record a derived rule
email-learn query "<needle>"                                           # LIKE search pairs + lessons
email-learn export                                                     # everything as markdown
email-learn summarize                                                  # optional LLM seam (noop stub today)

# drafting surface (agent ingest)
email-learn draft <file|-> --context "<one line>" --tags a,b [--source agent]   # → draft id
email-learn finalize <draft_id>                                                # → pair id
email-learn drafts [--all]
```

Install the CLI on its own: `cargo install --path .` (puts `email-learn` on `$PATH`).

## Use it as an agent skill

The skill lives in `skills/email-voice/SKILL.md`. Symlink it into your agent's skills dir:

```bash
ln -s "$PWD/skills/email-voice" ~/.pi/agent/skills/email-voice
```

Then any pi agent can load it by name (`email-voice`) and follow its draft → diff → lesson workflow.

## Roadmap

- **MCP server** (`email-mcp`) so pi/Claude can call `create_draft` / `finalize` / `query` as tools instead of shelling out to the CLI.
- **LLM provider** behind `EMAIL_LEARN_LLM` — wire the `LessonSummarizer` seam to ollama / openai / an MCP provider for lesson summarization and `/style-review`-style audits. Lesson *derivation* stays in the agent session either way.

## License

MIT
