//! Smart View integration tests: the serde contract, the rule compiler
//! (every field family × representative ops), sorts, limits, validation,
//! injection safety, and persistence round-trips.
//!
//! Fixtures are direct `tracks` inserts through a second WAL connection:
//! smart views query `tracks` directly (no FTS involved) and
//! rating/playCount/lastPlayed aren't scanner-settable, so going around
//! the scanner is both faster and the only way to hit those columns.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use navinamp_library::{
    Field, Library, MatchMode, Op, RuleGroup, RuleRow, RuleValue, SavedSmartView, SmartViewDef,
    SortField, Track,
};
use rusqlite::Connection;

use common::Scratch;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// One test's world: scratch dir, the `Library` under test, a raw
/// connection for direct inserts, and the fixture's "now".
struct Ctx {
    scratch: Scratch,
    library: Library,
    raw: Connection,
    now: i64,
}

impl Ctx {
    fn ago(&self, days: i64) -> i64 {
        self.now - days * 86_400
    }
}

fn ctx(label: &str) -> Ctx {
    let scratch = Scratch::new(label);
    let library = Library::open(scratch.db()).unwrap();
    let raw = Connection::open(scratch.db()).unwrap();
    Ctx {
        scratch,
        library,
        raw,
        now: now_secs(),
    }
}

/// A `tracks` row under construction. `path` auto-uniques via a counter.
struct Row {
    title: String,
    artist: String,
    album: String,
    genre: String,
    path: String,
    year: Option<i64>,
    rating: i64,
    play_count: i64,
    duration_secs: f64,
    bitrate_kbps: Option<i64>,
    last_played_at: Option<i64>,
    date_added: i64,
    source: String,
}

impl Row {
    fn new(title: &str) -> Row {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Row {
            title: title.into(),
            artist: "Unknown".into(),
            album: "Unknown".into(),
            genre: "Unknown".into(),
            path: format!("/m/track-{n}.mp3"),
            year: None,
            rating: 0,
            play_count: 0,
            duration_secs: 0.0,
            bitrate_kbps: None,
            last_played_at: None,
            date_added: 1_700_000_000,
            source: "local".into(),
        }
    }

    fn artist(mut self, v: &str) -> Row {
        self.artist = v.into();
        self
    }
    fn album(mut self, v: &str) -> Row {
        self.album = v.into();
        self
    }
    fn genre(mut self, v: &str) -> Row {
        self.genre = v.into();
        self
    }
    fn path(mut self, v: &str) -> Row {
        self.path = v.into();
        self
    }
    fn year(mut self, v: Option<i64>) -> Row {
        self.year = v;
        self
    }
    fn rating(mut self, v: i64) -> Row {
        self.rating = v;
        self
    }
    fn plays(mut self, v: i64) -> Row {
        self.play_count = v;
        self
    }
    fn duration(mut self, v: f64) -> Row {
        self.duration_secs = v;
        self
    }
    fn bitrate(mut self, v: Option<i64>) -> Row {
        self.bitrate_kbps = v;
        self
    }
    fn played(mut self, v: Option<i64>) -> Row {
        self.last_played_at = v;
        self
    }
    fn added(mut self, v: i64) -> Row {
        self.date_added = v;
        self
    }
    fn source(mut self, v: &str) -> Row {
        self.source = v.into();
        self
    }
}

fn insert(conn: &Connection, row: Row) {
    conn.execute(
        "INSERT INTO tracks (
             path, mtime, size, title, artist, album_artist, album, album_key,
             genre, year, duration_secs, bitrate_kbps, codec, rating,
             play_count, last_played_at, date_added, source
         ) VALUES (?1, 0, 0, ?2, ?3, '', ?4, ?5, ?6, ?7, ?8, ?9, 'mp3', ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            row.path,
            row.title,
            row.artist,
            row.album,
            format!(
                "{}\0{}",
                row.artist.to_lowercase(),
                row.album.to_lowercase()
            ),
            row.genre,
            row.year,
            row.duration_secs,
            row.bitrate_kbps,
            row.rating,
            row.play_count,
            row.last_played_at,
            row.date_added,
            row.source,
        ],
    )
    .unwrap();
}

