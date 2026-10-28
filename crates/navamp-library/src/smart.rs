//! Smart Views — Winamp3-style smart playlists evaluated live against the
//! unified `tracks` table.
//!
//! Three pieces live here:
//! - the serde rule model (the frozen IPC contract: `camelCase`, unknown
//!   fields and enum values rejected so frontend typos fail loudly at the
//!   boundary instead of silently matching nothing);
//! - a rule → SQL compiler: field names map to columns through an
//!   exhaustive whitelist and **every** value is a bound parameter, so
//!   rule input can never become SQL text;
//! - persistence (the `smart_views` table) and evaluation, reusing the
//!   track row mapping from [`crate::query`].
//!
//! Matching semantics (one place, on purpose):
//! - Text rules are case-insensitive via `LOWER()` applied to **both**
//!   operands — SQLite's `LOWER` only folds ASCII, but because the same
//!   function runs on both sides, non-ASCII text still matches itself.
//! - `contains` / `startsWith` / `endsWith` compile to `LIKE` with `%` and
//!   `_` escaped (`ESCAPE '\'`), so user values match literally — a value
//!   of `%` finds titles containing a percent sign, not everything.
//! - Numeric rules accept numbers or numeric strings (`1993` and `"1993"`);
//!   text rules likewise stringify number values. NULL cells (unknown
//!   year/bitrate, never-played) fail every comparison — **except**
//!   `notInLastDays` on `lastPlayed`, where "never played" reasonably
//!   counts as "not played in the last N days".
//! - `last_played_at` / `date_added` are unix **seconds** (see the
//!   `tracks` schema); date rules compute thresholds in the same unit and
//!   `before`/`after` parse `YYYY-MM-DD` as UTC midnight in that unit.
//! - Sorting: `sortDesc` reverses only the primary key (ties stay in
//!   ascending library order, so pages are deterministic); text keys use
//!   `COLLATE NOCASE`; `random` ignores direction by construction.

use std::fmt;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::Track;
use crate::query::{track_columns, track_from_row};
use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Model (frozen IPC contract)
// ---------------------------------------------------------------------------

/// A rule value: `string | number` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RuleValue {
    Str(String),
    Num(f64),
}

impl<'de> Deserialize<'de> for RuleValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RuleValueVisitor;

        impl serde::de::Visitor<'_> for RuleValueVisitor {
            type Value = RuleValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or a number")
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<RuleValue, E> {
                Ok(RuleValue::Str(v.to_owned()))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<RuleValue, E> {
                Ok(RuleValue::Num(v as f64))
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<RuleValue, E> {
                Ok(RuleValue::Num(v as f64))
            }

            fn visit_f64<E>(self, v: f64) -> std::result::Result<RuleValue, E>
            where
                E: serde::de::Error,
            {
                if v.is_finite() {
                    Ok(RuleValue::Num(v))
                } else {
                    Err(E::custom("rule values must be finite numbers"))
                }
            }
        }
        deserializer.deserialize_any(RuleValueVisitor)
    }
}

impl RuleValue {
    /// The number this value denotes for a numeric/date rule. Numeric
    /// strings are accepted (`"1993"`); anything else is `None`.
    fn as_number(&self) -> Option<f64> {
        match self {
            RuleValue::Num(n) if n.is_finite() => Some(*n),
            RuleValue::Str(s) => s.trim().parse().ok(),
            RuleValue::Num(_) => None,
        }
    }

    /// The text this value denotes for a text rule (numbers format
    /// minimally: `42`, `42.5`).
    fn as_text(&self) -> String {
        match self {
            RuleValue::Str(s) => s.clone(),
            RuleValue::Num(n) => format_number(*n),
        }
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A rule field: which track attribute a row constrains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Title,
    Artist,
    Album,
    Genre,
    Path,
    Source,
    Year,
    Rating,
    PlayCount,
    DurationSecs,
    BitrateKbps,
    LastPlayed,
    DateAdded,
}

/// What comparisons a field admits — decides op validity and compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Text,
    Number,
    Date,
}

