---
name: redline
description: Write emails that sound like the user by learning from (draft → final) revisions. Use when drafting, revising, or reviewing emails; and whenever the user shares a draft alongside the version they actually sent. Stores pairs, lessons, and matchable patterns in a local SQLite db via the `redline` CLI or MCP server.
---

# redline

Make outbound emails sound like the user. The system learns from **(draft, final)** revision pairs and automatically lints future drafts against stored voice patterns.

## Where the data lives

- DB: `~/.redline/emails.db` (shared by CLI, MCP server, and Tauri app). Override with `REDLINE_DB=/path/db`.
- CLI: `redline` (on PATH)
- MCP server: `redline mcp` (stdio, auto-configured in pi)
- Tauri app: `cd redline-app && bun run tauri dev`

No LLM calls from the CLI/server. You do all reasoning in-session.

## The single workflow

When asked to write or revise an email, follow this loop. One pass through it = full cycle.

### 1. Create the draft (patterns injected automatically)

```
redline draft <file> --context "topic + recipient type" --tags pitch,external
```

This returns the draft id **PLUS all stored voice patterns and any lint violations**. The patterns are your constraints — they represent what the user's voice does and doesn't do. Fix violations with `save_revision` before showing the draft to the user.

If using MCP, `create_draft` returns the same data. No separate lint call needed.

**If there are violations**: rewrite to resolve them, save the revision, check the returned violations again. Repeat until clean.

### 2. Hand off to the user

Tell the user the draft is ready in the app. They will edit it there. You don't control this step — wait for them to tell you they're done (or check `redline drafts` to see status).

### 3. Finalize (analysis + promotion returned automatically)

When the user has finalized the draft in the app:

```
redline finalize <draft_id>
```

Returns the pair id **PLUS diff analysis** (deletions, additions, word swaps, categorized changes, existing pattern hits) and any **auto-promoted patterns** (patterns that hit 3+ pairs auto-confirm).

### 4. Derive lessons from the analysis

Read the analysis. Focus on:

- **Deletions** — what got cut entirely (strongest voice signal).
- **Categorized changes** — each hunk tagged as deletion, structural, stylistic, factual, or punctuation.
- **Word swaps** — specific before→after replacements.

Derive 1–3 concrete lessons per pair. A good lesson is specific, actionable, and voice-coded:

- ✅ "No em-dashes in client emails" → store as a pattern
- ✅ "Use 'quick note' not 'I wanted to reach out'" → store as pattern + lesson
- ❌ "Be clear and professional" → useless, reject it

Store each lesson: `redline add-lesson <pair_id> "<lesson>" --tags pitch,external`
Store a matchable pattern: `redline add-pattern --rule "<rule>" --pattern "<match>" --category style`

**Always create a pattern alongside a lesson.** Lessons without patterns don't lint. That's the write loop — patterns catch voice issues in future drafts automatically.

## Key tools

| CLI | MCP | Purpose |
|---|---|---|
| `draft` | `create_draft` | Write draft, get patterns + lint violations back |
| `finalize` | `finalize_draft` | Finalize pair, get diff analysis + promotions back |
| `analyze <pair_id>` | `analyze_diff` | Deep-dive: deletions, categorized changes, swaps, hits |
| `add-pattern` | `add_pattern` | Create matchable voice pattern (literal or regex) |
| `list-patterns` | `list_patterns` | See all patterns the lint engine uses |
| `promote` | — | Manually trigger pattern promotion |
| `add-lesson` | `add_lesson` | Store a derived voice lesson |
| `lessons` | `list_lessons` | Read all lessons |
| `show <id>` | `show_pair` | Read a pair with diff (lines/sentences/side-by-side) |
| `recent N` | `recent_pairs` | Skim recent finals |
| `feedback` | `give_feedback` | Log what's painful (bugs, suggestions) |

## Pattern types

- **literal** — case-insensitive substring match. Pattern `"—"` catches any em-dash.
- **regex** — Rust regex syntax. Pattern `"^.{1,10}$"` catches terse one-liners.

Directions:
- **avoid** — flag if pattern is **found** in the draft ("⚠ em-dash detected")
- **prefer** — flag if pattern is **absent** ("consider using 'quick note'")

Categories: `punctuation`, `style`, `structure`, `factual`, `deletion`.

Patterns auto-promote from `unconfirmed` → `confirmed` after appearing in 3+ pairs' drafts. This runs automatically on `finalize`.

## Conventions

- Tags: lowercase, comma-separated. Vocab: `pitch`, `followup`, `external`, `internal`, `apology`, `decline`, `intro`.
- Context line: recipient type + intent, e.g. `"cold intro to investor"`.
- One ask per email is near-universal — default to it unless a pair teaches otherwise.

## Failure modes

- **Over-fitting to one pair.** Confirm across 2–3 before treating as a strong rule. Auto-promotion handles this.
- **Generic lessons.** If you can't name *what specifically changed*, don't store it.
- **Stale voice.** Re-check `recent` periodically. If a final contradicts an old lesson, flag it.
- **Silent edits.** Never delete lessons/patterns without surfacing the change.
- **Write-only graveyard.** Always create patterns alongside lessons — lessons without patterns don't lint.
