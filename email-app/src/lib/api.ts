// Typed wrappers over the Tauri commands in email-app/src-tauri/src/lib.rs.
// Command names are exactly the snake_case Rust fn names.
import { invoke } from "@tauri-apps/api/core";

export interface Pair {
  id: number;
  draft: string;
  final: string; // serde rename of `final_`
  diff: string;
  context: string | null;
  tags: string[];
  created_at: string;
}

export interface Lesson {
  id: number;
  pair_id: number | null;
  lesson: string;
  tags: string[];
  created_at: string;
}

export interface Draft {
  id: number;
  context: string | null;
  tags: string[];
  status: "draft" | "finalized" | string;
  finalized_pair_id: number | null;
  created_at: string;
  updated_at: string;
}

export interface DraftRevision {
  id: number;
  draft_id: number;
  content: string;
  source: string;
  created_at: string;
}

export interface DraftWithRevisions {
  draft: Draft;
  revisions: DraftRevision[];
  working_diff: string;
}

export interface DiffSegment {
  tag: "add" | "del" | "ctx";
  text: string;
}

export interface DiffRow {
  kind: "equal" | "removed" | "added";
  segments: DiffSegment[];
}

/** Parse a stored diff (structured JSON from Rust rich_diff) into renderable rows. */
export function parseDiff(json: string): DiffRow[] {
  if (!json || !json.trim()) return [];
  try {
    const rows = JSON.parse(json) as DiffRow[];
    return Array.isArray(rows) ? rows : [];
  } catch {
    return [];
  }
}

export interface SearchResult {
  drafts: Draft[];
  pairs: Pair[];
  lessons: Lesson[];
}

export interface Pattern {
  id: number;
  lesson_id: number | null;
  rule: string;
  pattern: string;
  pattern_type: string;
  direction: string;
  category: string;
  before_text: string | null;
  after_text: string | null;
  confidence: string;
  created_at: string;
}

export interface Violation {
  pattern_id: number;
  lesson_id: number | null;
  rule: string;
  category: string;
  direction: string;
  matched_text: string;
  context: string;
  line: number;
}

export interface Feedback {
  id: number;
  tool_name: string | null;
  message: string;
  severity: string;
  rating: number | null;
  agent_id: string | null;
  created_at: string;
}

export interface CategorizedChange {
  category: string;
  description: string;
  before: string;
  after: string;
}

export interface DiffAnalysis {
  pair_id: number;
  deletions: string[];
  additions: string[];
  word_swaps: [string, string][];
  categorized: CategorizedChange[];
  draft_pattern_hits: { pattern_id: number; rule: string }[];
  final_pattern_hits: { pattern_id: number; rule: string }[];
}

export const api = {
  listDrafts: (includeFinalized = false) =>
    invoke<Draft[]>("list_drafts", { includeFinalized }),
  getDraft: (id: number) => invoke<DraftWithRevisions | null>("get_draft", { id }),
  createDraft: (content: string, context: string | null, tags: string[], source = "agent") =>
    invoke<number>("create_draft", { content, context, tags, source }),
  saveRevision: (draftId: number, content: string, source = "user") =>
    invoke<number>("save_revision", { draftId, content, source }),
  restoreRevision: (draftId: number, revisionId: number) =>
    invoke<number>("restore_revision", { draftId, revisionId }),
  finalizeDraft: (draftId: number) => invoke<number>("finalize_draft", { draftId }),
  deleteDraft: (draftId: number) => invoke<void>("delete_draft", { draftId }),
  deletePair: (pairId: number) => invoke<void>("delete_pair", { pairId }),
  deleteLesson: (lessonId: number) => invoke<void>("delete_lesson", { lessonId }),
  updateDraftMeta: (draftId: number, context: string | null, tags: string[]) =>
    invoke<void>("update_draft_meta", { draftId, context, tags }),
  listPairs: (limit = 100) => invoke<Pair[]>("list_pairs", { limit }),
  showPair: (id: number) => invoke<Pair | null>("show_pair", { id }),
  listLessons: (tags: string[] = []) => invoke<Lesson[]>("list_lessons", { tags }),
  addLesson: (pairId: number, lesson: string, tags: string[]) =>
    invoke<number>("add_lesson", { pairId, lesson, tags }),
  search: (needle: string) => invoke<SearchResult>("search", { needle }),
  lintDraft: (content: string) => invoke<Violation[]>("lint_draft", { content }),
  listPatterns: () => invoke<Pattern[]>("list_patterns"),
  addPattern: (
    rule: string, pattern: string, patternType: string, direction: string, category: string,
    lessonId: number | null, beforeText: string | null, afterText: string | null,
  ) => invoke<number>("add_pattern", {
    rule, pattern, patternType, direction, category, lessonId, beforeText, afterText,
  }),
  deletePattern: (patternId: number) => invoke<void>("delete_pattern", { patternId }),
  listFeedback: () => invoke<Feedback[]>("list_feedback"),
  computeDiff: (old: string, newText: string, mode?: string) =>
    invoke<string>("compute_diff", { old, new: newText, mode }),
  analyzePair: (pairId: number) => invoke<DiffAnalysis | null>("analyze_pair", { pairId }),
};