impl Field {
    /// The `tracks` column for this field — from this exhaustive match,
    /// never from user input.
    fn column(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Path => "path",
            Field::Source => "source",
            Field::Year => "year",
            Field::Rating => "rating",
            Field::PlayCount => "play_count",
            Field::DurationSecs => "duration_secs",
            Field::BitrateKbps => "bitrate_kbps",
            Field::LastPlayed => "last_played_at",
            Field::DateAdded => "date_added",
        }
    }

    fn kind(self) -> FieldKind {
        match self {
            Field::Title
            | Field::Artist
            | Field::Album
            | Field::Genre
            | Field::Path
            | Field::Source => FieldKind::Text,
            Field::Year
            | Field::Rating
            | Field::PlayCount
            | Field::DurationSecs
            | Field::BitrateKbps => FieldKind::Number,
            Field::LastPlayed | Field::DateAdded => FieldKind::Date,
        }
    }
}

/// A rule operator (the union over all field kinds; validity is checked
/// per field kind with an error naming the field, op and rule index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Between,
    InLastDays,
    NotInLastDays,
    Before,
    After,
}

/// How the rules of a group combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    All,
    Any,
}

/// One rule: `field op value [value2]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleRow {
    pub field: Field,
    pub op: Op,
    pub value: RuleValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value2: Option<RuleValue>,
}

/// A flat group of rules combined by `match: "all" | "any"`. An empty
/// group matches every track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleGroup {
    #[serde(rename = "match")]
    pub mode: MatchMode,
    pub rules: Vec<RuleRow>,
}

/// How a smart view's results are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortField {
    Title,
    Artist,
    Album,
    Random,
    MostPlayed,
    RecentlyAdded,
    RecentlyPlayed,
}

/// A smart view definition, exactly as it crosses the IPC boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SmartViewDef {
    pub name: String,
    pub group: RuleGroup,
    /// Max rows (`null` = unlimited when run; previews cap at 500).
    pub limit: Option<f64>,
    pub sort: SortField,
    pub sort_desc: bool,
}

/// A saved smart view row: the storage id plus its definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedSmartView {
    pub id: i64,
    pub def: SmartViewDef,
}

// ---------------------------------------------------------------------------
// Compiler: RuleGroup -> SQL with bound params only
// ---------------------------------------------------------------------------

/// Largest LIMIT a definition may request at all (rejected at save *and*
/// preview time so a saved view can never carry a bigger one).
const MAX_LIMIT: f64 = 5000.0;
/// Hard row cap for previews.
const PREVIEW_CAP: i64 = 500;
const SECS_PER_DAY: f64 = 86_400.0;

/// What a compilation is for — only the effective LIMIT differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// A saved view: `limit: null` means unlimited.
    Run,
    /// A preview: `limit: null` means 500, and any limit is capped at 500.
    Preview,
}

/// A validated definition, ready to evaluate.
struct Compiled {
    /// `None` = no WHERE clause (match everything).
    where_sql: Option<String>,
    params: Vec<Value>,
    order_sql: String,
    limit: Option<i64>,
}

/// The IPC spelling of a field/op, for error messages — derived from the
/// serde renames so it can never drift from the wire contract.
fn ipc_label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `rules[i]: field `f`: op `o`: <msg>` — names all three, as the UI
/// shows these inline next to the offending row.
fn invalid(ctx: &str, msg: impl fmt::Display) -> Error {
    Error::InvalidSmartView(format!("{ctx}: {msg}"))
}

fn rule_context(rule: &RuleRow, index: usize) -> String {
    format!(
        "rules[{index}]: field `{}`: op `{}`",
        ipc_label(&rule.field),
        ipc_label(&rule.op)
    )
}

/// A bound parameter for a numeric rule: integral values bind as INTEGER
/// (clean affinity against the INTEGER columns), the rest as REAL.
fn num_value(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        Value::Integer(v as i64)
    } else {
        Value::Real(v)
    }
}

