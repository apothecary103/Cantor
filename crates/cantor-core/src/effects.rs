//! The effect registry: every built-in effect with its parameters, units,
//! ranges and defaults, so clients can introspect what they may set.
//!
//! An [`EffectSlot`](crate::EffectSlot) stores only the parameters it
//! changes; missing parameters take their defaults. Unknown parameters and
//! values outside a parameter's range are rejected with a diagnostic.

use crate::model::{EffectKind, EffectSlot};
use schemars::JsonSchema;
use serde::Serialize;

/// Describes one effect parameter.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct EffectParam {
    /// Stable identifier used in `EffectSlot::params`.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Unit of the value.
    pub unit: &'static str,
    /// Smallest value.
    pub min: f64,
    /// Largest value.
    pub max: f64,
    /// Value when the slot does not set it.
    pub default: f64,
    /// What the parameter controls.
    pub description: &'static str,
}

/// Describes one built-in effect.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct EffectDescriptor {
    /// The effect.
    pub kind: EffectKind,
    /// Human-readable name.
    pub name: &'static str,
    /// What the effect does.
    pub description: &'static str,
    /// Its parameters.
    pub params: &'static [EffectParam],
}

impl EffectDescriptor {
    /// Looks up a parameter by id.
    pub fn param(&self, id: &str) -> Option<&'static EffectParam> {
        self.params.iter().find(|p| p.id == id)
    }
}