/// The shared seven-row fixture. Times are relative to `ctx.now` except
/// the two rows with absolute 2000-02-29 / 2020-01-01 stamps (951_782_400 /
/// 1_577_836_800) for `before`/`after` assertions.
///
/// | row | title | artist | album | genre | year | rating | plays | dur | br | added | played | source |
/// |-----|-------|--------|-------|-------|------|--------|-------|-----|----|-------|--------|--------|
/// | r1 | Alpha | ANN | Zoo | Rock | 1990 | 1 | 0 | 100.5 | 128 | now-40d | never | local |
/// | r2 | beta | Bob | Zoo | rock | 2000 | 3 | 2 | 200 | 256 | now-10d | now-5d | local |
/// | r3 | Gamma | Cara | Mid | Jazz | NULL | 5 | 10 | 300.25 | NULL | now-2d | now-1d | local |
/// | r4 | 100% Blues | ANN | Mid | jazz | 2010 | 0 | 0 | 50 | 320 | now-100d | now-60d | local |
/// | r5 | Remote One | Zed | Net | Pop | 2020 | 4 | 7 | 150 | NULL | now-1d | never | navidrome |
/// | r6 | Old Days | Pat | Past | Blues | 1970 | 2 | 1 | 250 | 96 | 2000-02-29 | 2000-02-29 | local |
/// | r7 | Twentieth | Quinn | Past | Pop | 2005 | 2 | 3 | 175 | 191 | 2020-01-01 | 2020-01-01 | local |
fn rule_fixture(label: &str) -> Ctx {
    let c = ctx(label);
    insert(
        &c.raw,
        Row::new("Alpha")
            .artist("ANN")
            .album("Zoo")
            .genre("Rock")
            .year(Some(1990))
            .rating(1)
            .duration(100.5)
            .bitrate(Some(128))
            .added(c.ago(40))
            .path("/m/alpha.mp3"),
    );
    insert(
        &c.raw,
        Row::new("beta")
            .artist("Bob")
            .album("Zoo")
            .genre("rock")
            .year(Some(2000))
            .rating(3)
            .plays(2)
            .duration(200.0)
            .bitrate(Some(256))
            .added(c.ago(10))
            .played(Some(c.ago(5)))
            .path("/sub/beta.mp3"),
    );
    insert(
        &c.raw,
        Row::new("Gamma")
            .artist("Cara")
            .album("Mid")
            .genre("Jazz")
            .year(None)
            .rating(5)
            .plays(10)
            .duration(300.25)
            .bitrate(None)
            .added(c.ago(2))
            .played(Some(c.ago(1)))
            .path("/m/gamma.mp3"),
    );
    insert(
        &c.raw,
        Row::new("100% Blues")
            .artist("ANN")
            .album("Mid")
            .genre("jazz")
            .year(Some(2010))
            .rating(0)
            .duration(50.0)
            .bitrate(Some(320))
            .added(c.ago(100))
            .played(Some(c.ago(60)))
            .path("/m/blues.mp3"),
    );
    insert(
        &c.raw,
        Row::new("Remote One")
            .artist("Zed")
            .album("Net")
            .genre("Pop")
            .year(Some(2020))
            .rating(4)
            .plays(7)
            .duration(150.0)
            .bitrate(None)
            .added(c.ago(1))
            .played(None)
            .source("navidrome")
            .path("/net/remote.mp3"),
    );
    insert(
        &c.raw,
        Row::new("Old Days")
            .artist("Pat")
            .album("Past")
            .genre("Blues")
            .year(Some(1970))
            .rating(2)
            .plays(1)
            .duration(250.0)
            .bitrate(Some(96))
            .added(951_782_400)
            .played(Some(951_782_400))
            .path("/m/old.mp3"),
    );
    insert(
        &c.raw,
        Row::new("Twentieth")
            .artist("Quinn")
            .album("Past")
            .genre("Pop")
            .year(Some(2005))
            .rating(2)
            .plays(3)
            .duration(175.0)
            .bitrate(Some(191))
            .added(1_577_836_800)
            .played(Some(1_577_836_800))
            .path("/m/twentieth.mp3"),
    );
    c
}

const ALL_TITLES_ASC: &[&str] = &[
    "100% Blues",
    "Alpha",
    "beta",
    "Gamma",
    "Old Days",
    "Remote One",
    "Twentieth",
];

fn s(v: &str) -> RuleValue {
    RuleValue::Str(v.into())
}

fn n(v: f64) -> RuleValue {
    RuleValue::Num(v)
}

fn rule(field: Field, op: Op, value: RuleValue) -> RuleRow {
    RuleRow {
        field,
        op,
        value,
        value2: None,
    }
}

fn group(mode: MatchMode, rules: Vec<RuleRow>) -> RuleGroup {
    RuleGroup { mode, rules }
}

fn view(name: &str, group: RuleGroup) -> SmartViewDef {
    SmartViewDef {
        name: name.into(),
        group,
        limit: None,
        sort: SortField::Title,
        sort_desc: false,
    }
}

fn sorted_titles(tracks: &[Track]) -> Vec<String> {
    // lowercase key, mirroring the SQL side's NOCASE ordering for set checks
    let mut titles: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
    titles.sort_by_key(|t| t.to_lowercase());
    titles
}

fn preview_titles(lib: &Library, def: &SmartViewDef) -> Vec<String> {
    lib.smart_view_preview(def)
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect()
}

fn run_titles(lib: &Library, id: i64) -> Vec<String> {
    lib.smart_view_run(id)
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect()
}

/// The def must be rejected, with a message mentioning every needle.
fn assert_rejected(lib: &Library, def: &SmartViewDef, needles: &[&str]) {
    match lib.smart_view_preview(def) {
        Ok(tracks) => panic!("expected rejection, got {} tracks", tracks.len()),
        Err(e) => {
            let msg = e.to_string();
            for needle in needles {
                assert!(
                    msg.contains(needle),
                    "message {msg:?} must mention {needle:?}"
                );
            }
        }
    }
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_owned()).collect()
}

// ---------------------------------------------------------------------------
// Serde contract
// ---------------------------------------------------------------------------