/// Escapes LIKE metacharacters so the value matches literally.
fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `now - days`, saturated to the i64 range (an absurd day count can't
/// overflow the comparison into an error).
fn days_ago_threshold(days: f64) -> i64 {
    let t = now_unix_secs() as f64 - days * SECS_PER_DAY;
    t.clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// Compiles one rule to a SQL condition, pushing its bound params.
fn compile_rule(rule: &RuleRow, index: usize, params: &mut Vec<Value>) -> Result<String> {
    let ctx = rule_context(rule, index);
    let col = rule.field.column();

    match rule.field.kind() {
        FieldKind::Text => {
            // Text fields take the general string ops (`source` included,
            // for uniformity, even though only is/isNot make sense there).
            let text = rule.value.as_text();
            let sql = match rule.op {
                Op::Is => {
                    params.push(Value::Text(text));
                    format!("LOWER({col}) = LOWER(?)")
                }
                Op::IsNot => {
                    params.push(Value::Text(text));
                    format!("LOWER({col}) <> LOWER(?)")
                }
                Op::Contains => {
                    params.push(Value::Text(format!("%{}%", like_escape(&text))));
                    format!("LOWER({col}) LIKE LOWER(?) ESCAPE '\\'")
                }
                Op::NotContains => {
                    params.push(Value::Text(format!("%{}%", like_escape(&text))));
                    format!("LOWER({col}) NOT LIKE LOWER(?) ESCAPE '\\'")
                }
                Op::StartsWith => {
                    params.push(Value::Text(format!("{}%", like_escape(&text))));
                    format!("LOWER({col}) LIKE LOWER(?) ESCAPE '\\'")
                }
                Op::EndsWith => {
                    params.push(Value::Text(format!("%{}", like_escape(&text))));
                    format!("LOWER({col}) LIKE LOWER(?) ESCAPE '\\'")
                }
                _ => {
                    return Err(invalid(
                        &ctx,
                        "text fields support is, isNot, contains, notContains, startsWith, endsWith",
                    ));
                }
            };
            Ok(sql)
        }
        FieldKind::Number => {
            let require = |v: &RuleValue| -> Result<f64> {
                v.as_number().ok_or_else(|| {
                    invalid(
                        &ctx,
                        format!("value must be a number, got `{}`", v.as_text()),
                    )
                })
            };
            let sql = match rule.op {
                Op::Eq => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} = ?")
                }
                Op::Ne => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} <> ?")
                }
                Op::Gt => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} > ?")
                }
                Op::Gte => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} >= ?")
                }
                Op::Lt => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} < ?")
                }
                Op::Lte => {
                    params.push(num_value(require(&rule.value)?));
                    format!("{col} <= ?")
                }
                Op::Between => {
                    let Some(v2) = &rule.value2 else {
                        return Err(invalid(&ctx, "between requires value2 (both bounds)"));
                    };
                    let (a, b) = (require(&rule.value)?, require(v2)?);
                    params.push(num_value(a));
                    params.push(num_value(b));
                    format!("{col} BETWEEN ? AND ?")
                }
                _ => {
                    return Err(invalid(
                        &ctx,
                        "number fields support eq, ne, gt, gte, lt, lte, between",
                    ));
                }
            };
            Ok(sql)
        }
        FieldKind::Date => {
            let sql = match rule.op {
                Op::InLastDays | Op::NotInLastDays => {
                    let Some(days) = rule.value.as_number() else {
                        return Err(invalid(&ctx, "value must be the number of days"));
                    };
                    if days <= 0.0 {
                        return Err(invalid(&ctx, "requires a number of days greater than 0"));
                    }
                    params.push(Value::Integer(days_ago_threshold(days)));
                    if rule.op == Op::InLastDays {
                        format!("{col} >= ?")
                    } else {
                        // "not played in the last N days" includes never
                        // played — NULL fails plain comparisons.
                        format!("({col} IS NULL OR {col} < ?)")
                    }
                }
                Op::Before | Op::After => {
                    let RuleValue::Str(text) = &rule.value else {
                        return Err(invalid(&ctx, "requires a `YYYY-MM-DD` string"));
                    };
                    let Some(secs) = parse_date(text) else {
                        return Err(invalid(
                            &ctx,
                            format!("cannot parse `{text}` as a `YYYY-MM-DD` date"),
                        ));
                    };
                    params.push(Value::Integer(secs));
                    if rule.op == Op::Before {
                        format!("{col} < ?")
                    } else {
                        format!("{col} > ?")
                    }
                }
                _ => {
                    return Err(invalid(
                        &ctx,
                        "date fields support inLastDays, notInLastDays, before, after",
                    ));
                }
            };
            Ok(sql)
        }
    }
}

/// Compiles a group: rules joined by AND/OR. Empty group → `None` (no
/// WHERE clause, matches all).
fn compile_group(group: &RuleGroup, params: &mut Vec<Value>) -> Result<Option<String>> {
    if group.rules.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::with_capacity(group.rules.len());
    for (index, rule) in group.rules.iter().enumerate() {
        parts.push(compile_rule(rule, index, params)?);
    }
    let joiner = match group.mode {
        MatchMode::All => " AND ",
        MatchMode::Any => " OR ",
    };
    Ok(Some(parts.join(joiner)))
}

