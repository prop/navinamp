//! Offline prompt → Smart View rules (no AI provider configured).
//!
//! This is the no-network fallback for AI search: a small deterministic parser
//! that maps common library request phrasings ("jazz from the 70s", "highly
//! rated metal", "recently added ambient", "top 50 random") onto the same
//! [`SmartViewDef`] contract the cloud path produces. The result is validated
//! and evaluated by the normal Smart View engine, so the fallback can never
//! return rows the library engine would not.
//!
//! Deliberately conservative: if a phrase is not recognised and carries no
//! free text, the parser returns `None` and the caller keeps the "configure a
//! provider" error. Structured matches win over free-text matching; a flat
//! rule group cannot express `(year AND (title OR artist))`, so leftover words
//! are only used when no structured rule was found.

use std::collections::HashSet;

use navinamp_library::{
    Field, MatchMode, Op, RuleGroup, RuleRow, RuleValue, SmartViewDef, SortField,
};

/// Library vocabulary the parser can recognise (real genres/artists).
pub struct RulesContext<'a> {
    pub genres: &'a [String],
    pub artists: &'a [String],
}

/// Shared display name for AI/offline searches (first 60 chars, trimmed).
pub(crate) fn default_name(prompt: &str) -> String {
    prompt
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Words carrying no search intent once the structured phrases are extracted.
const STOP_WORDS: &[&str] = &[
    "a", "added", "after", "all", "an", "and", "any", "are", "artist", "artists", "be", "before",
    "best", "between", "by", "can", "do", "find", "for", "from", "give", "hear", "highly", "i",
    "in", "is", "it", "just", "like", "listen", "me", "more", "most", "music", "my", "never", "of",
    "on", "only", "or", "over", "played", "play", "please", "random", "rated", "recently", "show",
    "since", "some", "song", "songs", "sort", "star", "stars", "that", "the", "then", "to",
    "track", "tracks", "under", "until", "want", "with", "you",
];

/// A parsed prompt plus any trailing words the flat rule group had to drop.
pub struct ParsedPrompt {
    pub def: SmartViewDef,
    /// Present when a genre/artist/year/etc. matched but leftover free text
    /// could not be combined (a flat group has one `match` mode). Surfaced in
    /// the UI explanation instead of pretending the narrower query was exact.
    pub ignored: Option<String>,
}

#[derive(Default)]
struct Draft {
    rules: Vec<RuleRow>,
    sort: Option<SortField>,
    sort_desc: bool,
    limit: Option<f64>,
    /// Lowercased vocabulary values that produced rules (for the ignored-text
    /// note).
    matched: Vec<String>,
}

impl Draft {
    fn push(&mut self, field: Field, op: Op, value: RuleValue) {
        self.rules.push(RuleRow {
            field,
            op,
            value,
            value2: None,
        });
    }

    fn push_text(&mut self, field: Field, op: Op, value: &str) {
        self.push(field, op, RuleValue::Str(value.to_owned()));
    }

    fn push_num(&mut self, field: Field, op: Op, value: f64) {
        self.push(field, op, RuleValue::Num(value));
    }

    fn between(&mut self, field: Field, a: f64, b: f64) {
        self.rules.push(RuleRow {
            field,
            op: Op::Between,
            value: RuleValue::Num(a),
            value2: Some(RuleValue::Num(b)),
        });
    }

    /// First explicit ordering wins (recency phrases beat the title default).
    fn sort_by(&mut self, sort: SortField, desc: bool) {
        if self.sort.is_none() {
            self.sort = Some(sort);
            self.sort_desc = desc;
        }
    }

    fn finish(self, prompt: &str, mode: MatchMode, limit: Option<f64>) -> SmartViewDef {
        SmartViewDef {
            name: default_name(prompt),
            group: RuleGroup {
                mode,
                rules: self.rules,
            },
            limit: Some(limit.or(self.limit).unwrap_or(1000.0).clamp(1.0, 5000.0)),
            sort: self.sort.unwrap_or(SortField::Title),
            sort_desc: self.sort_desc,
        }
    }
}

/// Parse a free-form prompt into a Smart View definition, or `None` when
/// nothing recognisable is present.
pub fn parse_prompt(prompt: &str, ctx: &RulesContext<'_>) -> Option<ParsedPrompt> {
    let lower = prompt.to_lowercase();
    // Apostrophes are dropped so "70's" tokenises as "70s".
    let cleaned = lower.replace('\'', "");
    let tokens = tokenize(&cleaned);
    let mut draft = Draft::default();

    match_vocabulary(&mut draft, &lower, ctx.genres, Field::Genre, 2);
    match_vocabulary(&mut draft, &lower, ctx.artists, Field::Artist, 2);
    match_years(&mut draft, &lower, &tokens);
    match_quality(&mut draft, &lower, &tokens);
    match_recency(&mut draft, &lower);
    match_duration(&mut draft, &lower);
    match_order(&mut draft, &tokens);

    if draft.rules.is_empty() {
        // A limit/order-only prompt ("top 50", "shuffle") searches everything.
        let only_order = draft.limit.is_some() || draft.sort.is_some();
        if !only_order {
            let phrase = free_text(&tokens)?;
            let mut rules = Draft::default();
            rules.push_text(Field::Title, Op::Contains, &phrase);
            rules.push_text(Field::Artist, Op::Contains, &phrase);
            rules.push_text(Field::Album, Op::Contains, &phrase);
            return Some(ParsedPrompt {
                def: rules.finish(prompt, MatchMode::Any, Some(500.0)),
                ignored: None,
            });
        }
    }

    // Structured matches win; leftover words that cannot be combined with a
    // flat group are reported rather than silently dropped.
    let ignored = ignored_note(&lower, &draft.matched);
    Some(ParsedPrompt {
        def: draft.finish(prompt, MatchMode::All, None),
        ignored,
    })
}

// ---- recognisers -----------------------------------------------------------

/// Match real genre/artist names, longest first (so "hip hop" beats "hop").
fn match_vocabulary(draft: &mut Draft, lower: &str, values: &[String], field: Field, max: usize) {
    let mut hits: Vec<&String> = values
        .iter()
        .filter(|value| !value.trim().is_empty())
        // A needle longer than the prompt cannot match. The small margin keeps
        // Unicode lowercasing (which can grow) safe without allocating first.
        .filter(|value| value.len() <= lower.len() + 8)
        .filter(|value| contains_phrase(lower, &value.to_lowercase()))
        .collect();
    hits.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for value in hits.into_iter().take(max) {
        draft.matched.push(value.to_lowercase());
        draft.push_text(field, Op::Contains, value);
    }
}

fn match_years(draft: &mut Draft, lower: &str, tokens: &[String]) {
    // Decade words first ("the nineties").
    for (word, start) in [
        ("forties", 1940),
        ("fifties", 1950),
        ("sixties", 1960),
        ("seventies", 1970),
        ("eighties", 1980),
        ("nineties", 1990),
    ] {
        if lower.contains(word) {
            draft.between(Field::Year, start as f64, (start + 9) as f64);
            return;
        }
    }

    let years: Vec<(usize, (f64, f64))> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, token)| decade_range(token).map(|range| (i, range)))
        .collect();
    if let Some((_, (start, end))) = years.first() {
        draft.between(Field::Year, *start, *end);
        return;
    }

    let explicit: Vec<(usize, f64)> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, token)| explicit_year(token).map(|y| (i, y)))
        .collect();
    match explicit.as_slice() {
        // "between 1990 and 1999" / "from 1990 to 1999".
        [_, (_, b), ..] => {
            let a = explicit[0].1;
            draft.between(Field::Year, a.min(*b), a.max(*b));
        }
        [(i, year)] => {
            let before = i
                .checked_sub(1)
                .and_then(|j| tokens.get(j))
                .map(String::as_str);
            match before {
                Some("after" | "over") => draft.push_num(Field::Year, Op::Gt, *year),
                Some("since" | "from") => draft.push_num(Field::Year, Op::Gte, *year),
                Some("before" | "under") => draft.push_num(Field::Year, Op::Lt, *year),
                Some("until") => draft.push_num(Field::Year, Op::Lte, *year),
                _ => draft.push_num(Field::Year, Op::Eq, *year),
            }
        }
        _ => {}
    }
}