#[test]
fn contract_def_json_round_trips_exactly() {
    let json = r#"{
        "name": "90s electronic",
        "group": {
            "match": "any",
            "rules": [
                { "field": "genre", "op": "is", "value": "Electronic" },
                { "field": "year", "op": "between", "value": 1990, "value2": 1999 },
                { "field": "lastPlayed", "op": "inLastDays", "value": 7 },
                { "field": "playCount", "op": "gt", "value": 5 }
            ]
        },
        "limit": 100,
        "sort": "recentlyAdded",
        "sortDesc": true
    }"#;
    let def: SmartViewDef = serde_json::from_str(json).unwrap();
    assert_eq!(def.name, "90s electronic");
    assert_eq!(def.group.mode, MatchMode::Any);
    assert_eq!(def.group.rules.len(), 4);
    assert_eq!(def.group.rules[1].value2, Some(n(1999.0)));
    assert_eq!(def.limit, Some(100.0));
    assert_eq!(def.sort, SortField::RecentlyAdded);
    assert!(def.sort_desc);

    // exact round-trip (limit, value2, mode, direction all survive)
    let wire = serde_json::to_string(&def).unwrap();
    assert_eq!(serde_json::from_str::<SmartViewDef>(&wire).unwrap(), def);

    // and every key on the wire is camelCase
    assert_camel_case(&serde_json::to_value(&def).unwrap());
    let saved = SavedSmartView {
        id: 7,
        def: def.clone(),
    };
    assert_camel_case(&serde_json::to_value(&saved).unwrap());
    let saved_rt =
        serde_json::from_str::<SavedSmartView>(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert_eq!(saved_rt, saved);
}

fn assert_camel_case(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                assert!(!key.contains('_'), "key {key:?} is not camelCase");
                assert_camel_case(value);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(assert_camel_case),
        _ => {}
    }
}

#[test]
fn contract_enum_spellings_are_exact_and_unknown_rejected() {
    for spelling in [
        "title",
        "artist",
        "album",
        "genre",
        "path",
        "source",
        "year",
        "rating",
        "playCount",
        "durationSecs",
        "bitrateKbps",
        "lastPlayed",
        "dateAdded",
    ] {
        let field: Field = serde_json::from_str(&format!("\"{spelling}\"")).unwrap();
        assert_eq!(
            serde_json::to_string(&field).unwrap(),
            format!("\"{spelling}\"")
        );
    }
    for spelling in [
        "is",
        "isNot",
        "contains",
        "notContains",
        "startsWith",
        "endsWith",
        "eq",
        "ne",
        "gt",
        "gte",
        "lt",
        "lte",
        "between",
        "inLastDays",
        "notInLastDays",
        "before",
        "after",
    ] {
        let op: Op = serde_json::from_str(&format!("\"{spelling}\"")).unwrap();
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            format!("\"{spelling}\"")
        );
    }
    for spelling in [
        "title",
        "artist",
        "album",
        "random",
        "mostPlayed",
        "recentlyAdded",
        "recentlyPlayed",
    ] {
        let sort: SortField = serde_json::from_str(&format!("\"{spelling}\"")).unwrap();
        assert_eq!(
            serde_json::to_string(&sort).unwrap(),
            format!("\"{spelling}\"")
        );
    }
    for spelling in ["all", "any"] {
        let mode: MatchMode = serde_json::from_str(&format!("\"{spelling}\"")).unwrap();
        assert_eq!(
            serde_json::to_string(&mode).unwrap(),
            format!("\"{spelling}\"")
        );
    }

    // unknown enum values are boundary errors, never silent wildcards
    assert!(serde_json::from_str::<Field>("\"bogus\"").is_err());
    assert!(serde_json::from_str::<Field>("\"play_count\"").is_err()); // snake_case is wrong
    assert!(serde_json::from_str::<Op>("\"near\"").is_err());
    assert!(serde_json::from_str::<SortField>("\"byYear\"").is_err());
    assert!(serde_json::from_str::<MatchMode>("\"all-of\"").is_err());
}

#[test]
fn contract_rejects_unknown_fields_and_non_scalar_values() {
    let base = r#"{
        "name": "x",
        "group": { "match": "all", "rules": [
            { "field": "title", "op": "contains", "value": "y" }
        ] },
        "sort": "title",
        "sortDesc": false
    }"#;
    assert!(serde_json::from_str::<SmartViewDef>(base).is_ok());

    // deny_unknown_fields: typos fail at the boundary
    for mutated in [
        base.replace("\"sort\"", "\"sorting\""),
        base.replace("\"sortDesc\"", "\"sortdesc\""),
        base.replace("\"group\"", "\"groups\""),
        base.replace("\"match\"", "\"mode\""),
        base.replace("\"value\"", "\"val\""),
        base.replace("\"field\"", "\"feild\""),
        base.replace("\"rules\"", "\"rule\""),
    ] {
        assert!(
            serde_json::from_str::<SmartViewDef>(&mutated).is_err(),
            "should reject: {mutated}"
        );
    }

    // rule values must be string or number
    for bad in ["true", "null", "[\"x\"]", "{\"a\":1}"] {
        let json = format!(r#"{{ "field": "title", "op": "is", "value": {bad} }}"#);
        assert!(
            serde_json::from_str::<RuleRow>(&json).is_err(),
            "value {bad} should not deserialize"
        );
    }

    // limit is number | null
    let bad_limit = base.replace(
        "\"sort\": \"title\"",
        "\"limit\": \"100\", \"sort\": \"title\"",
    );
    assert!(serde_json::from_str::<SmartViewDef>(&bad_limit).is_err());
    let null_limit = base.replace(
        "\"sort\": \"title\"",
        "\"limit\": null, \"sort\": \"title\"",
    );
    let def: SmartViewDef = serde_json::from_str(&null_limit).unwrap();
    assert_eq!(def.limit, None);
}

// ---------------------------------------------------------------------------
// Text rules
// ---------------------------------------------------------------------------