/// Stable tie-breakers appended to every deterministic sort (the primary
/// key is excluded from the chain where it already heads it).
const ORDER_TAIL: &str =
    ", artist COLLATE NOCASE, album COLLATE NOCASE, track_no, title COLLATE NOCASE, path";

fn order_sql(sort: SortField, desc: bool) -> String {
    let head = |key: &str| {
        if desc {
            format!("{key} DESC")
        } else {
            key.to_owned()
        }
    };
    match sort {
        SortField::Title => format!("{}{ORDER_TAIL}", head("title COLLATE NOCASE")),
        SortField::Artist => format!(
            "{}, album COLLATE NOCASE, track_no, title COLLATE NOCASE, path",
            head("artist COLLATE NOCASE")
        ),
        SortField::Album => format!(
            "{}, artist COLLATE NOCASE, track_no, title COLLATE NOCASE, path",
            head("album COLLATE NOCASE")
        ),
        // Direction is meaningless for RANDOM(); not applied.
        SortField::Random => "RANDOM()".to_owned(),
        SortField::MostPlayed => format!("{}{ORDER_TAIL}", head("play_count")),
        SortField::RecentlyAdded => format!("{}{ORDER_TAIL}", head("date_added")),
        SortField::RecentlyPlayed => format!("{}{ORDER_TAIL}", head("last_played_at")),
    }
}

/// Validates `def` and compiles it for `purpose`.
fn compile_def(def: &SmartViewDef, purpose: Purpose) -> Result<Compiled> {
    if let Some(limit) = def.limit {
        if limit.fract() != 0.0 {
            return Err(Error::InvalidSmartView(format!(
                "limit must be a whole number, got {limit}"
            )));
        }
        if !(1.0..=MAX_LIMIT).contains(&limit) {
            let max = MAX_LIMIT as i64;
            return Err(Error::InvalidSmartView(format!(
                "limit must be between 1 and {max}, got {limit:.0}"
            )));
        }
    }
    let mut params = Vec::new();
    let where_sql = compile_group(&def.group, &mut params)?;
    let limit = match (purpose, def.limit) {
        (Purpose::Preview, Some(n)) => Some((n.min(PREVIEW_CAP as f64)) as i64),
        (Purpose::Preview, None) => Some(PREVIEW_CAP),
        (Purpose::Run, Some(n)) => Some(n as i64),
        (Purpose::Run, None) => None,
    };
    Ok(Compiled {
        where_sql,
        params,
        order_sql: order_sql(def.sort, def.sort_desc),
        limit,
    })
}

/// Runs a compiled view against `tracks`, reusing the shared row mapping.
fn execute(conn: &Connection, compiled: Compiled) -> rusqlite::Result<Vec<Track>> {
    let mut sql = format!("SELECT {} FROM tracks", track_columns(""));
    if let Some(where_sql) = &compiled.where_sql {
        sql.push_str(" WHERE ");
        sql.push_str(where_sql);
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(&compiled.order_sql);
    let mut params = compiled.params;
    if let Some(limit) = compiled.limit {
        sql.push_str(" LIMIT ?");
        params.push(Value::Integer(limit));
    }
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), track_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tracks)
}

// ---------------------------------------------------------------------------
// Storage + public operations (called through `Library`)
// ---------------------------------------------------------------------------

/// All saved views in tree order (`position`, then id).
pub(super) fn list_views(conn: &Connection) -> Result<Vec<SavedSmartView>> {
    let mut stmt = conn.prepare("SELECT id, def FROM smart_views ORDER BY position, id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, json)| {
            let def = serde_json::from_str(&json).map_err(|e| {
                Error::InvalidSmartView(format!(
                    "smart view {id}: stored definition is corrupt: {e}"
                ))
            })?;
            Ok(SavedSmartView { id, def })
        })
        .collect()
}