fn match_quality(draft: &mut Draft, lower: &str, tokens: &[String]) {
    let already = draft.rules.iter().any(|r| r.field == Field::Rating);
    if already {
        return;
    }
    if lower.contains("highly rated")
        || lower.contains("top rated")
        || lower.contains("best rated")
        || lower.contains("well rated")
    {
        draft.push_num(Field::Rating, Op::Gte, 4.0);
        return;
    }
    for window in tokens.windows(2) {
        let (word, unit) = (&window[0], &window[1]);
        if unit != "star" && unit != "stars" {
            continue;
        }
        let stars = match word.as_str() {
            "one" => Some(1.0),
            "two" => Some(2.0),
            "three" => Some(3.0),
            "four" => Some(4.0),
            "five" => Some(5.0),
            _ => word.parse::<f64>().ok().filter(|n| (1.0..=5.0).contains(n)),
        };
        if let Some(stars) = stars {
            draft.push_num(Field::Rating, Op::Gte, stars);
            return;
        }
    }
}

fn match_recency(draft: &mut Draft, lower: &str) {
    let never = lower.contains("never played")
        || lower.contains("unplayed")
        || lower.contains("not played yet");
    if never {
        draft.push_num(Field::PlayCount, Op::Eq, 0.0);
    } else if lower.contains("not played")
        && (lower.contains("recent")
            || lower.contains("while")
            || lower.contains("long time")
            || lower.contains("ages"))
    {
        draft.push_num(Field::LastPlayed, Op::NotInLastDays, 180.0);
    } else if lower.contains("recently played") || lower.contains("just played") {
        draft.push_num(Field::LastPlayed, Op::InLastDays, 30.0);
        draft.sort_by(SortField::RecentlyPlayed, true);
    }

    if lower.contains("recently added")
        || lower.contains("just added")
        || lower.contains("newly added")
        || lower.contains("new additions")
    {
        draft.push_num(Field::DateAdded, Op::InLastDays, 30.0);
        draft.sort_by(SortField::RecentlyAdded, true);
    } else if lower.contains("this year") {
        draft.push_num(Field::DateAdded, Op::InLastDays, 365.0);
    }

    if lower.contains("most played")
        || lower.contains("played a lot")
        || lower.contains("favorite")
        || lower.contains("favourite")
    {
        draft.sort_by(SortField::MostPlayed, true);
    }
}