#[test]
fn text_rules_every_title_op_is_case_insensitive() {
    let c = rule_fixture("text-title");
    let preview = |rules| preview_titles(&c.library, &view("t", group(MatchMode::All, rules)));

    assert_eq!(
        preview(vec![rule(Field::Title, Op::Is, s("alpha"))]),
        strs(&["Alpha"])
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::Is, s("ALPHA"))]),
        strs(&["Alpha"])
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::IsNot, s("alpha"))]).len(),
        6
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::Contains, s("BL"))]),
        strs(&["100% Blues"])
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::NotContains, s("a"))]),
        strs(&["100% Blues", "Remote One", "Twentieth"])
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::StartsWith, s("old"))]),
        strs(&["Old Days"])
    );
    assert_eq!(
        preview(vec![rule(Field::Title, Op::EndsWith, s("ONE"))]),
        strs(&["Remote One"])
    );
    // empty string: contains "" matches everything, startsWith "" too
    assert_eq!(
        preview(vec![rule(Field::Title, Op::Contains, s(""))]).len(),
        7
    );
}

#[test]
fn text_rules_cover_artist_album_genre_path_source() {
    let c = rule_fixture("text-fields");
    let one = |field, op, value: &str| -> Vec<String> {
        preview_titles(
            &c.library,
            &view("t", group(MatchMode::All, vec![rule(field, op, s(value))])),
        )
    };

    assert_eq!(
        one(Field::Artist, Op::Is, "ann"),
        strs(&["100% Blues", "Alpha"])
    );
    assert_eq!(one(Field::Artist, Op::Contains, "ara"), strs(&["Gamma"]));
    assert_eq!(one(Field::Album, Op::Is, "zoo"), strs(&["Alpha", "beta"]));
    assert_eq!(one(Field::Album, Op::IsNot, "past").len(), 5);
    assert_eq!(
        one(Field::Genre, Op::Is, "jazz"),
        strs(&["100% Blues", "Gamma"])
    );
    assert_eq!(one(Field::Genre, Op::Is, "ROCK"), strs(&["Alpha", "beta"]));
    assert_eq!(one(Field::Path, Op::Contains, "sub/"), strs(&["beta"]));
    assert_eq!(
        one(Field::Path, Op::StartsWith, "/m/"),
        strs(&["100% Blues", "Alpha", "Gamma", "Old Days", "Twentieth"])
    );
    // `source` accepts the general string ops; is/isNot are the sane ones
    assert_eq!(one(Field::Source, Op::Is, "local").len(), 6);
    assert_eq!(
        one(Field::Source, Op::IsNot, "local"),
        strs(&["Remote One"])
    );
    assert_eq!(
        one(Field::Source, Op::Is, "navidrome"),
        strs(&["Remote One"])
    );
    assert_eq!(
        one(Field::Source, Op::Contains, "navi"),
        strs(&["Remote One"])
    );
}

#[test]
fn text_rules_stringify_number_values() {
    let c = ctx("text-num-coerce");
    insert(&c.raw, Row::new("1999").year(Some(1999)));
    insert(&c.raw, Row::new("Party Like 1999"));

    let hits = preview_titles(
        &c.library,
        &view(
            "t",
            group(
                MatchMode::All,
                vec![rule(Field::Title, Op::Contains, n(1999.0))],
            ),
        ),
    );
    assert_eq!(hits.len(), 2); // number value matched as the text "1999"
}

// ---------------------------------------------------------------------------
// Number rules
// ---------------------------------------------------------------------------

#[test]
fn number_rules_cover_every_field_and_op() {
    let c = rule_fixture("numbers");
    let one = |field, op, value: RuleValue| -> Vec<String> {
        let def = view("t", group(MatchMode::All, vec![rule(field, op, value)]));
        let mut titles = preview_titles(&c.library, &def);
        titles.sort();
        titles
    };

    // year: eq/ne/gt/gte/lt/lte + string-typed numbers accepted
    assert_eq!(one(Field::Year, Op::Eq, n(1990.0)), strs(&["Alpha"]));
    assert_eq!(one(Field::Year, Op::Eq, s("1990")), strs(&["Alpha"]));
    assert_eq!(
        one(Field::Year, Op::Ne, n(1990.0)),
        strs(&["100% Blues", "Old Days", "Remote One", "Twentieth", "beta"])
    ); // Gamma's NULL year is excluded, not "not 1990"
    assert_eq!(
        one(Field::Year, Op::Gt, n(2000.0)),
        strs(&["100% Blues", "Remote One", "Twentieth"])
    );
    assert_eq!(one(Field::Year, Op::Gte, n(2000.0)).len(), 4);
    assert_eq!(one(Field::Year, Op::Lt, n(1971.0)), strs(&["Old Days"]));
    assert_eq!(one(Field::Year, Op::Lte, n(1970.0)), strs(&["Old Days"]));

    // rating / playCount / durationSecs (fractional) / bitrateKbps
    assert_eq!(
        one(Field::Rating, Op::Gte, n(3.0)),
        strs(&["Gamma", "Remote One", "beta"])
    );
    assert_eq!(
        one(Field::PlayCount, Op::Gt, n(5.0)),
        strs(&["Gamma", "Remote One"])
    );
    assert_eq!(
        one(Field::BitrateKbps, Op::Lte, n(128.0)),
        strs(&["Alpha", "Old Days"])
    );
    assert_eq!(one(Field::DurationSecs, Op::Eq, n(100.5)), strs(&["Alpha"]));
}