macro_rules! p {
    ($id:literal, $name:literal, $unit:literal, $min:expr, $max:expr, $default:expr, $desc:literal) => {
        EffectParam {
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

const EQ: &[EffectParam] = &[
    p!(
        "high_pass_hz",
        "High-pass",
        "Hz",
        0.0,
        1000.0,
        0.0,
        "Second-order high-pass cutoff; 0 turns it off."
    ),
    p!(
        "low_freq_hz",
        "Low shelf frequency",
        "Hz",
        20.0,
        1000.0,
        120.0,
        "Corner of the low shelf."
    ),
    p!(
        "low_gain_db",
        "Low shelf gain",
        "dB",
        -24.0,
        24.0,
        0.0,
        "Boost or cut below the low shelf corner."
    ),
    p!(
        "mid_freq_hz",
        "Mid frequency",
        "Hz",
        100.0,
        10000.0,
        1000.0,
        "Centre of the peaking band."
    ),
    p!(
        "mid_gain_db",
        "Mid gain",
        "dB",
        -24.0,
        24.0,
        0.0,
        "Boost or cut of the peaking band."
    ),
    p!(
        "mid_q",
        "Mid Q",
        "ratio",
        0.1,
        10.0,
        1.0,
        "Bandwidth of the peaking band; higher is narrower."
    ),
    p!(
        "presence_freq_hz",
        "Presence frequency",
        "Hz",
        1000.0,
        12000.0,
        4000.0,
        "Centre of the presence band."
    ),
    p!(
        "presence_gain_db",
        "Presence gain",
        "dB",
        -24.0,
        24.0,
        0.0,
        "Boost or cut of the presence band."
    ),
    p!(
        "presence_q",
        "Presence Q",
        "ratio",
        0.1,
        10.0,
        1.0,
        "Bandwidth of the presence band."
    ),
    p!(
        "high_freq_hz",
        "High shelf frequency",
        "Hz",
        2000.0,
        20000.0,
        10000.0,
        "Corner of the high shelf."
    ),
    p!(
        "high_gain_db",
        "High shelf gain",
        "dB",
        -24.0,
        24.0,
        0.0,
        "Boost or cut above the high shelf corner."
    ),
];

const COMPRESSOR: &[EffectParam] = &[
    p!(
        "threshold_db",
        "Threshold",
        "dBFS",
        -60.0,
        0.0,
        -18.0,
        "Level above which gain is reduced."
    ),
    p!(
        "ratio",
        "Ratio",
        "ratio",
        1.0,
        20.0,
        3.0,
        "Input dB above threshold per output dB."
    ),
    p!(
        "knee_db",
        "Knee",
        "dB",
        0.0,
        24.0,
        6.0,
        "Width of the soft knee around the threshold."
    ),
    p!(
        "attack_ms",
        "Attack",
        "ms",
        0.1,
        200.0,
        10.0,
        "Time to react to a rise in level."
    ),
    p!(
        "release_ms",
        "Release",
        "ms",
        5.0,
        2000.0,
        120.0,
        "Time to recover after the level falls."
    ),
    p!(
        "makeup_db",
        "Makeup gain",
        "dB",
        -12.0,
        24.0,
        0.0,
        "Gain applied after compression."
    ),
];

const DE_ESSER: &[EffectParam] = &[
    p!(
        "freq_hz",
        "Frequency",
        "Hz",
        2000.0,
        12000.0,
        6000.0,
        "Lower edge of the sibilance band."
    ),
    p!(
        "threshold_db",
        "Threshold",
        "dBFS",
        -60.0,
        0.0,
        -30.0,
        "Sibilance-band level above which it is reduced."
    ),
    p!(
        "range_db",
        "Range",
        "dB",
        0.0,
        24.0,
        8.0,
        "Largest reduction of the sibilance band."
    ),
];

const REVERB: &[EffectParam] = &[
    p!(
        "room_size",
        "Room size",
        "ratio",
        0.0,
        1.0,
        0.5,
        "Larger rooms decay more slowly."
    ),
    p!(
        "damping",
        "Damping",
        "ratio",
        0.0,
        1.0,
        0.5,
        "High-frequency absorption of the walls."
    ),
    p!(
        "pre_delay_ms",
        "Pre-delay",
        "ms",
        0.0,
        200.0,
        20.0,
        "Gap between the dry sound and the first reflections."
    ),
    p!(
        "width",
        "Width",
        "ratio",
        0.0,
        1.0,
        1.0,
        "Stereo width of the reverberation."
    ),
    p!(
        "mix",
        "Mix",
        "ratio",
        0.0,
        1.0,
        0.2,
        "Wet share of the output; 0 is dry."
    ),
];

const REGISTRY: &[EffectDescriptor] = &[
    EffectDescriptor {
        kind: EffectKind::Eq,
        name: "EQ",
        description: "High-pass, low shelf, two peaking bands and a high shelf.",
        params: EQ,
    },
    EffectDescriptor {
        kind: EffectKind::Compressor,
        name: "Compressor",
        description: "Stereo-linked feed-forward compressor with a soft knee.",
        params: COMPRESSOR,
    },
    EffectDescriptor {
        kind: EffectKind::DeEsser,
        name: "De-esser",
        description: "Reduces the sibilance band when it exceeds the threshold.",
        params: DE_ESSER,
    },
    EffectDescriptor {
        kind: EffectKind::Reverb,
        name: "Reverb",
        description: "Room simulation with parallel combs and series all-passes.",
        params: REVERB,
    },
];

/// Every built-in effect.
pub fn registry() -> &'static [EffectDescriptor] {
    REGISTRY
}

/// The descriptor of an effect.
pub fn descriptor(kind: EffectKind) -> &'static EffectDescriptor {
    REGISTRY
        .iter()
        .find(|d| d.kind == kind)
        .expect("every effect kind is registered")
}

impl EffectSlot {
    /// A slot with every parameter at its default.
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            enabled: true,
            params: Default::default(),
        }
    }

    /// A parameter's value, or its default when the slot does not set it.
    /// Unknown ids return `None`.
    pub fn value(&self, id: &str) -> Option<f64> {
        let p = descriptor(self.kind).param(id)?;
        Some(self.params.get(id).copied().unwrap_or(p.default))
    }

    /// Everything wrong with the slot's parameters.
    pub fn issues(&self) -> Vec<String> {
        let d = descriptor(self.kind);
        let mut out = Vec::new();
        for (id, &v) in &self.params {
            match d.param(id) {
                None => out.push(format!(
                    "{} has no parameter `{id}` (known: {})",
                    d.name,
                    d.params.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")
                )),
                Some(p) if !v.is_finite() || v < p.min || v > p.max => out.push(format!(
                    "{} `{id}` = {v} is outside {}..={} {}",
                    d.name, p.min, p.max, p.unit
                )),
                Some(_) => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_lie_in_range_and_ids_are_unique() {
        for d in registry() {
            let mut ids: Vec<_> = d.params.iter().map(|p| p.id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), d.params.len(), "{}", d.name);
            for p in d.params {
                assert!(p.min <= p.default && p.default <= p.max, "{}", p.id);
            }
        }
    }

    #[test]
    fn slots_report_unknown_and_out_of_range_params() {
        let mut s = EffectSlot::new(EffectKind::Reverb);
        assert!(s.issues().is_empty());
        assert_eq!(s.value("mix"), Some(0.2));
        s.params.insert("mix".into(), 2.0);
        s.params.insert("wetness".into(), 0.5);
        let issues = s.issues();
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues.iter().any(|i| i.contains("wetness")));
        assert!(issues.iter().any(|i| i.contains("outside")));
    }
}