fn match_duration(draft: &mut Draft, lower: &str) {
    if lower.contains("long track")
        || lower.contains("long song")
        || lower.contains("longest")
        || lower.contains("epic")
    {
        draft.push_num(Field::DurationSecs, Op::Gte, 420.0);
    } else if lower.contains("short track")
        || lower.contains("short song")
        || lower.contains("shortest")
        || lower.contains("quick")
    {
        draft.push_num(Field::DurationSecs, Op::Lte, 180.0);
    }
}

fn match_order(draft: &mut Draft, tokens: &[String]) {
    // "top 50", "first 100", "best 25".
    for window in tokens.windows(2) {
        if !matches!(window[0].as_str(), "top" | "first" | "best") {
            continue;
        }
        if let Some(n) = window[1]
            .parse::<f64>()
            .ok()
            .filter(|n| (1.0..=5000.0).contains(n))
        {
            draft.limit = Some(n);
        }
    }
    // "50 tracks", "20 songs".
    for window in tokens.windows(2) {
        if !matches!(window[1].as_str(), "track" | "tracks" | "song" | "songs") {
            continue;
        }
        if let Some(n) = window[0]
            .parse::<f64>()
            .ok()
            .filter(|n| (1.0..=5000.0).contains(n))
        {
            draft.limit = Some(n);
        }
    }

    if tokens.iter().any(|t| t == "random" || t == "shuffle") {
        draft.sort_by(SortField::Random, false);
    }
    if tokens.iter().any(|t| t == "alphabetical") {
        draft.sort_by(SortField::Title, false);
    }
    if tokens.windows(2).any(|w| w[0] == "by" && w[1] == "artist") {
        draft.sort_by(SortField::Artist, false);
    }
    if tokens.windows(2).any(|w| w[0] == "by" && w[1] == "album") {
        draft.sort_by(SortField::Album, false);
    }
}

// ---- text helpers ----------------------------------------------------------