/// Validates `def` first (nothing invalid is ever persisted), then upserts
/// it: `Some(id)` updates that row — or inserts under that exact id if it
/// vanished — and `None` appends a new row. Returns the stored id.
pub(super) fn save_view(conn: &Connection, id: Option<i64>, def: &SmartViewDef) -> Result<i64> {
    compile_def(def, Purpose::Run)?;
    let json = serde_json::to_string(def)
        .map_err(|e| Error::InvalidSmartView(format!("cannot encode definition: {e}")))?;
    match id {
        Some(id) => {
            let updated = conn.execute(
                "UPDATE smart_views SET name = ?1, def = ?2 WHERE id = ?3",
                rusqlite::params![def.name, json, id],
            )?;
            if updated == 0 {
                conn.execute(
                    "INSERT INTO smart_views (id, name, def) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, def.name, json],
                )?;
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO smart_views (name, def) VALUES (?1, ?2)",
                rusqlite::params![def.name, json],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Deletes a view. Deleting an unknown id is a no-op (the UI deletes from
/// a list it just fetched; a race shouldn't turn into an error toast).
pub(super) fn delete_view(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM smart_views WHERE id = ?1", [id])?;
    Ok(())
}

/// Loads, revalidates and runs a saved view. Unknown id →
/// [`Error::SmartViewNotFound`].
pub(super) fn run_view(conn: &Connection, id: i64) -> Result<Vec<Track>> {
    let json: Option<String> = conn
        .query_row("SELECT def FROM smart_views WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(json) = json else {
        return Err(Error::SmartViewNotFound(id));
    };
    let def: SmartViewDef = serde_json::from_str(&json).map_err(|e| {
        Error::InvalidSmartView(format!(
            "smart view {id}: stored definition is corrupt: {e}"
        ))
    })?;
    let compiled = compile_def(&def, Purpose::Run)?;
    execute(conn, compiled).map_err(Error::from)
}

/// Validates and evaluates `def` without persisting anything. Hard cap of
/// [`PREVIEW_CAP`] rows (or the def's own smaller limit).
pub(super) fn preview(conn: &Connection, def: &SmartViewDef) -> Result<Vec<Track>> {
    let compiled = compile_def(def, Purpose::Preview)?;
    execute(conn, compiled).map_err(Error::from)
}

// ---------------------------------------------------------------------------
// Date parsing: strict `YYYY-MM-DD` → unix seconds (UTC midnight)
// ---------------------------------------------------------------------------

fn ascii_digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n: i64 = 0;
    for &d in bytes {
        n = n * 10 + i64::from(d - b'0');
    }
    Some(n)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 }; // Mar=0 … Feb=11
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Strict `YYYY-MM-DD` (UTC) → unix seconds; anything else is `None`.
fn parse_date(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let year = ascii_digits(&b[0..4])?;
    let month = ascii_digits(&b[5..7])?;
    let day = ascii_digits(&b[8..10])?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_utc_midnight() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("1969-12-31"), Some(-86_400));
        assert_eq!(parse_date("2020-01-01"), Some(1_577_836_800));
        assert_eq!(parse_date("2000-02-29"), Some(951_782_400)); // leap day
    }

    #[test]
    fn dates_reject_malformed_and_impossible_values() {
        for bad in [
            "",
            "2020-01-0",
            "2020-1-01",
            "202-01-01",
            "20200-01-01",
            "2020/01/01",
            "2020-01-01T00:00:00Z",
            "not-a-date",
            "2020-13-01",
            "2020-00-10",
            "2020-01-00",
            "2020-01-32",
            "2021-02-29", // not leap
            "9999999999-01-01",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?} should not parse");
        }
        // 2020 IS a leap year, for contrast with 2021 above.
        assert_eq!(parse_date("2020-02-29"), Some(1_582_934_400));
    }

    #[test]
    fn like_metacharacters_are_escaped() {
        assert_eq!(like_escape("plain"), "plain");
        assert_eq!(like_escape("a%b_c\\d"), "a\\%b\\_c\\\\d");
    }

    #[test]
    fn number_values_bind_as_integer_when_integral() {
        assert!(matches!(num_value(1993.0), Value::Integer(1993)));
        assert!(matches!(num_value(100.5), Value::Real(_)));
    }

    #[test]
    fn rule_values_coerce_leniently() {
        assert_eq!(RuleValue::Str(" 1993 ".into()).as_number(), Some(1993.0));
        assert_eq!(RuleValue::Str("nope".into()).as_number(), None);
        assert_eq!(RuleValue::Num(42.0).as_text(), "42");
        assert_eq!(RuleValue::Num(42.5).as_text(), "42.5");
    }
}
