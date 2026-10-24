//! The parameter registry: every curve parameter with unit, range and
//! default, so clients can introspect what they may edit.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Names a curve parameter, such as `"breathiness"`. Known parameters are
/// listed by [`registry`]; ops naming anything else return a diagnostic.
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
        lookup(&self.0)
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

/// Describes one curve parameter. `min`/`max`/`default` apply to the final
/// value `base + model_prediction + user_delta`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ParamDescriptor {
    /// Stable identifier used in ops and project files.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Unit of curve values.
    pub unit: &'static str,
    /// Smallest final value.
    pub min: f64,
    /// Largest final value.
    pub max: f64,
    /// Value when nothing is predicted or edited.
    pub default: f64,
    /// What the parameter controls.
    pub description: &'static str,
}

impl ParamDescriptor {
    /// Largest magnitude a delta layer may hold: the full span of the range.
    pub fn max_delta(&self) -> f64 {
        self.max - self.min
    }
}

/// Well-known parameter ids.
pub mod known {
    /// Pitch deviation from the note-derived base pitch.
    pub const PITCH: &str = "pitch";
    /// Loudness of the voice.
    pub const ENERGY: &str = "energy";
    /// Aspiration noise.
    pub const BREATHINESS: &str = "breathiness";
    /// Ratio of voiced to unvoiced excitation.
    pub const VOICING: &str = "voicing";
    /// Vocal effort.
    pub const TENSION: &str = "tension";
    /// Formant shift.
    pub const GENDER: &str = "gender";
    /// Consonant speed.
    pub const VELOCITY: &str = "velocity";
    /// Jaw and mouth opening.
    pub const MOUTH_OPENING: &str = "mouth_opening";
}

const REGISTRY: &[ParamDescriptor] = &[
    ParamDescriptor {
        id: known::PITCH,
        name: "Pitch deviation",
        unit: "cents",
        min: -2400.0,
        max: 2400.0,
        default: 0.0,
        description: "Offset from the base pitch derived from notes, portamento and vibrato.",
    },
    ParamDescriptor {
        id: known::ENERGY,
        name: "Energy",
        unit: "dB",
        min: -96.0,
        max: 12.0,
        default: 0.0,
        description: "Loudness of the voice relative to the model's neutral level.",
    },
    ParamDescriptor {
        id: known::BREATHINESS,
        name: "Breathiness",
        unit: "dB",
        min: -96.0,
        max: 12.0,
        default: 0.0,
        description: "Level of aspiration noise relative to the model's neutral level.",
    },
    ParamDescriptor {
        id: known::VOICING,
        name: "Voicing",
        unit: "dB",
        min: -96.0,
        max: 12.0,
        default: 0.0,
        description: "Level of the harmonic (voiced) component.",
    },
    ParamDescriptor {
        id: known::TENSION,
        name: "Tension",
        unit: "logit",
        min: -10.0,
        max: 10.0,
        default: 0.0,
        description: "Vocal effort; positive is tenser, negative is softer.",
    },
    ParamDescriptor {
        id: known::GENDER,
        name: "Gender",
        unit: "semitones",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        description: "Formant shift; positive sounds smaller and brighter.",
    },
    ParamDescriptor {
        id: known::VELOCITY,
        name: "Velocity",
        unit: "ratio",
        min: 0.25,
        max: 4.0,
        default: 1.0,
        description: "Consonant speed multiplier.",
    },
    ParamDescriptor {
        id: known::MOUTH_OPENING,
        name: "Mouth opening",
        unit: "ratio",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        description: "Jaw and mouth opening, where the backend supports it.",
    },
];

/// Every known curve parameter. Backend support flags arrive with singers.
pub fn registry() -> &'static [ParamDescriptor] {
    REGISTRY
}

/// Looks up a parameter by id.
pub fn lookup(id: &str) -> Option<&'static ParamDescriptor> {
    REGISTRY.iter().find(|d| d.id == id)
}
