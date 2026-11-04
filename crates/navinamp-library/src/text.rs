//! Tag charset repair: legacy tag encodings decoded as Latin-1.
//!
//! Cyrillic MP3s commonly carry ID3v1 or ISO-8859-1-declared ID3v2 frames
//! whose bytes are actually **CP1251** (Windows-1251) or **KOI8-R**. Tag
//! readers follow the declared Latin-1 encoding, which maps those bytes 1:1
//! into U+00xx — the library then stores mojibake like
//! `Ïåñíÿ î ëþáâè` instead of `Песня о любви`.
//!
//! [`repair_tag_text`] recognizes that shape and decodes the bytes back into
//! Unicode. It also fixes UTF-8 bytes that a reader decoded as Latin-1
//! (`ÐœÐ¾ÑÐºÐ²Ð°` → `Москва`).
//!
//! Detection is deliberately conservative:
//! - Latin-1/CP1252-mapped text (chars ≤ U+00FF) and Windows-1250-mapped
//!   text (Central-European readers surface Latin Extended-A letters such as
//!   `ă`) are reconstructed; proper Unicode and pure ASCII pass through
//!   untouched, so the repair is idempotent;
//! - high bytes must make up a meaningful share of the text's LETTERS
//!   (~25%), which keeps `Motörhead`, `Björk` or `Été à Zürich` safe while
//!   still repairing the common mixed shape `DJ … feat. <Russian title>`
//!   (counting punctuation/spaces instead would dilute it);
//! - a candidate must decode into plausible Cyrillic (letter-frequency
//!   scored). CP1251 and KOI8-R are both decoded; a near-tie or a degenerate
//!   (near-zero-frequency) result is treated as ambiguous and left alone,
//!   which keeps majority-accented Latin text out of the repair path;
//! - a short Latin run glued to one edge of a Cyrillic word is tolerated
//!   (`Nуггнойй`, `xто`), while Latin letters mixed *through* a word
//!   (`Йtй` from `Été`) still reject the candidate.
//!
//! Unfixable by nature: UTF-16 bytes decoded as another wide encoding (the
//! result is CJK-looking text with no reliable inverse) — left alone.
//! Also inherently ambiguous: text made up ENTIRELY of accented Latin
//! letters that happens to decode cleanly under a legacy Cyrillic codepage
//! (e.g. an all-caps Scandinavian title) — without a language model there is
//! no byte-level distinction, so such strings may be "repaired" wrongly.

use std::borrow::Cow;

use encoding_rs::{KOI8_R, WINDOWS_1250, WINDOWS_1251};

/// Approximate Russian letter frequencies (percent). Used to pick between
/// CP1251 and KOI8-R when both decodings produce Cyrillic: only the correct
/// byte mapping yields the high-frequency letters (о, е, а, и, н, т, …).
fn russian_weight(c: char) -> f32 {
    match c.to_lowercase().next().unwrap_or(c) {
        'о' => 10.9,
        'е' => 8.5,
        'а' => 8.0,
        'и' => 7.4,
        'н' => 6.7,
        'т' => 6.3,
        'с' => 5.5,
        'р' => 4.7,
        'в' => 4.5,
        'л' => 4.4,
        'к' => 3.5,
        'м' => 3.2,
        'д' => 3.0,
        'п' => 2.8,
        'у' => 2.6,
        'я' => 2.0,
        'ы' => 1.9,
        'ь' => 1.7,
        'г' => 1.7,
        'з' => 1.6,
        'б' => 1.6,
        'ч' => 1.4,
        'й' => 1.2,
        'х' => 1.0,
        'ж' => 0.9,
        'ш' => 0.7,
        'ю' => 0.6,
        'ц' => 0.5,
        'щ' => 0.4,
        'э' => 0.3,
        'ф' => 0.3,
        'ъ' | 'ё' => 0.04,
        _ => 0.0,
    }
}

