//! The parameter registry: the track expression curves and the numeric
//! note parameters, each with unit, range, default and description, so
//! clients can introspect what they may edit.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Names a track curve parameter: `pitd`, `dyn`, `gen`, `bre`, `bri` or
/// `tone` (see [`curves`]). Ops naming anything else are rejected.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ParamId(pub String);

impl ParamId {
    /// Creates a parameter id from its name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The parameter's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The registry entry for this parameter, if it is known.
    pub fn descriptor(&self) -> Option<&'static ParamDescriptor> {
        curve(&self.0)
    }
}

impl Borrow<str> for ParamId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ParamId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Describes one parameter: a track curve or a numeric note field.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ParamDescriptor {
    /// Stable identifier: the curve id, or the note field path such as
    /// `"portamento.length_ms"` (fields of `Note`, `Timing` and `NotePitch`).
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Unit of the value.
    pub unit: &'static str,
    /// Smallest allowed value.
    pub min: f64,
    /// Largest allowed value.
    pub max: f64,
    /// Value when nothing is set.
    pub default: f64,
    /// What the parameter controls.
    pub description: &'static str,
}

impl ParamDescriptor {
    /// Whether `v` is finite and within `min..=max`.
    pub fn accepts(&self, v: f64) -> bool {
        v.is_finite() && v >= self.min && v <= self.max
    }
}

/// Track curve ids.
pub mod known {
    /// Pitch deviation in cents, added to the note pitch.
    pub const PITD: &str = "pitd";
    /// Loudness in dB.
    pub const DYN: &str = "dyn";
    /// Gender / formant shift in %.
    pub const GEN: &str = "gen";
    /// Breathiness in %.
    pub const BRE: &str = "bre";
    /// Brightness / tension in %.
    pub const BRI: &str = "bri";
    /// Pitch-independent timbre shift in semitones.
    pub const TONE: &str = "tone";
}

macro_rules! d {
    ($id:expr, $name:literal, $unit:literal, $min:expr, $max:expr, $default:expr, $desc:literal) => {
        ParamDescriptor {
            id: $id,
            name: $name,
            unit: $unit,
            min: $min,
            max: $max,
            default: $default,
            description: $desc,
        }
    };
}

const CURVES: &[ParamDescriptor] = &[
    d!(
        known::PITD,
        "Pitch deviation",
        "cents",
        -1200.0,
        1200.0,
        0.0,
        "Added to the note pitch (after portamento, vibrato and bends); for what the note parameters cannot express."
    ),
    d!(
        known::DYN,
        "Dynamics",
        "dB",
        -24.0,
        12.0,
        0.0,
        "Loudness offset."
    ),
    d!(
        known::GEN,
        "Gender",
        "%",
        -100.0,
        100.0,
        0.0,
        "Formant shift; positive is deeper / more masculine, negative brighter / more feminine."
    ),
    d!(
        known::BRE,
        "Breathiness",
        "%",
        0.0,
        100.0,
        0.0,
        "Mix of aperiodic (breath) noise into the voice."
    ),
    d!(
        known::BRI,
        "Brightness",
        "%",
        -100.0,
        100.0,
        0.0,
        "Spectral tilt; positive is brighter and tenser, negative darker and softer."
    ),
    d!(
        known::TONE,
        "Tone",
        "semitones",
        -12.0,
        12.0,
        0.0,
        "Timbre shift independent of pitch, as if the sample were sung higher or lower."
    ),
];

