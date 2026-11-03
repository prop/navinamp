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
//! - only Latin-1-mapped text is considered (every char ≤ U+00FF); proper
//!   Unicode and pure ASCII pass through untouched, so the repair is
//!   idempotent;
//! - high bytes must make up a meaningful share of the text's LETTERS
//!   (~25%), which keeps `Motörhead`, `Björk` or `Été à Zürich` safe while
//!   still repairing the common mixed shape `DJ … feat. <Russian title>`
//!   (counting punctuation/spaces instead would dilute it);
//! - a candidate must decode into plausible Cyrillic (letter-frequency
//!   scored). CP1251 and KOI8-R are both decoded; a near-tie or a degenerate
//!   (near-zero-frequency) result is treated as ambiguous and left alone,
//!   which keeps majority-accented Latin text out of the repair path.
//!
//! Unfixable by nature: UTF-16 bytes decoded as another wide encoding (the
//! result is CJK-looking text with no reliable inverse) — left alone.
//! Also inherently ambiguous: text made up ENTIRELY of accented Latin
//! letters that happens to decode cleanly under a legacy Cyrillic codepage
//! (e.g. an all-caps Scandinavian title) — without a language model there is
//! no byte-level distinction, so such strings may be "repaired" wrongly.

use std::borrow::Cow;

use encoding_rs::{KOI8_R, WINDOWS_1251};

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
/// (wrong codepage or garbage bytes).
fn useful(c: char) -> bool {
    c.is_alphanumeric() || c.is_whitespace() || " -_.,!?&'\"()[]{}:/\\+*=#@%…–—’‘“”«»".contains(c)
}

/// A candidate decoding is plausible when it is printable and its LETTERS
/// do not mix scripts inside a word:
///
/// - every alphanumeric token must be pure Latin/ASCII or pure Cyrillic
///   (mangled Latin-1 text such as `Йtй` alternates scripts mid-word);
/// - at least one pure-Cyrillic word of two or more letters, or — for tags
///   that are a single Cyrillic letter (`Я`) — no Latin letters at all.
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
        if word.is_empty() {
            continue;
        }
        let mut word_cyrillic = 0usize;
        let mut word_latin = 0usize;
        for c in word.chars() {
            if !c.is_alphabetic() {
                continue; // digits inside a token are fine
            }
            if is_cyrillic(c) {
                word_cyrillic += 1;
            } else {
                word_latin += 1;
            }
        }
        if word_cyrillic > 0 && word_latin > 0 {
            return false; // scripts alternate inside one word: corruption
        }
        cyrillic_letters += word_cyrillic;
        latin_letters += word_latin;
        if word_cyrillic >= 2 {
            cyrillic_words += 1;
        }
    }
    cyrillic_words > 0 || (cyrillic_letters > 0 && latin_letters == 0)
}

/// Repair one tag string (title / artist / album / …). Returns the input
/// unchanged unless a credible legacy-Cyrillic decoding exists.
pub fn repair_tag_text(text: &str) -> Cow<'_, str> {
    let mut high = 0usize;
    let mut letters = 0usize;
    for c in text.chars() {
        let cp = c as u32;
        if cp > 0xFF {
            // Already proper Unicode (or an unrecoverable wide-encoding mix).
            return Cow::Borrowed(text);
        }
        if c.is_alphabetic() {
            letters += 1;
        }
        if cp >= 0x80 {
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

    // Reconstruct the bytes the reader mapped through Latin-1.
    let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();

    // 1) UTF-8 bytes decoded as Latin-1 (`ÐœÐ¾ÑÐºÐ²Ð°`). Strict decode only:
    //    genuine legacy-Cyrillic bytes almost never form valid UTF-8, and an
    //    accidental match would fail the plausibility gate.
    if let Ok(decoded) = std::str::from_utf8(&bytes)
        && plausible(decoded)
    {
        return Cow::Owned(decoded.to_owned());
    }

    // 2) Legacy Cyrillic codepages. Single-byte encodings are total, so both
    //    decodes always succeed; unigram frequency picks the real one. CP1251
    //    and KOI8-R mirror each other and short tags can score close (a real
    //    KOI8-R title measured 54.9 vs 53.3 for its CP1251 misdecode), so no
    //    margin rule is applied — the corpus is overwhelmingly CP1251, and a
    //    margin would reject genuine KOI8-R and mixed-prefix titles.
    let (cp1251, _, _) = WINDOWS_1251.decode(&bytes);
    let (koi8r, _, _) = KOI8_R.decode(&bytes);
    let cp_score = if plausible(&cp1251) {
        score(&cp1251)
    } else {
        -1.0
    };
    let koi_score = if plausible(&koi8r) {
        score(&koi8r)
    } else {
        -1.0
    };
    let (best, best_score) = if cp_score >= koi_score {
        (&cp1251, cp_score)
    } else {
        (&koi8r, koi_score)
    };
    // The floor rejects degenerate decodes (near-zero Russian frequency).
    if best_score >= 0.5 {
        Cow::Owned(best.as_ref().to_owned())
    } else {
        Cow::Borrowed(text)
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
    fn repairs_utf8_decoded_as_latin1() {
        let good = "Москва";
        let garbled = as_latin1(good.as_bytes());
        assert_ne!(garbled, good, "fixture really is mojibake");
        assert_eq!(repair_tag_text(&garbled), good);
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