#[test]
fn number_between_needs_both_bounds_and_hits_nulls_never() {
    let c = rule_fixture("between");
    let between = |field, a: RuleValue, b: Option<RuleValue>| {
        let mut titles = preview_titles(
            &c.library,
            &view(
                "t",
                group(
                    MatchMode::All,
                    vec![RuleRow {
                        field,
                        op: Op::Between,
                        value: a,
                        value2: b,
                    }],
                ),
            ),
        );
        titles.sort();
        titles
    };

    // 150–260 s: beta 200, Remote One 150, Old Days 250, Twentieth 175
    assert_eq!(
        between(Field::DurationSecs, n(150.0), Some(n(260.0))),
        strs(&["Old Days", "Remote One", "Twentieth", "beta"])
    );
    // string bounds parse as numbers
    assert_eq!(
        between(Field::Year, s("1990"), Some(s("2010"))),
        strs(&["100% Blues", "Alpha", "Twentieth", "beta"])
    );
    // NULL year and NULL bitrate never match a between
    assert_eq!(
        between(Field::BitrateKbps, n(0.0), Some(n(5000.0))).len(),
        5
    );
}

// ---------------------------------------------------------------------------
// Date rules
// ---------------------------------------------------------------------------

#[test]
fn date_rules_in_last_days_and_absolute_dates() {
    let c = rule_fixture("dates");
    let one = |field, op, value: RuleValue| {
        let def = view("t", group(MatchMode::All, vec![rule(field, op, value)]));
        sorted_titles(&c.library.smart_view_preview(&def).unwrap())
    };

    // lastPlayed inLast 7: beta (5d) + Gamma (1d)
    assert_eq!(
        one(Field::LastPlayed, Op::InLastDays, n(7.0)),
        strs(&["beta", "Gamma"])
    );
    // notInLast 7 includes the never-played rows (Alpha, Remote One)
    assert_eq!(
        one(Field::LastPlayed, Op::NotInLastDays, n(7.0)),
        strs(&["100% Blues", "Alpha", "Old Days", "Remote One", "Twentieth"])
    );
    // string day counts parse
    assert_eq!(
        one(Field::LastPlayed, Op::InLastDays, s("7")),
        strs(&["beta", "Gamma"])
    );
    // dateAdded inLast 30: beta 10d, Gamma 2d, Remote One 1d
    assert_eq!(
        one(Field::DateAdded, Op::InLastDays, n(30.0)),
        strs(&["beta", "Gamma", "Remote One"])
    );
    // dateAdded notInLast 30 — date_added is NOT NULL, same set logic
    assert_eq!(one(Field::DateAdded, Op::NotInLastDays, n(30.0)).len(), 4);

    // before/after with absolute YYYY-MM-DD (UTC)
    assert_eq!(
        one(Field::DateAdded, Op::Before, s("2010-01-01")),
        strs(&["Old Days"])
    );
    assert_eq!(one(Field::DateAdded, Op::After, s("2010-01-01")).len(), 6);
    assert_eq!(
        one(Field::LastPlayed, Op::Before, s("2015-01-01")),
        strs(&["Old Days"])
    );
    // after 2015: Twentieth + the 2026-relative plays; never-played excluded
    assert_eq!(
        one(Field::LastPlayed, Op::After, s("2015-01-01")),
        strs(&["100% Blues", "beta", "Gamma", "Twentieth"])
    );
}

// ---------------------------------------------------------------------------
// Group semantics
// ---------------------------------------------------------------------------

#[test]
fn match_all_is_conjunction_match_any_is_disjunction() {
    let c = rule_fixture("all-any");
    let rules = vec![
        rule(Field::Genre, Op::Is, s("rock")), // Alpha, beta
        rule(Field::Year, Op::Gte, n(2000.0)), // beta, 100% Blues, Twentieth, Remote One
    ];
    let all = view("all", group(MatchMode::All, rules.clone()));
    assert_eq!(preview_titles(&c.library, &all), strs(&["beta"]));
    let any = view("any", group(MatchMode::Any, rules));
    assert_eq!(
        preview_titles(&c.library, &any),
        strs(&["100% Blues", "Alpha", "beta", "Remote One", "Twentieth"])
    );
}