fn tokenize(lower: &str) -> Vec<String> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whole-word/phrase containment (so genre "pop" does not match "popular").
/// `match_indices` yields char-boundary starts, which keeps the slicing safe
/// for non-ASCII library names.
fn contains_phrase(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    for (start, _) in haystack.match_indices(needle) {
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// `70s` → 1970–1979, `1980s` → 1980–1989 (two-digit decades ≥30 are 1900s).
fn decade_range(token: &str) -> Option<(f64, f64)> {
    let digits = token.strip_suffix('s')?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i32 = digits.parse().ok()?;
    match digits.len() {
        2 => {
            let start = if n >= 30 { 1900 + n } else { 2000 + n };
            Some((start as f64, (start + 9) as f64))
        }
        4 if n % 10 == 0 && (1900..=2090).contains(&n) => Some((n as f64, (n + 9) as f64)),
        _ => None,
    }
}

fn explicit_year(token: &str) -> Option<f64> {
    if token.len() != 4 {
        return None;
    }
    let year: i32 = token.parse().ok()?;
    (1900..=2100).contains(&year).then_some(year as f64)
}

/// Trim filler from both ends of a token run.
fn trim_edges<'a>(tokens: &'a [String], stop: &HashSet<&str>) -> &'a [String] {
    let mut start = 0;
    let mut end = tokens.len();
    while start < end && stop.contains(tokens[start].as_str()) {
        start += 1;
    }
    while end > start && stop.contains(tokens[end - 1].as_str()) {
        end -= 1;
    }
    &tokens[start..end]
}

/// Leftover meaningful words as one phrase. Only filler at the *edges* is
/// trimmed: interior articles are kept so "dark side of the moon" still
/// matches the real title.
fn free_text(tokens: &[String]) -> Option<String> {
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let words = trim_edges(tokens, &stop);
    if words.is_empty() || words.iter().all(|word| stop.contains(word.as_str())) {
        return None;
    }
    Some(words.join(" "))
}

