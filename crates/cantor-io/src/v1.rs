//! Converts Cantor v1 projects (`{"format": "cantor-project", "version":
//! 1|2, "project": ...}`, 1920 ticks per quarter, notes in parts) to the
//! current model. Everything that cannot be carried over exactly is named
//! in a diagnostic.
//!
//! Mapping:
//! - ticks ÷ 4 (PPQ 1920 → 480); part offsets are added to note starts;
//! - `pitch_cents` → `key` (+ a constant bend for any fraction);
//! - portamento `before` (fraction before the note) → `start_ms`;
//! - vibrato `start` (fraction of the note) → `length` (% from the end),
//!   `rate_hz` → `period_ms`, fades and drift kept;
//! - curves (`user_delta`, plus `model_prediction` if any): `pitch` →
//!   `pitd`, `energy` → `dyn` (both unchanged units); `gender` semitones
//!   (+ = smaller) → `gen` % (+ = deeper, ±100 ≈ ±3 st); `breathiness` dB
//!   → `bre` ≈ 10 %/dB above neutral;
//! - effects, mix, tempo, time signatures, the first key and sections as-is.

use cantor_core::{Diagnostic, Project};
use serde_json::{Map, Value, json};

const TICK_DIV: f64 = 4.0;

fn ticks(v: &Value) -> i64 {
    (v.as_f64().unwrap_or(0.0) / TICK_DIV).round() as i64
}

fn f(v: &Value, k: &str, d: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(d)
}