fn score(s: &str) -> f32 {
    s.chars().map(russian_weight).sum()
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

/// Tag text may legitimately contain letters, digits, whitespace and the
/// usual punctuation; anything else after a candidate decode is a red flag
/// (wrong codepage or garbage bytes). ASCII punctuation is all accepted;
/// common typographic marks (middle dot, ellipsis, dashes, quotes) are the
/// shapes real tags use.
fn useful(c: char) -> bool {
    c.is_alphanumeric()
        || c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(
            c,
            '…' | '–' | '—' | '’' | '‘' | '“' | '”' | '«' | '»' | '·' | '•' | '№' | '°'
        )
}

/// A candidate decoding is plausible when it is printable and its LETTERS
/// do not mix scripts *through* a word:
///
/// - a word may be pure Latin/ASCII or pure Cyrillic;
/// - a short (≤2 letters) contiguous Latin run glued to one edge of an
///   otherwise-Cyrillic word is tolerated — pirate tags often have shapes
///   like `Nуггнойй` or `xто`, and rejecting them left whole rows garbled;
/// - accented-Latin false positives still fail: `Йtй` (from `Été`) puts the
///   Latin letter in the middle, and `Ьber`/`Niсo` have a Latin majority;
/// - at least one Cyrillic word of two or more letters, or — for tags that
///   are a single Cyrillic letter (`Я`) — no Latin letters at all.
///
/// Mixed-language titles stay valid because the mixing is per WORD
/// (`DJ Slon feat. Песня`), which is exactly the common real-world shape.
fn plausible(s: &str) -> bool {
    if !s.chars().all(useful) {
        return false;
    }
    let mut cyrillic_letters = 0usize;
    let mut latin_letters = 0usize;
    let mut cyrillic_words = 0usize;
    for word in s.split(|c: char| !c.is_alphanumeric()) {
        let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
        if letters.is_empty() {
            continue;
        }
        let mut word_cyrillic = 0usize;
        let mut word_latin = 0usize;
        let mut latin_positions = Vec::new();
        for (index, c) in letters.iter().enumerate() {
            if is_cyrillic(*c) {
                word_cyrillic += 1;
            } else {
                word_latin += 1;
                latin_positions.push(index);
            }
        }
        if word_cyrillic > 0 && word_latin > 0 {
            let latin_prefix = word_latin <= 2
                && latin_positions
                    .iter()
                    .enumerate()
                    .all(|(k, position)| *position == k);
            let latin_suffix = word_latin <= 2
                && latin_positions
                    .iter()
                    .enumerate()
                    .all(|(k, position)| *position == letters.len() - word_latin + k);
            // Require an edge run and a Cyrillic majority; anything else is
            // alternating-script garbage (`Йtй`) or accented Latin.
            if !(latin_prefix || latin_suffix) || word_cyrillic <= word_latin {
                return false;
            }
        }
        cyrillic_letters += word_cyrillic;
        latin_letters += word_latin;
        if word_cyrillic >= 2 {
            cyrillic_words += 1;
        }
    }
    cyrillic_words > 0 || (cyrillic_letters > 0 && latin_letters == 0)
}

/// Best legacy-Cyrillic reading of one byte string: UTF-8 (a reader that
/// double-decoded UTF-8) wins outright; otherwise CP1251/KOI8-R are scored by
/// Russian letter frequency. Returns `None` when no reading is credible.
fn best_decoding(bytes: &[u8]) -> Option<(String, f32)> {
    if let Ok(decoded) = std::str::from_utf8(bytes)
        && plausible(decoded)
    {
        return Some((decoded.to_owned(), f32::MAX));
    }
    // Single-byte encodings are total, so both decodes always succeed;
    // unigram frequency picks the real one. CP1251 and KOI8-R mirror each
    // other and short tags can score close (a real KOI8-R title measured
    // 54.9 vs 53.3 for its CP1251 misdecode), so no margin rule is applied —
    // the corpus is overwhelmingly CP1251, and a margin would reject genuine
    // KOI8-R and mixed-prefix titles. Ties keep CP1251 (checked first).
    let (cp1251, _, _) = WINDOWS_1251.decode(bytes);
    let (koi8r, _, _) = KOI8_R.decode(bytes);
    let mut best: Option<(String, f32)> = None;
    for candidate in [cp1251.as_ref(), koi8r.as_ref()] {
        if !plausible(candidate) {
            continue;
        }
        let candidate_score = score(candidate);
        if candidate_score < 0.5 {
            continue; // degenerate decode (near-zero Russian frequency)
        }
        if best
            .as_ref()
            .is_none_or(|(_, score)| candidate_score > *score)
        {
            best = Some((candidate.to_owned(), candidate_score));
        }
    }
    best
}

/// Repair one tag string (title / artist / album / …). Returns the input
/// unchanged unless a credible legacy-Cyrillic decoding exists.
pub fn repair_tag_text(text: &str) -> Cow<'_, str> {
    let mut high = 0usize;
    let mut letters = 0usize;
    for c in text.chars() {
        if c.is_alphabetic() {
            letters += 1;
        }
        if (c as u32) >= 0x80 {
            high += 1;
        }
    }
    // Pure ASCII, or high bytes are only a fringe of the letters: not
    // mojibake. The 25% bar counts LETTERS (not spaces/punctuation) so
    // "DJ Slon feat. <mangled title>" shapes still get repaired while a
    // single accent in a Latin name never does.
    if high == 0 || letters == 0 || high * 4 < letters {
        return Cow::Borrowed(text);
    }

    // The reader may have mapped the bytes through Latin-1/CP1252 (the usual
    // case, every char ≤ U+00FF) or through a Central-European codepage
    // (CP1250/ISO-8859-2), which surfaces as Latin Extended-A letters such
    // as `ă` (0xE3). Reconstruct both candidate byte strings; a string that
    // cannot be encoded (proper Unicode) yields no CP1250 candidate.
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    if text.chars().all(|c| (c as u32) <= 0xFF) {
        candidates.push(text.chars().map(|c| c as u8).collect());
    }
    let (cp1250, _, cp1250_errors) = WINDOWS_1250.encode(text);
    if !cp1250_errors {
        let bytes = cp1250.into_owned();
        if !candidates.contains(&bytes) {
            candidates.push(bytes);
        }
    }

    let mut best: Option<(String, f32)> = None;
    for bytes in &candidates {
        if let Some((decoded, candidate_score)) = best_decoding(bytes)
            && best
                .as_ref()
                .is_none_or(|(_, score)| candidate_score > *score)
        {
            best = Some((decoded, candidate_score));
        }
    }
    match best {
        Some((decoded, _)) => Cow::Owned(decoded),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact corruption a Latin-1-happy reader applies to legacy bytes.
    fn as_latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|b| char::from(*b)).collect()
    }

    fn cp1251(good: &str) -> String {
        let (bytes, _, _) = WINDOWS_1251.encode(good);
        as_latin1(&bytes)
    }

    fn koi8r(good: &str) -> String {
        let (bytes, _, _) = KOI8_R.encode(good);
        as_latin1(&bytes)
    }

    /// A Central-European (CP1250) reader mapping the same legacy bytes.
    fn cp1250_mapped(bytes: &[u8]) -> String {
        let (text, _, _) = encoding_rs::WINDOWS_1250.decode(bytes);
        text.into_owned()
    }

    #[test]
    fn repairs_cp1251_title() {
        let good = "Сергей Жуков - Районы-кварталы";
        let garbled = cp1251(good);
        assert_ne!(garbled, good, "fixture really is mojibake");
        assert_eq!(repair_tag_text(&garbled), good);
    }

    #[test]
    fn repairs_koi8r_title() {
        let good = "Песня о любви";
        let garbled = koi8r(good);
        assert_ne!(garbled, good, "fixture really is mojibake");
        assert_eq!(repair_tag_text(&garbled), good);
    }

    #[test]
    fn repairs_cp1250_mapped_title() {
        // A Central-European reader maps the same CP1251 bytes through
        // CP1250; the result contains Latin Extended-A letters (`ă`), not
        // just U+00xx.
        let good = "Песня о любви";
        let (bytes, _, _) = WINDOWS_1251.encode(good);
        let garbled = cp1250_mapped(&bytes);
        assert_ne!(garbled, good, "fixture really is mojibake");
        assert_eq!(repair_tag_text(&garbled), good);
    }

    #[test]
    fn repairs_latin_prefixed_cyrillic_words() {
        // Pirate tags often glue a short Latin run to a Cyrillic word; a
        // whole-word script-purity rule left these rows garbled.
        for good in ["NПесня", "Dассиадэ", "xто"] {
            let garbled = cp1251(good);
            assert_ne!(garbled, good, "{good}: fixture really is mojibake");
            assert_eq!(repair_tag_text(&garbled), good);
        }
    }

    #[test]
    fn keeps_cp1250_accented_latin() {
        for name in ["Kőszönöm", "Ťažký", "Žluťoučký kůň", "Ábel"] {
            assert_eq!(repair_tag_text(name), name, "{name} must stay untouched");
        }
    }

    #[test]
    fn repairs_utf8_decoded_as_latin1() {
        let good = "Москва";
        let garbled = as_latin1(good.as_bytes());
        assert_ne!(garbled, good, "fixture really is mojibake");
        assert_eq!(repair_tag_text(&garbled), good);
    }

    #[test]
    fn repairs_reported_remote_tag_shapes() {
        // Shapes from a real Navidrome library: CP1251 bytes read through
        // CP1252, including short Latin runs glued to Cyrillic words (which
        // the pre-v2 whole-word purity rule rejected).
        for (garbled, expected) in [
            ("Nóããíîéé àä·ä", "Nуггнойй ад·д"),
            ("1,5 ëà îóèéé·ïðàâà", "1,5 ла оуийй·права"),
            ("xòî lîäý /Eàäò", "xто lодэ /Eадт"),
            ("Dàññèàäý", "Dассиадэ"),
            ("Ìàðàäîíè", "Марадони"),
        ] {
            assert_eq!(repair_tag_text(garbled), expected, "{garbled}");
        }
    }

    #[test]
    fn keeps_ascii_and_mixed_ascii_titles() {
        assert_eq!(repair_tag_text("Hello World"), "Hello World");
        // Mostly-ASCII with a Cyrillic prefix: the ASCII part stays.
        let good = "DJ Песня";
        let garbled = cp1251(good);
        assert_eq!(repair_tag_text(&garbled), good);
    }

    #[test]
    fn repairs_titles_behind_long_ascii_prefixes() {
        // The common feature/collab shape: the Latin prefix is longer than
        // the Cyrillic part, so a whitespace-wide dominance gate would miss
        // exactly the tags this repair exists for.
        for good in [
            "DJ Slon feat. Песня",
            "Bad Sector - Ангел",
            "VA - Сборник 2004",
        ] {
            let garbled = cp1251(good);
            assert_ne!(garbled, good, "fixture really is mojibake");
            assert_eq!(repair_tag_text(&garbled), good);
        }
    }

    #[test]
    fn repairs_short_cyrillic_words() {
        assert_eq!(repair_tag_text(&cp1251("Я")), "Я");
        assert_eq!(repair_tag_text(&cp1251("Ёж")), "Ёж");
    }

    #[test]
    fn keeps_mixed_script_false_positives() {
        // A CP1251 decode of genuine accented Latin alternates scripts inside
        // words ("Йtй"), which the plausibility gate rejects.
        for name in [
            "Été à Zürich",
            "Hüsker Dü",
            "Niño",
            "Coração",
            "Motörhead",
            "Björk",
            "Café",
            "Über",
        ] {
            assert_eq!(repair_tag_text(name), name, "{name} must stay untouched");
        }
    }

    #[test]
    fn keeps_proper_unicode() {
        assert_eq!(repair_tag_text("Сергей Жуков"), "Сергей Жуков");
        assert_eq!(repair_tag_text("東京"), "東京");
    }

    #[test]
    fn keeps_accented_latin_names() {
        for name in ["Motörhead", "Björk", "Été à Zürich", "Café", "Über"] {
            assert_eq!(repair_tag_text(name), name, "{name} must stay untouched");
        }
    }

    #[test]
    fn keeps_empty_and_blank() {
        assert_eq!(repair_tag_text(""), "");
        assert_eq!(repair_tag_text("   "), "   ");
    }

    #[test]
    fn is_idempotent() {
        let good = "Сергей Жуков - Районы-кварталы";
        let once = repair_tag_text(&cp1251(good)).into_owned();
        assert_eq!(repair_tag_text(&once), good);
    }
}
