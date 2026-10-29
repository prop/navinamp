//! ReplayGain tag scanning and gain selection (applied at track open).
//!
//! # Which tags, and the clamp math
//!
//! Tags are read with lofty from the same bytes symphonia will decode — a
//! second cheap pass on the producer thread at track open (headers only,
//! properties parsing disabled). Four standard frames/comments are honoured
//! (lofty maps all of them to [`ItemKey`]s from ID3v2 `TXXX`, Vorbis
//! comments, APE and MP4 freeform): `REPLAYGAIN_ALBUM_GAIN`,
//! `REPLAYGAIN_ALBUM_PEAK`, `REPLAYGAIN_TRACK_GAIN`,
//! `REPLAYGAIN_TRACK_PEAK`. Values arrive as strings like `"-6.48 dB"`; the
//! `dB` suffix (any case) is stripped before parsing.
//!
//! Selection: **album gain preferred, track gain fallback** — the album
//! pair must have its gain present to win (a lone album peak proves
//! nothing), otherwise a track gain applies, otherwise the gain is 1.0.
//! Peak protection limits the chosen linear gain `g = 10^(dB/20)` so that
//! `peak * g <= 1.0`, using the peak that belongs to the chosen gain
//! (album peak for album gain, track peak for track gain). A missing peak
//! is treated as full scale (`1.0`), so an unpeak-tagged file can never be
//! boosted above 0 dBFS — attenuation always applies as tagged. The result
//! is floored at 0 (f32 underflow on absurd attenuation tags); the peak
//! rule is the only upper bound by design (`peak * gain == 1.0` is safe at
//! any magnitude).
//!
//! The resulting scalar is applied by the DSP chain *before* preamp/EQ; see
//! [`crate::dsp`] for where it enters the output path.

use std::io::{BufReader, Read, Seek};

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

/// Parsed ReplayGain tag values (all optional).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct RgTags {
    album_gain_db: Option<f32>,
    album_peak: Option<f32>,
    track_gain_db: Option<f32>,
    track_peak: Option<f32>,
}

/// Parse a ReplayGain gain string: `"-6.48 dB"`, `"+2 dB"`, `"-6.48"` —
/// an optional `dB`/`db` suffix, optional sign, finite float. Anything else
/// (empty, garbage, NaN) is `None`.
fn parse_db(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    let number = trimmed
        .strip_suffix("dB")
        .or_else(|| trimmed.strip_suffix("db"))
        .or_else(|| trimmed.strip_suffix("DB"))
        .unwrap_or(trimmed)
        .trim();
    let db: f32 = number.parse().ok()?;
    db.is_finite().then_some(db)
}

/// Parse a ReplayGain peak string (a linear 0..~2 sample-scale float).
fn parse_peak(value: &str) -> Option<f32> {
    let p: f32 = value.trim().parse().ok()?;
    p.is_finite().then_some(p)
}

impl RgTags {
    /// The applied linear gain; see the module docs for the selection and
    /// clamping rules.
    fn applied_gain(&self) -> f32 {
        let (gain_db, peak) = match (self.album_gain_db, self.track_gain_db) {
            (Some(album), _) => (album, self.album_peak),
            (None, Some(track)) => (track, self.track_peak),
            (None, None) => return 1.0,
        };
        let mut gain = 10f32.powf(gain_db / 20.0);
        // Peak protection: peak * gain <= 1.0 (missing peak = full scale).
        let peak = peak.unwrap_or(1.0).max(f32::MIN_POSITIVE);
        gain = gain.min(1.0 / peak);
        // Floor only (f32 underflow on absurd attenuation tags); the peak
        // rule above is the real upper bound.
        gain.max(0.0)
    }
}

/// Read the ReplayGain tags from `reader` (any seekable media; the reader is
/// left at an arbitrary position — callers rewind). Best-effort: any lofty
/// error means "no tags", i.e. a gain of 1.0, never a failed track open.
fn scan_tags<R: Read + Seek>(reader: &mut R) -> Option<RgTags> {
    let probe = Probe::new(BufReader::new(reader))
        .options(ParseOptions::new().read_properties(false))
        .guess_file_type()
        .ok()?;
    let tagged = probe.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let get_db = |key: ItemKey| tag.get_string(key).and_then(parse_db);
    let get_peak = |key: ItemKey| tag.get_string(key).and_then(parse_peak);
    Some(RgTags {
        album_gain_db: get_db(ItemKey::ReplayGainAlbumGain),
        album_peak: get_peak(ItemKey::ReplayGainAlbumPeak),
        track_gain_db: get_db(ItemKey::ReplayGainTrackGain),
        track_peak: get_peak(ItemKey::ReplayGainTrackPeak),
    })
}