/// Words the structured rules could not absorb (after removing matched
/// genres/artists and numeric/year tokens). Shown in the report explanation
/// so a narrowed query is never presented as an exact match.
fn ignored_note(lower: &str, matched: &[String]) -> Option<String> {
    let mut rest = lower.to_owned();
    for value in matched {
        rest = rest.replace(value, " ");
    }
    let tokens = tokenize(&rest.replace('\'', ""));
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let words: Vec<&str> = trim_edges(&tokens, &stop)
        .iter()
        .map(String::as_str)
        .filter(|word| !is_note_filler(word))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Words already consumed by year/limit/order rules, so they never appear as
/// "ignored text" in the note (they stay meaningful for free-text searches —
/// "top gun" must keep "top").
fn is_note_filler(word: &str) -> bool {
    if word.parse::<f64>().is_ok() {
        return true;
    }
    if word
        .strip_suffix('s')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }
    matches!(
        word,
        "top" | "first" | "best" | "random" | "shuffle" | "alphabetical"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(genres: &'a [String], artists: &'a [String]) -> RulesContext<'a> {
        RulesContext { genres, artists }
    }

    fn vocab() -> (Vec<String>, Vec<String>) {
        (
            vec![
                "Jazz".into(),
                "Metal".into(),
                "Ambient".into(),
                "Hip Hop".into(),
            ],
            vec!["Radiohead".into(), "Aphex Twin".into()],
        )
    }

    #[test]
    fn genre_and_decade_become_all_rules() {
        let (genres, artists) = vocab();
        let def = parse_prompt("jazz from the 70s", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert_eq!(def.group.mode, MatchMode::All);
        assert!(def.group.rules.iter().any(|r| r.field == Field::Genre
            && r.op == Op::Contains
            && r.value == RuleValue::Str("Jazz".into())));
        assert!(def.group.rules.iter().any(|r| r.field == Field::Year
            && r.op == Op::Between
            && r.value == RuleValue::Num(1970.0)
            && r.value2 == Some(RuleValue::Num(1979.0))));
    }

    #[test]
    fn quality_and_artist_phrases() {
        let (genres, artists) = vocab();
        let def = parse_prompt("highly rated radiohead", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(def.group.rules.iter().any(|r| r.field == Field::Rating
            && r.op == Op::Gte
            && r.value == RuleValue::Num(4.0)));
        assert!(
            def.group
                .rules
                .iter()
                .any(|r| r.field == Field::Artist && r.value == RuleValue::Str("Radiohead".into()))
        );
    }

    #[test]
    fn recency_sets_rule_and_ordering() {
        let (genres, artists) = vocab();
        let def = parse_prompt("recently added ambient", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(def.group.rules.iter().any(|r| r.field == Field::DateAdded
            && r.op == Op::InLastDays
            && r.value == RuleValue::Num(30.0)));
        assert_eq!(def.sort, SortField::RecentlyAdded);
        assert!(def.sort_desc);
    }

    #[test]
    fn never_played_and_most_played() {
        let (genres, artists) = vocab();
        let def = parse_prompt("most played jazz", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert_eq!(def.sort, SortField::MostPlayed);
        let def = parse_prompt("never played", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(def.group.rules.iter().any(|r| r.field == Field::PlayCount
            && r.op == Op::Eq
            && r.value == RuleValue::Num(0.0)));
    }

    #[test]
    fn limit_only_prompt_matches_everything() {
        let (genres, artists) = vocab();
        let def = parse_prompt("top 50", &ctx(&genres, &artists)).unwrap().def;
        assert!(def.group.rules.is_empty());
        assert_eq!(def.limit, Some(50.0));
        assert_eq!(def.group.mode, MatchMode::All);
    }

    #[test]
    fn free_text_falls_back_to_any_field_phrase() {
        let (genres, artists) = vocab();
        let def = parse_prompt("dark side of the moon", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert_eq!(def.group.mode, MatchMode::Any);
        assert_eq!(def.group.rules.len(), 3);
        assert!(
            def.group
                .rules
                .iter()
                .all(|r| r.value == RuleValue::Str("dark side of the moon".into()))
        );
        // Names keep their leading adjective ("new order").
        let def = parse_prompt("new order", &ctx(&[], &[])).unwrap().def;
        assert!(
            def.group
                .rules
                .iter()
                .any(|r| r.value == RuleValue::Str("new order".into()))
        );
    }

    #[test]
    fn stop_word_only_prompt_is_not_understood() {
        let (genres, artists) = vocab();
        assert!(parse_prompt("play some music please", &ctx(&genres, &artists)).is_none());
        assert!(parse_prompt("", &ctx(&genres, &artists)).is_none());
    }

    #[test]
    fn vocabulary_boundaries_hold() {
        let genres = vec!["Pop".to_string()];
        let artists: Vec<String> = vec![];
        // "pop" inside "popular" is not a genre match; the prompt still
        // becomes a free-text search for the word itself.
        let def = parse_prompt("popular songs", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(!def.group.rules.iter().any(|r| r.field == Field::Genre));
        let def = parse_prompt("pop songs", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(
            def.group
                .rules
                .iter()
                .any(|r| r.field == Field::Genre && r.value == RuleValue::Str("Pop".into()))
        );
    }

    #[test]
    fn structured_match_reports_ignored_free_text() {
        let (genres, artists) = vocab();

        let parsed = parse_prompt("metal dark side of the moon", &ctx(&genres, &artists)).unwrap();
        assert!(
            parsed
                .def
                .group
                .rules
                .iter()
                .any(|r| r.field == Field::Genre)
        );
        assert_eq!(parsed.ignored.as_deref(), Some("dark side of the moon"));

        let parsed =
            parse_prompt("highly rated ambient brian eno", &ctx(&genres, &artists)).unwrap();
        assert_eq!(parsed.ignored.as_deref(), Some("brian eno"));

        let parsed = parse_prompt("jazz songs by miles davis", &ctx(&genres, &artists)).unwrap();
        assert_eq!(parsed.ignored.as_deref(), Some("miles davis"));

        // Numbers/decades consumed by rules never show up as ignored text.
        let parsed = parse_prompt("top 50 metal", &ctx(&genres, &artists)).unwrap();
        assert_eq!(parsed.ignored, None);
        let parsed = parse_prompt("jazz from the 70s", &ctx(&genres, &artists)).unwrap();
        assert_eq!(parsed.ignored, None);
    }

    #[test]
    fn year_boundaries_are_exclusive() {
        let (genres, artists) = vocab();
        let def = parse_prompt("before 1990", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(def.group.rules.iter().any(|r| r.field == Field::Year
            && r.op == Op::Lt
            && r.value == RuleValue::Num(1990.0)));
        let def = parse_prompt("after 2000", &ctx(&genres, &artists))
            .unwrap()
            .def;
        assert!(def.group.rules.iter().any(|r| r.field == Field::Year
            && r.op == Op::Gt
            && r.value == RuleValue::Num(2000.0)));
    }
}
