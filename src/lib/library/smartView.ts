/**
 * Smart-view rule metadata (M4): which fields exist, which operators apply
 * per field type, and what value input each (field, op) pair wants. Shared
 * by the rule builder (SmartViewEditor) so the dropdowns and the coercion in
 * `buildDef` can never disagree. Pure data + helpers — no IPC, no state.
 */

import type { SmartField, SmartOp, SmartSortField, SmartViewDef } from "./ipc";

export type FieldTypeName = "string" | "number" | "date";

export interface SmartFieldMeta {
  id: SmartField;
  label: string;
  type: FieldTypeName;
}

/** In tree order: text fields, then numeric, then dates. */
export const SMART_FIELDS: SmartFieldMeta[] = [
  { id: "title", label: "Title", type: "string" },
  { id: "artist", label: "Artist", type: "string" },
  { id: "album", label: "Album", type: "string" },
  { id: "genre", label: "Genre", type: "string" },
  { id: "path", label: "Path", type: "string" },
  { id: "source", label: "Source", type: "string" },
  { id: "year", label: "Year", type: "number" },
  { id: "rating", label: "Rating", type: "number" },
  { id: "playCount", label: "Play count", type: "number" },
  { id: "durationSecs", label: "Duration (sec)", type: "number" },
  { id: "bitrateKbps", label: "Bitrate (kbps)", type: "number" },
  { id: "lastPlayed", label: "Last played", type: "date" },
  { id: "dateAdded", label: "Date added", type: "date" },
];

const FIELD_BY_ID = new Map<SmartField, SmartFieldMeta>(SMART_FIELDS.map((f) => [f.id, f]));

export interface SmartOpMeta {
  id: SmartOp;
  label: string;
}

export const STRING_OPS: SmartOpMeta[] = [
  { id: "is", label: "is" },
  { id: "isNot", label: "is not" },
  { id: "contains", label: "contains" },
  { id: "notContains", label: "does not contain" },
  { id: "startsWith", label: "starts with" },
  { id: "endsWith", label: "ends with" },
];

export const NUMBER_OPS: SmartOpMeta[] = [
  { id: "eq", label: "is" },
  { id: "ne", label: "is not" },
  { id: "gt", label: "is greater than" },
  { id: "gte", label: "is at least" },
  { id: "lt", label: "is less than" },
  { id: "lte", label: "is at most" },
  { id: "between", label: "is between" },
];

export const DATE_OPS: SmartOpMeta[] = [
  { id: "inLastDays", label: "in the last (days)" },
  { id: "notInLastDays", label: "not in the last (days)" },
  { id: "before", label: "before" },
  { id: "after", label: "after" },
];

export function fieldTypeOf(field: SmartField): FieldTypeName {
  return FIELD_BY_ID.get(field)?.type ?? "string";
}

export function fieldLabel(field: SmartField): string {
  return FIELD_BY_ID.get(field)?.label ?? field;
}

/** Operators applicable to a field, per its type. */
export function opsForField(field: SmartField): SmartOpMeta[] {
  switch (fieldTypeOf(field)) {
    case "number":
      return NUMBER_OPS;
    case "date":
      return DATE_OPS;
    default:
      return STRING_OPS;
  }
}

export function defaultOpFor(field: SmartField): SmartOp {
  return opsForField(field)[0].id;
}

/** What the value input for a (field, op) pair is: text / number / range / date. */
export type ValueInputKind = "text" | "number" | "range" | "date";

export function inputKindOf(field: SmartField, op: SmartOp): ValueInputKind {
  const type = fieldTypeOf(field);
  if (type === "string") return "text";
  if (type === "number") return op === "between" ? "range" : "number";
  return op === "inLastDays" || op === "notInLastDays" ? "number" : "date";
}

export const SMART_SORTS: { id: SmartSortField; label: string }[] = [
  { id: "title", label: "Title" },
  { id: "artist", label: "Artist" },
  { id: "album", label: "Album" },
  { id: "random", label: "Random" },
  { id: "mostPlayed", label: "Most played" },
  { id: "recentlyAdded", label: "Recently added" },
  { id: "recentlyPlayed", label: "Recently played" },
];

/** A sensible starting def for the "+ NEW SMART VIEW" entry point. */
export function emptySmartViewDef(): SmartViewDef {
  return {
    name: "",
    group: {
      match: "all",
      rules: [{ field: "artist", op: "contains", value: "" }],
    },
    limit: null,
    sort: "title",
    sortDesc: false,
  };
}