const NOTE_PARAMS: &[ParamDescriptor] = &[
    d!(
        "vel",
        "Velocity",
        "%",
        0.0,
        200.0,
        100.0,
        "Consonant velocity; above 100 shortens the consonant, below lengthens it."
    ),
    d!("vol", "Volume", "%", 0.0, 200.0, 100.0, "Note intensity."),
    d!(
        "mod",
        "Modulation",
        "%",
        0.0,
        100.0,
        0.0,
        "How much of the sample's own pitch wobble is kept."
    ),
    d!(
        "timing.preutter_ms",
        "Pre-utterance",
        "ms",
        0.0,
        2000.0,
        0.0,
        "How early the sample starts before the note; unset uses the oto.ini value."
    ),
    d!(
        "timing.overlap_ms",
        "Overlap",
        "ms",
        -1000.0,
        2000.0,
        0.0,
        "Crossfade with the previous note; unset uses the oto.ini value."
    ),
    d!(
        "timing.offset_ms",
        "Start offset",
        "ms",
        -1000.0,
        1000.0,
        0.0,
        "Extra offset into the sample."
    ),
    d!(
        "portamento.length_ms",
        "Portamento length",
        "ms",
        0.0,
        1000.0,
        80.0,
        "Duration of the glide from the previous note; 0 is a hard step."
    ),
    d!(
        "portamento.start_ms",
        "Portamento start",
        "ms",
        -1000.0,
        1000.0,
        -40.0,
        "Where the glide starts relative to the note start; negative is before."
    ),
    d!(
        "portamento.overshoot_cents",
        "Overshoot",
        "cents",
        0.0,
        200.0,
        0.0,
        "How far the glide goes past the target before settling."
    ),
    d!(
        "portamento.preparation_cents",
        "Preparation",
        "cents",
        0.0,
        200.0,
        0.0,
        "Dip opposite to the jump just before the glide."
    ),
    d!(
        "scoop.cents",
        "Scoop depth",
        "cents",
        -1200.0,
        1200.0,
        -150.0,
        "Pitch the note starts at relative to its target; negative is below."
    ),
    d!(
        "scoop.length_ms",
        "Scoop length",
        "ms",
        0.0,
        2000.0,
        100.0,
        "Time to bend from the scoop start to the target."
    ),
    d!(
        "fall.cents",
        "Fall depth",
        "cents",
        -1200.0,
        1200.0,
        -300.0,
        "Pitch reached at the note's end; negative drops."
    ),
    d!(
        "fall.length_ms",
        "Fall length",
        "ms",
        0.0,
        2000.0,
        150.0,
        "Duration of the fall, ending at the note's end."
    ),
    d!(
        "vibrato.length",
        "Vibrato length",
        "%",
        0.0,
        100.0,
        65.0,
        "Share of the note, from its end, that vibrates."
    ),
    d!(
        "vibrato.period_ms",
        "Vibrato period",
        "ms",
        50.0,
        1000.0,
        175.0,
        "One cycle (1000 / rate in Hz); 150..220 is typical."
    ),
    d!(
        "vibrato.depth_cents",
        "Vibrato depth",
        "cents",
        0.0,
        200.0,
        20.0,
        "Half the peak-to-peak swing."
    ),
    d!(
        "vibrato.fade_in",
        "Vibrato fade-in",
        "%",
        0.0,
        100.0,
        30.0,
        "Share of the vibrato over which depth grows from 0."
    ),
    d!(
        "vibrato.fade_out",
        "Vibrato fade-out",
        "%",
        0.0,
        100.0,
        20.0,
        "Share of the vibrato over which depth shrinks to 0."
    ),
    d!(
        "vibrato.phase",
        "Vibrato phase",
        "cycles",
        0.0,
        1.0,
        0.0,
        "Starting phase."
    ),
    d!(
        "vibrato.shift",
        "Vibrato shift",
        "%",
        -100.0,
        100.0,
        0.0,
        "Center offset as a share of depth."
    ),
    d!(
        "vibrato.drift",
        "Vibrato drift",
        "ratio",
        0.0,
        1.0,
        0.0,
        "Slow random wander of rate and depth, seeded by the note id."
    ),
    d!(
        "points.cents",
        "Bend point",
        "cents",
        -1200.0,
        1200.0,
        0.0,
        "Pitch offset of an extra bend point."
    ),
];

/// The track expression curves.
pub fn curves() -> &'static [ParamDescriptor] {
    CURVES
}

/// The numeric note parameters, by field path.
pub fn note_params() -> &'static [ParamDescriptor] {
    NOTE_PARAMS
}

/// Looks up a track curve by id.
pub fn curve(id: &str) -> Option<&'static ParamDescriptor> {
    CURVES.iter().find(|d| d.id == id)
}

/// Looks up a note parameter by field path.
pub fn note_param(id: &str) -> Option<&'static ParamDescriptor> {
    NOTE_PARAMS.iter().find(|d| d.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Note, Portamento, Vibrato};

    #[test]
    fn defaults_lie_in_range() {
        for d in curves().iter().chain(note_params()) {
            assert!(d.accepts(d.default), "{}", d.id);
        }
    }

    #[test]
    fn note_defaults_match_the_model() {
        let n = Note::new(0, 480, 60, "a");
        let (p, v) = (Portamento::default(), Vibrato::default());
        let get = |id| note_param(id).unwrap().default;
        assert_eq!(get("vel"), n.vel);
        assert_eq!(get("vol"), n.vol);
        assert_eq!(get("mod"), n.mod_);
        assert_eq!(get("portamento.length_ms"), p.length_ms);
        assert_eq!(get("portamento.start_ms"), p.start_ms);
        assert_eq!(get("vibrato.length"), v.length);
        assert_eq!(get("vibrato.period_ms"), v.period_ms);
        assert_eq!(get("vibrato.depth_cents"), v.depth_cents);
        assert_eq!(get("vibrato.fade_in"), v.fade_in);
        assert_eq!(get("vibrato.fade_out"), v.fade_out);
    }
}