#[test]
fn empty_group_matches_everything() {
    let c = rule_fixture("empty-group");
    for mode in [MatchMode::All, MatchMode::Any] {
        let def = view("everything", group(mode, vec![]));
        let hits = c.library.smart_view_preview(&def).unwrap();
        assert_eq!(sorted_titles(&hits), strs(ALL_TITLES_ASC));
    }
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[test]
fn limit_is_applied_and_validated() {
    let c = rule_fixture("limits");

    let mut def = view("two", group(MatchMode::All, vec![]));
    def.limit = Some(2.0);
    assert_eq!(
        preview_titles(&c.library, &def),
        strs(&["100% Blues", "Alpha"])
    );

    def.limit = None;
    assert_eq!(preview_titles(&c.library, &def).len(), 7);

    // 0, negative, fractional and >5000 limits are rejected everywhere —
    // including save, so nothing invalid is ever persisted.
    for bad in [0.0, -5.0, 2.5, 5001.0] {
        let mut def = view("bad", group(MatchMode::All, vec![]));
        def.limit = Some(bad);
        assert_rejected(&c.library, &def, &["limit"]);
        assert!(c.library.smart_view_save(None, &def).is_err());
    }
    assert_eq!(c.library.smart_views_list().unwrap().len(), 0); // nothing leaked in

    // boundaries: 1 and 5000 are fine
    for good in [1.0, 5000.0] {
        let mut def = view("good", group(MatchMode::All, vec![]));
        def.limit = Some(good);
        assert!(c.library.smart_view_preview(&def).is_ok());
    }
}

#[test]
fn preview_caps_at_500_run_honors_the_saved_def() {
    let c = ctx("cap-500");
    for i in 0..501 {
        insert(&c.raw, Row::new(&format!("T{:04}", i)));
    }
    let unlimited = view("all", group(MatchMode::All, vec![]));

    // null limit → 500; explicit smaller limit wins; limit > cap still 500
    assert_eq!(c.library.smart_view_preview(&unlimited).unwrap().len(), 500);
    let mut small = unlimited.clone();
    small.limit = Some(300.0);
    assert_eq!(c.library.smart_view_preview(&small).unwrap().len(), 300);
    let mut big = unlimited.clone();
    big.limit = Some(600.0);
    let hits = c.library.smart_view_preview(&big).unwrap();
    assert_eq!(hits.len(), 500);
    assert_eq!(hits.last().unwrap().title, "T0499"); // title sort, first 500

    // run() is uncapped by the preview limit and honors the def's own
    let id = c.library.smart_view_save(None, &unlimited).unwrap();
    assert_eq!(run_titles(&c.library, id).len(), 501);
    let id = c.library.smart_view_save(None, &big).unwrap();
    assert_eq!(run_titles(&c.library, id).len(), 501);
    let mut three = unlimited.clone();
    three.limit = Some(3.0);
    let id = c.library.smart_view_save(None, &three).unwrap();
    assert_eq!(
        run_titles(&c.library, id),
        strs(&["T0000", "T0001", "T0002"])
    );
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

#[test]
fn sorts_cover_every_field_in_both_directions() {
    let c = rule_fixture("sorts");
    let ordered = |sort: SortField, desc: bool| {
        let def = SmartViewDef {
            name: "s".into(),
            group: group(MatchMode::All, vec![]),
            limit: None,
            sort,
            sort_desc: desc,
        };
        preview_titles(&c.library, &def)
    };

    // title: NOCASE asc, and sortDesc reverses the primary key only
    assert_eq!(ordered(SortField::Title, false), strs(ALL_TITLES_ASC));
    assert_eq!(
        ordered(SortField::Title, true),
        strs(&[
            "Twentieth",
            "Remote One",
            "Old Days",
            "Gamma",
            "beta",
            "Alpha",
            "100% Blues"
        ])
    );

    // artist: ANN ties broken by album (Mid before Zoo)
    assert_eq!(
        ordered(SortField::Artist, false),
        strs(&[
            "100% Blues",
            "Alpha",
            "beta",
            "Gamma",
            "Old Days",
            "Twentieth",
            "Remote One"
        ])
    );
    assert_eq!(
        ordered(SortField::Artist, true),
        strs(&[
            "Remote One",
            "Twentieth",
            "Old Days",
            "Gamma",
            "beta",
            "100% Blues",
            "Alpha"
        ])
    );

    // album: Zoo ties broken by path (alpha before beta)
    assert_eq!(
        ordered(SortField::Album, false),
        strs(&[
            "100% Blues",
            "Gamma",
            "Remote One",
            "Old Days",
            "Twentieth",
            "Alpha",
            "beta"
        ])
    );

    // mostPlayed desc: 10,7,3,2,1,0,0 (the 0s tie on artist/album)
    assert_eq!(
        ordered(SortField::MostPlayed, true),
        strs(&[
            "Gamma",
            "Remote One",
            "Twentieth",
            "beta",
            "Old Days",
            "100% Blues",
            "Alpha"
        ])
    );
    assert_eq!(
        ordered(SortField::MostPlayed, false),
        strs(&[
            "100% Blues",
            "Alpha",
            "Old Days",
            "beta",
            "Twentieth",
            "Remote One",
            "Gamma"
        ])
    );

    // recentlyAdded desc: now-1d, now-2d, now-10d, now-40d, now-100d, 2020, 2000
    assert_eq!(
        ordered(SortField::RecentlyAdded, true),
        strs(&[
            "Remote One",
            "Gamma",
            "beta",
            "Alpha",
            "100% Blues",
            "Twentieth",
            "Old Days"
        ])
    );
    assert_eq!(
        ordered(SortField::RecentlyAdded, false),
        strs(&[
            "Old Days",
            "Twentieth",
            "100% Blues",
            "Alpha",
            "beta",
            "Gamma",
            "Remote One"
        ])
    );

    // recentlyPlayed: NULLs (never played) sort FIRST ascending and LAST
    // descending (SQLite: NULL < every value); ties within the NULLs on artist
    assert_eq!(
        ordered(SortField::RecentlyPlayed, false),
        strs(&[
            "Alpha",
            "Remote One",
            "Old Days",
            "Twentieth",
            "100% Blues",
            "beta",
            "Gamma"
        ])
    );
    assert_eq!(
        ordered(SortField::RecentlyPlayed, true),
        strs(&[
            "Gamma",
            "beta",
            "100% Blues",
            "Twentieth",
            "Old Days",
            "Alpha",
            "Remote One"
        ])
    );
}

#[test]
fn random_sort_returns_exactly_the_matching_set() {
    let c = rule_fixture("random");
    let mut def = view("shuffle", group(MatchMode::All, vec![]));
    def.sort = SortField::Random;
    for _ in 0..3 {
        let hits = c.library.smart_view_preview(&def).unwrap();
        assert_eq!(sorted_titles(&hits), strs(ALL_TITLES_ASC));
    }
    // direction is accepted (and ignored) on random
    def.sort_desc = true;
    let hits = c.library.smart_view_preview(&def).unwrap();
    assert_eq!(hits.len(), 7);
}

#[test]
fn sort_tiebreakers_make_pages_deterministic() {
    let c = ctx("tiebreak");
    for i in 0..12 {
        // same artist/album/track-less: only path differentiates
        insert(
            &c.raw,
            Row::new(&format!("Same {i}")).artist("Same").album("Same"),
        );
    }
    let mut def = view("same", group(MatchMode::All, vec![]));
    def.sort = SortField::Title;
    let first = preview_titles(&c.library, &def);
    let second = preview_titles(&c.library, &def);
    assert_eq!(first, second);
    assert_eq!(first.len(), 12);
}

// ---------------------------------------------------------------------------
// Validation errors
// ---------------------------------------------------------------------------

#[test]
fn validation_names_the_field_op_and_rule_index() {
    let c = rule_fixture("validation");
    let at = |rules: Vec<RuleRow>| view("v", group(MatchMode::All, rules));

    // wrong-kind ops
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::Title, Op::Eq, s("x"))]),
        &["rules[0]", "title", "eq"],
    );
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::Year, Op::Contains, s("19"))]),
        &["rules[0]", "year", "contains"],
    );
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::DateAdded, Op::Is, s("2020-01-01"))]),
        &["rules[0]", "dateAdded", "is"],
    );

    // the index is the *offending* rule's, not the first
    assert_rejected(
        &c.library,
        &at(vec![
            rule(Field::Genre, Op::Is, s("Rock")),
            rule(Field::Artist, Op::Is, s("ANN")),
            rule(Field::PlayCount, Op::StartsWith, n(1.0)),
        ]),
        &["rules[2]", "playCount", "startsWith"],
    );

    // wrong-type values
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::Year, Op::Eq, s("nineteen-ninety"))]),
        &["rules[0]", "year", "eq", "number"],
    );
    assert_rejected(
        &c.library,
        &at(vec![RuleRow {
            field: Field::Year,
            op: Op::Between,
            value: n(1990.0),
            value2: Some(s("not a year")),
        }]),
        &["rules[0]", "year", "between", "number"],
    );

    // between without value2
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::Year, Op::Between, n(1990.0))]),
        &["rules[0]", "year", "between", "value2"],
    );

    // bad dates
    for bad in [
        "2020-13-01",
        "2021-02-29",
        "2020-1-01",
        "01/02/2020",
        "not-a-date",
        "",
    ] {
        assert_rejected(
            &c.library,
            &at(vec![rule(Field::LastPlayed, Op::Before, s(bad))]),
            &["rules[0]", "lastPlayed", "before"],
        );
    }
    // before/after need a date *string*
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::LastPlayed, Op::After, n(1_577_836_800.0))]),
        &["rules[0]", "lastPlayed", "after", "YYYY-MM-DD"],
    );

    // inLastDays needs days > 0
    for bad in [0.0, -3.0] {
        assert_rejected(
            &c.library,
            &at(vec![rule(Field::DateAdded, Op::InLastDays, n(bad))]),
            &["rules[0]", "dateAdded", "inLastDays", "greater than 0"],
        );
    }
    assert_rejected(
        &c.library,
        &at(vec![rule(Field::DateAdded, Op::InLastDays, s("soon"))]),
        &["rules[0]", "dateAdded", "inLastDays", "days"],
    );
}

