/**
 * Smart-view rule metadata (M4): which fields exist, which operators apply
 * per field type, and what value input each (field, op) pair wants. Shared
 * by the rule builder (SmartViewEditor) so the dropdowns and the coercion in
 * `buildDef` can never disagree. Pure data + helpers — no IPC, no state.
 */

import type { RuleRow, SmartField, SmartOp, SmartSortField, SmartViewDef } from "./ipc";

export type FieldTypeName = "string" | "number" | "date" | "boolean";

export interface SmartFieldMeta {
  id: SmartField;
  label: string;
  type: FieldTypeName;
}

/** In tree order: text fields, then numeric, then dates. */
export const SMART_FIELDS: SmartFieldMeta[] = [
  { id: "title", label: "Title", type: "string" },
  { id: "artist", label: "Artist", type: "string" },
  { id: "albumArtist", label: "Album artist", type: "string" },
  { id: "album", label: "Album", type: "string" },
  { id: "genre", label: "Genre", type: "string" },
  { id: "codec", label: "Codec", type: "string" },
  { id: "path", label: "Path", type: "string" },
  { id: "source", label: "Source", type: "string" },
  { id: "server", label: "Server id", type: "string" },
  { id: "starred", label: "Starred", type: "boolean" },
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

export const BOOLEAN_OPS: SmartOpMeta[] = [
  { id: "is", label: "is" },
  { id: "isNot", label: "is not" },
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
    case "boolean":
      return BOOLEAN_OPS;
    default:
      return STRING_OPS;
  }
}

export function defaultOpFor(field: SmartField): SmartOp {
  return opsForField(field)[0].id;
}

/** Keep a stored operator only when the field's type still admits it. */
export function normalizeOp(field: SmartField, op: SmartOp): SmartOp {
  return opsForField(field).some((candidate) => candidate.id === op)
    ? op
    : defaultOpFor(field);
}

/** What the value input for a (field, op) pair is: text / number / range / date / boolean. */
export type ValueInputKind = "text" | "number" | "range" | "date" | "boolean";

export function inputKindOf(field: SmartField, op: SmartOp): ValueInputKind {
  const type = fieldTypeOf(field);
  if (type === "string") return "text";
  if (type === "number") return op === "between" ? "range" : "number";
  if (type === "boolean") return "boolean";
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

/** Editable rule row shared by the Smart View editor and Advanced Search. */
export interface EditableRule {
  field: SmartField;
  op: SmartOp;
  valueRaw: string;
  value2Raw: string;
}

/** A blank row with an operator/value shaped for the field's type. */
export function blankRule(field: SmartField = "artist"): EditableRule {
  const op = defaultOpFor(field);
  return {
    field,
    op,
    valueRaw: inputKindOf(field, op) === "boolean" ? "1" : "",
    value2Raw: "",
  };
}

export type RuleCoercion =
  | { ok: true; rows: RuleRow[] }
  | { ok: false; errors: Record<number, string> };

/**
 * Coerce raw editor rows into the wire contract. Pure and shared, so the
 * Smart View editor and the Advanced Search builder can never disagree.
 */
export function coerceRuleRows(rules: EditableRule[]): RuleCoercion {
  const errors: Record<number, string> = {};
  const rows: RuleRow[] = [];
  for (let i = 0; i < rules.length; i++) {
    const r = rules[i];
    const label = fieldLabel(r.field);
    const type = fieldTypeOf(r.field);
    if (type === "string") {
      if (r.valueRaw.trim() === "") {
        errors[i] = `${label} needs a value`;
        continue;
      }
      rows.push({ field: r.field, op: r.op, value: r.valueRaw });
    } else if (type === "boolean") {
      if (r.valueRaw !== "1" && r.valueRaw !== "0") {
        errors[i] = `${label} needs true or false`;
        continue;
      }
      rows.push({ field: r.field, op: r.op, value: Number(r.valueRaw) });
    } else if (type === "number") {
      const v = Number(r.valueRaw);
      if (r.valueRaw.trim() === "" || !Number.isFinite(v)) {
        errors[i] = `${label} needs a number`;
        continue;
      }
      if (r.op === "between") {
        const v2 = Number(r.value2Raw);
        if (r.value2Raw.trim() === "" || !Number.isFinite(v2)) {
          errors[i] = "range needs an upper bound";
          continue;
        }
        rows.push({ field: r.field, op: r.op, value: v, value2: v2 });
      } else {
        rows.push({ field: r.field, op: r.op, value: v });
      }
    } else {
      // date field
      if (r.op === "inLastDays" || r.op === "notInLastDays") {
        const v = Number(r.valueRaw);
        if (r.valueRaw.trim() === "" || !Number.isFinite(v) || v < 0) {
          errors[i] = "days needs a non-negative number";
          continue;
        }
        rows.push({ field: r.field, op: r.op, value: v });
      } else {
        if (r.valueRaw === "") {
          errors[i] = "pick a date";
          continue;
        }
        rows.push({ field: r.field, op: r.op, value: r.valueRaw });
      }
    }
  }
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return { ok: true, rows };
}

/** Empty definition for the ad-hoc Advanced Search (no name, wide limit). */
export function emptySearchDef(): SmartViewDef {
  return {
    name: "",
    group: { match: "all", rules: [] },
    limit: 1000,
    sort: "title",
    sortDesc: false,
  };
}

const OP_TEXT: Partial<Record<SmartOp, string>> = {
  is: "=",
  isNot: "≠",
  contains: "contains",
  notContains: "does not contain",
  startsWith: "starts with",
  endsWith: "ends with",
  eq: "=",
  ne: "≠",
  gt: ">",
  gte: "≥",
  lt: "<",
  lte: "≤",
  between: "between",
  inLastDays: "in the last",
  notInLastDays: "not in the last",
  before: "before",
  after: "after",
};

function displayValue(rule: RuleRow): string {
  if (rule.field === "starred") return Number(rule.value) === 1 ? "true" : "false";
  return typeof rule.value === "string" ? `“${rule.value}”` : String(rule.value);
}

/** Plain-English rendering of a rule group (AI banner / rule summary). */
export function plainRules(def: SmartViewDef): string {
  const rules = def.group.rules;
  if (rules.length === 0) return "everything";
  return rules
    .map((rule) => {
      const label = fieldLabel(rule.field);
      const op = OP_TEXT[rule.op] ?? rule.op;
      if (rule.op === "between" && rule.value2 != null) {
        return `${label} between ${displayValue(rule)} and ${
          typeof rule.value2 === "string" ? `“${rule.value2}”` : rule.value2
        }`;
      }
      if (rule.op === "inLastDays" || rule.op === "notInLastDays") {
        return `${label} ${op} ${displayValue(rule)} day(s)`;
      }
      return `${label} ${op} ${displayValue(rule)}`;
    })
    .join(def.group.match === "all" ? " AND " : " OR ");
}