/// Scan `reader` for ReplayGain tags and return the scalar the DSP chain
/// should apply for this track. Never fails.
pub(crate) fn scan<R: Read + Seek>(reader: &mut R) -> f32 {
    scan_tags(reader).map_or(1.0, |tags| tags.applied_gain())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_db_strings() {
        assert_eq!(parse_db("-6.48 dB"), Some(-6.48));
        assert_eq!(parse_db("-6.48dB"), Some(-6.48));
        assert_eq!(parse_db("  +2.0 db "), Some(2.0));
        assert_eq!(parse_db("-6.48"), Some(-6.48));
        assert_eq!(parse_db("0"), Some(0.0));
        assert_eq!(parse_db(""), None);
        assert_eq!(parse_db("loud"), None);
        assert_eq!(parse_db("NaN"), None);
        assert_eq!(parse_db("inf"), None);
    }

    #[test]
    fn parses_peak_strings() {
        assert_eq!(parse_peak("0.987654"), Some(0.987654));
        assert_eq!(parse_peak("1.0"), Some(1.0));
        assert_eq!(parse_peak(""), None);
        assert_eq!(parse_peak("loud"), None);
    }

    #[test]
    fn missing_tags_are_unity() {
        assert_eq!(RgTags::default().applied_gain(), 1.0);
        // A lone peak (of either kind) is not a gain.
        assert_eq!(
            RgTags {
                album_peak: Some(0.5),
                track_peak: Some(0.9),
                ..RgTags::default()
            }
            .applied_gain(),
            1.0
        );
    }

    #[test]
    fn album_gain_preferred_over_track() {
        let tags = RgTags {
            album_gain_db: Some(-6.0),
            track_gain_db: Some(-18.0),
            album_peak: Some(0.5),
            track_peak: Some(0.5),
        };
        // Album wins: -6 dB ≈ 0.501 (peak 0.5 allows up to 2.0).
        let g = tags.applied_gain();
        assert!((g - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6, "{g}");
    }

    #[test]
    fn track_gain_used_when_album_missing() {
        let tags = RgTags {
            track_gain_db: Some(-6.0),
            track_peak: Some(0.25),
            ..RgTags::default()
        };
        // -6 dB ≈ 0.501, but track peak 0.25 caps the gain at 4.0 → unclamped.
        assert!((tags.applied_gain() - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        // With track peak 0.5: cap 2.0, still above 0.501 → unclamped.
        let tags = RgTags {
            track_gain_db: Some(-6.0),
            track_peak: Some(0.5),
            ..RgTags::default()
        };
        assert!((tags.applied_gain() - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
    }

    #[test]
    fn peak_limits_positive_gain() {
        // +12 dB = 3.98 linear; album peak 0.5 caps it at exactly 2.0.
        let tags = RgTags {
            album_gain_db: Some(12.0),
            album_peak: Some(0.5),
            ..RgTags::default()
        };
        assert_eq!(tags.applied_gain(), 2.0);
        // Track variant clamps with the track peak.
        let tags = RgTags {
            track_gain_db: Some(12.0),
            track_peak: Some(0.4),
            ..RgTags::default()
        };
        assert_eq!(tags.applied_gain(), 2.5);
    }

    #[test]
    fn missing_peak_forbids_boost_but_allows_cut() {
        // Positive gain without peak info: assume full scale → no boost.
        let tags = RgTags {
            album_gain_db: Some(4.0),
            ..RgTags::default()
        };
        assert_eq!(tags.applied_gain(), 1.0);
        // Negative gain without peak info applies in full.
        let tags = RgTags {
            track_gain_db: Some(-9.0),
            ..RgTags::default()
        };
        assert!((tags.applied_gain() - 10f32.powf(-9.0 / 20.0)).abs() < 1e-6);
    }

    #[test]
    fn absurd_tags_are_bounded() {
        // An enormous gain with a real peak is bounded by the peak rule
        // (+60 dB = 31.6 linear; album peak 8.0 caps it at 1/8).
        let tags = RgTags {
            album_gain_db: Some(60.0),
            album_peak: Some(8.0),
            ..RgTags::default()
        };
        assert_eq!(tags.applied_gain(), 0.125);
        // A deep attenuation tag underflows to zero, never negative.
        let tags = RgTags {
            album_gain_db: Some(-1000.0),
            ..RgTags::default()
        };
        assert_eq!(tags.applied_gain(), 0.0);
    }
}