// ---------------------------------------------------------------------------
// Injection safety
// ---------------------------------------------------------------------------

#[test]
fn injection_attempts_are_treated_as_literals() {
    let c = rule_fixture("injection");
    let payload = "x\"; DROP TABLE tracks; --";
    insert(&c.raw, Row::new(payload));

    // `is` matches only the literal row…
    let hits = preview_titles(
        &c.library,
        &view(
            "t",
            group(MatchMode::All, vec![rule(Field::Title, Op::Is, s(payload))]),
        ),
    );
    assert_eq!(hits, strs(&[payload]));

    // …and a second classic payload finds nothing (it's a literal contains)
    let sql = "'; DELETE FROM tracks WHERE '1'='1";
    let hits = preview_titles(
        &c.library,
        &view(
            "t",
            group(
                MatchMode::All,
                vec![rule(Field::Title, Op::Contains, s(sql))],
            ),
        ),
    );
    assert!(hits.is_empty());

    // the library is entirely intact afterwards
    assert_eq!(c.library.stats().unwrap().tracks, 8);
    let everything = view("all", group(MatchMode::All, vec![]));
    assert_eq!(c.library.smart_view_preview(&everything).unwrap().len(), 8);

    // saved views survive too, and saving the payload as a name/def is safe
    let mut def = everything.clone();
    def.name = payload.into();
    let id = c.library.smart_view_save(None, &def).unwrap();
    assert_eq!(c.library.smart_views_list().unwrap().len(), 1);
    assert_eq!(run_titles(&c.library, id).len(), 8);
}

#[test]
fn like_wildcards_in_values_match_literally() {
    let c = rule_fixture("wildcards");
    let one = |op, value: &str| {
        preview_titles(
            &c.library,
            &view(
                "t",
                group(MatchMode::All, vec![rule(Field::Title, op, s(value))]),
            ),
        )
    };

    // "%" and "_" are literals, not wildcards
    assert_eq!(one(Op::Contains, "%"), strs(&["100% Blues"]));
    assert_eq!(one(Op::Contains, "100%"), strs(&["100% Blues"]));
    assert_eq!(one(Op::StartsWith, "100"), strs(&["100% Blues"]));
    assert_eq!(one(Op::Is, "100% Blues"), strs(&["100% Blues"]));
    assert!(one(Op::Contains, "_").is_empty());
    assert_eq!(one(Op::NotContains, "%").len(), 6);
    // backslash is literal too
    assert!(one(Op::Contains, "\\").is_empty());
}