/// Converts a v1 project envelope; `Err` when it isn't one this can read.
pub fn convert(envelope: &Value) -> Result<(Project, Vec<Diagnostic>), String> {
    let p = envelope
        .get("project")
        .ok_or("no `project` in the v1 envelope")?;
    let mut diags = vec![Diagnostic::new(
        cantor_core::Level::Info,
        "v1.converted",
        "Converted from a Cantor v1 project; save it under a new name to keep the original",
    )];
    let mut warn = |code: &str, msg: String| diags.push(Diagnostic::warning(code, msg));

    let tempo: Vec<Value> = p["tempo_map"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| json!({ "tick": ticks(&t["tick"]), "bpm": f(t, "bpm", 120.0) }))
        .collect();
    let time_sig: Vec<Value> = p["time_sig_map"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| json!({ "bar": t["bar"], "num": t["numerator"], "den": t["denominator"] }))
        .collect();
    let keys = p["key_map"].as_array().cloned().unwrap_or_default();
    if keys.len() > 1 {
        warn(
            "v1.key_changes",
            format!("{} key changes; only the first key is kept", keys.len() - 1),
        );
    }
    let key = keys
        .first()
        .map(|k| json!({ "tonic": k["tonic"], "scale": k["scale"] }));
    let sections: Vec<Value> = p["sections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| json!({ "name": s["name"], "start": ticks(&s["start"]), "end": ticks(&s["end"]) }))
        .collect();
    for k in ["takes", "seeds"] {
        if p[k].as_object().is_some_and(|o| !o.is_empty()) {
            warn(
                "v1.dropped",
                format!("`{k}` (alternative takes) are not part of this version and were dropped"),
            );
        }
    }

    let mut tracks = Vec::new();
    let (mut clamped, mut bent, mut envelopes, mut phonemes) = (0usize, 0usize, 0usize, 0usize);
    for t in p["tracks"].as_array().cloned().unwrap_or_default() {
        let name = t["name"].as_str().unwrap_or("Track").to_owned();
        match t["backend"].as_str() {
            Some("utau") | None => {}
            Some(b) => warn(
                "v1.backend",
                format!("Track `{name}` used the `{b}` backend; it now sings with the UTAU synth"),
            ),
        }
        if t["doubling"]["layers"].as_u64().unwrap_or(1) > 1 {
            warn(
                "v1.doubling",
                format!(
                    "Track `{name}`: doubling layers were dropped (duplicate the track instead)"
                ),
            );
        }
        if t["voice_mix"].as_array().is_some_and(|a| !a.is_empty()) {
            warn(
                "v1.voice_mix",
                format!("Track `{name}`: the voice mix was dropped"),
            );
        }
        let mut notes = Vec::new();
        for part in t["parts"].as_array().cloned().unwrap_or_default() {
            let base = part["start"].as_f64().unwrap_or(0.0);
            for n in part["notes"].as_array().cloned().unwrap_or_default() {
                let cents = f(&n, "pitch_cents", 6000.0);
                let key = (cents / 100.0).round();
                let e = &n["expr"];
                let po = &e["portamento"];
                let len = po["length_ms"].as_f64().unwrap_or(80.0);
                let mut pitch = json!({ "portamento": {
                    "length_ms": len, "start_ms": -len * f(po, "before", 0.6),
                    "overshoot_cents": f(po, "overshoot_cents", 0.0), "preparation_cents": f(po, "preparation_cents", 0.0), "shape": "s" } });
                if let Some(v) = e.get("vibrato").filter(|v| v.is_object()) {
                    pitch["vibrato"] = json!({
                        "length": ((1.0 - f(v, "start", 0.3)) * 100.0).clamp(0.0, 100.0),
                        "period_ms": 1000.0 / f(v, "rate_hz", 5.5).max(0.5),
                        "depth_cents": f(v, "depth_cents", 20.0),
                        "fade_in": (f(v, "fade_in", 0.3) * 100.0).clamp(0.0, 100.0),
                        "fade_out": (f(v, "fade_out", 0.2) * 100.0).clamp(0.0, 100.0),
                        "phase": f(v, "phase", 0.0), "shift": 0.0, "drift": f(v, "drift", 0.0).clamp(0.0, 1.0) });
                }
                if (cents - key * 100.0).abs() > 0.01 {
                    pitch["points"] = json!([{ "ms": 0.0, "cents": cents - key * 100.0 }]);
                    bent += 1;
                }
                if e.get("attack").is_some_and(|a| !a.is_null())
                    || e.get("release").is_some_and(|a| !a.is_null())
                {
                    envelopes += 1;
                }
                if n.get("phonemes").is_some_and(|a| !a.is_null())
                    || n.get("lang").is_some_and(|a| !a.is_null())
                {
                    phonemes += 1;
                }
                notes.push(json!({
                    "id": n["id"], "start": ((base + n["start"].as_f64().unwrap_or(0.0)) / TICK_DIV).round() as i64,
                    "length": (n["length"].as_f64().unwrap_or(1920.0) / TICK_DIV).round().max(1.0) as i64,
                    "key": key.clamp(0.0, 127.0) as u8, "lyric": n["lyric"], "pitch": pitch }));
            }
        }
        // Rounding to 480 PPQ can make neighbours touch; trim the earlier note.
        notes.sort_by_key(|n| n["start"].as_i64());
        for i in 1..notes.len() {
            let next = notes[i]["start"].as_i64().unwrap_or(0);
            let (s, l) = (
                notes[i - 1]["start"].as_i64().unwrap_or(0),
                notes[i - 1]["length"].as_i64().unwrap_or(1),
            );
            if s + l > next {
                notes[i - 1]["length"] = json!((next - s).max(1));
            }
        }
        let mut curves = Map::new();
        if let Some(cs) = t["curves"].as_object() {
            for (id, c) in cs {
                let (to, conv): (&str, fn(f64) -> f64) = match id.as_str() {
                    "pitch" => ("pitd", |v| v),
                    "energy" => ("dyn", |v| v),
                    "gender" => ("gen", |v| -v * 100.0 / 3.0),
                    "breathiness" => ("bre", |v| v * 10.0),
                    other => {
                        warn(
                            "v1.curve_dropped",
                            format!(
                                "Track `{name}`: the `{other}` curve has no equivalent and was dropped"
                            ),
                        );
                        continue;
                    }
                };
                let range = match to {
                    "pitd" => (-1200.0, 1200.0),
                    "dyn" => (-24.0, 12.0),
                    "bre" => (0.0, 100.0),
                    _ => (-100.0, 100.0),
                };
                let mut pts: Vec<(i64, f64)> = Vec::new();
                for layer in ["model_prediction", "user_delta"] {
                    for pt in c[layer]["points"].as_array().cloned().unwrap_or_default() {
                        let (tk, v) = (ticks(&pt[0]), conv(pt[1].as_f64().unwrap_or(0.0)));
                        match pts.iter_mut().find(|(x, _)| *x == tk) {
                            Some(e) => e.1 += v,
                            None => pts.push((tk, v)),
                        }
                    }
                }
                pts.sort_by_key(|p| p.0);
                for p in &mut pts {
                    let v = p.1.clamp(range.0, range.1);
                    clamped += usize::from(v != p.1);
                    p.1 = v;
                }
                if pts.is_empty() {
                    continue;
                }
                let interp = match c["user_delta"]["interp"].as_str() {
                    Some("step") => "step",
                    Some("linear") => "linear",
                    _ => "smooth",
                };
                curves.insert(to.to_owned(), json!({ "points": pts.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>(), "interp": interp }));
            }
        }
        tracks.push(json!({
            "id": t["id"], "name": name, "singer": t["singer"]["id"].as_str().unwrap_or(""), "phonemizer": "auto",
            "mix": t["mix"], "effects": t["effects"], "curves": curves, "notes": notes }));
    }
    if bent > 0 {
        warn(
            "v1.microtonal",
            format!("{bent} notes were off the semitone grid; the difference is a pitch bend"),
        );
    }
    if clamped > 0 {
        warn(
            "v1.curve_clamped",
            format!("{clamped} curve points were outside the current ranges and were clamped"),
        );
    }
    if envelopes > 0 {
        warn(
            "v1.envelopes",
            format!(
                "{envelopes} notes had attack/release envelopes, which were dropped (use `dyn`)"
            ),
        );
    }
    if phonemes > 0 {
        warn(
            "v1.phonemes",
            format!(
                "{phonemes} notes had phoneme or language overrides, which were dropped (set `alias` instead)"
            ),
        );
    }
    let project = json!({
        "format": cantor_core::FORMAT, "meta": p["meta"], "tempo": tempo, "time_sig": time_sig, "key": key,
        "sections": sections, "tracks": tracks });
    let project: Project = serde_json::from_value(project)
        .map_err(|e| format!("v1 project could not be converted: {e}"))?;
    Ok((project, diags))
}