// ---------------------------------------------------------------------------
// Persistence round-trip
// ---------------------------------------------------------------------------

#[test]
fn save_update_delete_and_list_preserve_defs_exactly() {
    let c = rule_fixture("persistence");

    let mut def = view(
        "Chill Jazz",
        group(
            MatchMode::Any,
            vec![
                rule(Field::Genre, Op::Is, s("Jazz")),
                RuleRow {
                    field: Field::Year,
                    op: Op::Between,
                    value: n(1990.0),
                    value2: Some(s("1999")), // string bound survives the round-trip
                },
            ],
        ),
    );
    def.limit = Some(250.0);
    def.sort = SortField::MostPlayed;
    def.sort_desc = true;

    // create
    let id = c.library.smart_view_save(None, &def).unwrap();
    assert_eq!(
        c.library.smart_views_list().unwrap(),
        vec![SavedSmartView {
            id,
            def: def.clone()
        }]
    );

    // update in place: same id, new def
    let mut updated = def.clone();
    updated.name = "Chill Jazz v2".into();
    updated.limit = Some(10.0);
    updated.sort = SortField::RecentlyPlayed;
    updated.group.rules[0] = rule(Field::Genre, Op::IsNot, s("Jazz"));
    assert_eq!(c.library.smart_view_save(Some(id), &updated).unwrap(), id);
    assert_eq!(
        c.library.smart_views_list().unwrap(),
        vec![SavedSmartView {
            id,
            def: updated.clone()
        }]
    );

    // a second view gets its own id; list order is creation order
    let id2 = c.library.smart_view_save(None, &def).unwrap();
    assert_ne!(id, id2);
    assert_eq!(
        c.library
            .smart_views_list()
            .unwrap()
            .iter()
            .map(|v| v.id)
            .collect::<Vec<_>>(),
        vec![id, id2]
    );

    // run works while saved, fails after deletion
    assert!(!run_titles(&c.library, id).is_empty());
    c.library.smart_view_delete(id2).unwrap();
    assert_eq!(c.library.smart_views_list().unwrap().len(), 1);
    let err = c.library.smart_view_run(id2).unwrap_err().to_string();
    assert!(err.contains("not found"), "{err}");
    // deleting an unknown id is a no-op, not an error
    c.library.smart_view_delete(9_999).unwrap();

    // upsert semantics: saving with a vanished id re-inserts under it
    assert_eq!(c.library.smart_view_save(Some(9_999), &def).unwrap(), 9_999);
    assert_eq!(c.library.smart_views_list().unwrap().len(), 2);

    // previews never persist anything
    c.library.smart_view_preview(&def).unwrap();
    assert_eq!(c.library.smart_views_list().unwrap().len(), 2);
}

#[test]
fn published_playlist_mapping_round_trips_and_cascades() {
    let c = rule_fixture("publish-mapping");
    let def = view(
        "Mix",
        group(
            MatchMode::All,
            vec![rule(Field::Artist, Op::Is, s("Björk"))],
        ),
    );
    let id = c.library.smart_view_save(None, &def).unwrap();
    assert_eq!(c.library.smart_view_playlist_id(id).unwrap(), None);

    c.library.set_smart_view_playlist_id(id, "pl-1").unwrap();
    assert_eq!(
        c.library.smart_view_playlist_id(id).unwrap().as_deref(),
        Some("pl-1")
    );

    // Re-publishing replaces the remembered id (upsert, not a second row).
    c.library.set_smart_view_playlist_id(id, "pl-2").unwrap();
    assert_eq!(
        c.library.smart_view_playlist_id(id).unwrap().as_deref(),
        Some("pl-2")
    );

    // Deleting the view drops the mapping with it.
    c.library.smart_view_delete(id).unwrap();
    assert_eq!(c.library.smart_view_playlist_id(id).unwrap(), None);
}

// ---------------------------------------------------------------------------
// Scanner coexistence
// ---------------------------------------------------------------------------

#[test]
fn scans_leave_smart_views_untouched() {
    let c = rule_fixture("scan-coexist");

    // a real file, scanned the normal way, that the view will pick up
    let music = c.scratch.dir.join("music");
    common::write_mp3(
        &music.join("fresh.mp3"),
        50,
        &common::Meta {
            title: "Fresh Rock".into(),
            artist: "New".into(),
            album_artist: "New".into(),
            album: "Rocks".into(),
            genre: "Rock".into(),
            year: 2026,
            track: 1,
            disc: 1,
        },
        1_700_000_000,
    );

    let def = view(
        "Rock only",
        group(MatchMode::All, vec![rule(Field::Genre, Op::Is, s("Rock"))]),
    );
    let id = c.library.smart_view_save(None, &def).unwrap();
    assert_eq!(run_titles(&c.library, id), strs(&["Alpha", "beta"]));

    let report = c.library.scan_roots(&[music], |_| {}).unwrap();
    assert_eq!(report.added, 1, "{report:?}");

    // the view survived the scan byte-for-byte…
    assert_eq!(
        c.library.smart_views_list().unwrap(),
        vec![SavedSmartView {
            id,
            def: def.clone()
        }]
    );
    // …and its live evaluation now includes the scanned file
    assert_eq!(
        run_titles(&c.library, id),
        strs(&["Alpha", "beta", "Fresh Rock"])
    );

    // direct-insert rows are untouched by the scan (different root)
    assert_eq!(c.library.stats().unwrap().tracks, 8);
}
