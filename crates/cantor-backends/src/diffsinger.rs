//! DiffSinger backend: OpenUtau-format voicebanks (duration, pitch and
//! variance models plus an acoustic model) with a PC-NSF-HiFiGAN vocoder.
//!
//! The model contract follows OpenUtau's DiffSinger renderer and OpenVPI's
//! deployment exporters; see docs/decisions/0011-diffsinger-backend.md.

use crate::onnx::{Model, scalar_f32, scalar_i64, tensor_bool, tensor_f32, tensor_i64};
use cantor_core::{Diagnostic, EntityRef, PhonemeAlphabet};
use cantor_phon::place::{GroupPhone, fit_group, to_frames};
use cantor_phon::{Dictionary, Lyric, PhoneKind, parse_lyric};
use cantor_render::backend::{
    AcousticInput, Backend, BackendError, Grid, Phone, PhraseInput, Prediction,
};
use cantor_render::phrase::PhraseNote;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Noise samples per frame reserved for each diffusion model. The graphs
/// take the first `size(like)` of them.
const ACOUSTIC_NOISE_PER_FRAME: usize = 1024;
const VARIANCE_NOISE_PER_FRAME: usize = 256;

/// Seed domains, so each model draws independent noise from a phrase seed.
const SEED_PITCH: u64 = 0x7069_7463_6800_0001;
const SEED_VARIANCE: u64 = 0x7661_7269_0000_0002;
const SEED_ACOUSTIC: u64 = 0x6163_6f75_0000_0003;

/// Predicted consonant length when no duration model is present.
const FALLBACK_CONSONANT_SECS: f64 = 0.08;

#[derive(Debug, Deserialize)]
struct AcousticConfig {
    phonemes: String,
    #[serde(default)]
    languages: Option<String>,
    #[serde(default)]
    use_lang_id: bool,
    acoustic: String,
    vocoder: String,
    #[serde(default)]
    augmentation_args: Option<Augmentation>,
    #[serde(default)]
    use_key_shift_embed: bool,
    #[serde(default)]
    use_speed_embed: bool,
    #[serde(default)]
    use_energy_embed: bool,
    #[serde(default)]
    use_breathiness_embed: bool,
    #[serde(default)]
    use_voicing_embed: bool,
    #[serde(default)]
    use_tension_embed: bool,
    #[serde(default)]
    use_continuous_acceleration: bool,
    #[serde(default = "one")]
    max_depth: f32,
    sample_rate: u32,
    hop_size: usize,
    num_mel_bins: usize,
    #[serde(default)]
    mel_base: Option<String>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
struct Augmentation {
    random_pitch_shifting: Option<PitchShifting>,
}

#[derive(Debug, Deserialize)]
struct PitchShifting {
    range: [f32; 2],
}

#[derive(Debug, Deserialize)]
struct VarianceConfig {
    phonemes: String,
    #[serde(default)]
    languages: Option<String>,
    #[serde(default)]
    use_lang_id: bool,
    linguistic: String,
    #[serde(default)]
    dur: Option<String>,
    #[serde(default)]
    pitch: Option<String>,
    #[serde(default)]
    variance: Option<String>,
    #[serde(default)]
    predict_dur: bool,
    #[serde(default)]
    predict_energy: bool,
    #[serde(default)]
    predict_breathiness: bool,
    #[serde(default)]
    predict_voicing: bool,
    #[serde(default)]
    predict_tension: bool,
    #[serde(default)]
    use_expr: bool,
}

#[derive(Debug, Deserialize)]
struct VocoderConfig {
    #[serde(default)]
    name: Option<String>,
    model: String,
    sample_rate: u32,
    hop_size: usize,
    num_mel_bins: usize,
    #[serde(default)]
    mel_base: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CharacterConfig {
    #[serde(default)]
    default_phonemizer: Option<String>,
}

/// Phoneme and language ids for one model.
#[derive(Debug)]
struct Vocab {
    phonemes: HashMap<String, i64>,
    languages: HashMap<String, i64>,
    use_lang_id: bool,
}

impl Vocab {
    fn load(
        dir: &Path,
        phonemes: &str,
        languages: Option<&str>,
        use_lang_id: bool,
    ) -> Result<Self, BackendError> {
        let read_map = |file: &str| -> Result<HashMap<String, i64>, BackendError> {
            let text = read_text(&dir.join(file))?;
            serde_json::from_str(&text).map_err(|e| BackendError(format!("{file}: {e}")))
        };
        Ok(Self {
            phonemes: read_map(phonemes)?,
            languages: match languages {
                Some(l) => read_map(l)?,
                None => HashMap::new(),
            },
            use_lang_id,
        })
    }

    fn token(&self, symbol: &str) -> Option<i64> {
        self.phonemes.get(symbol).copied()
    }

    fn language(&self, symbol: &str) -> i64 {
        if !self.use_lang_id {
            return 0;
        }
        symbol
            .split_once('/')
            .and_then(|(lang, _)| self.languages.get(lang).copied())
            .unwrap_or(0)
    }

    /// (tokens, languages) for a phone sequence.
    fn encode(&self, phones: &[Phone]) -> Result<(Vec<i64>, Vec<i64>), BackendError> {
        let mut tokens = Vec::with_capacity(phones.len());
        for p in phones {
            tokens.push(self.token(&p.symbol).ok_or_else(|| {
                BackendError(format!(
                    "phoneme `{}` is not in the model's vocabulary",
                    p.symbol
                ))
            })?);
        }
        Ok((
            tokens,
            phones.iter().map(|p| self.language(&p.symbol)).collect(),
        ))
    }
}

/// A linguistic encoder plus a predictor.
#[derive(Debug)]
struct Stage {
    vocab: Vocab,
    linguistic: Model,
    predictor: Model,
}

impl Stage {
    fn load(dir: &Path, cfg: &VarianceConfig, predictor: &str) -> Result<Self, BackendError> {
        Ok(Self {
            vocab: Vocab::load(
                dir,
                &cfg.phonemes,
                cfg.languages.as_deref(),
                cfg.use_lang_id,
            )?,
            linguistic: Model::load(&dir.join(&cfg.linguistic))?,
            predictor: Model::load(&dir.join(predictor))?,
        })
    }
}

#[derive(Debug)]
struct VarianceStage {
    stage: Stage,
    /// Registry ids of predicted parameters, in model output order.
    params: Vec<&'static str>,
}

/// A loaded DiffSinger voicebank with its vocoder.
#[derive(Debug)]
pub struct DiffSinger {
    root: PathBuf,
    id: String,
    version: String,
    grid: Grid,
    default_lang: String,
    acoustic_vocab: Vocab,
    acoustic: Model,
    acoustic_cfg: AcousticConfig,
    vocoder: Model,
    dur: Option<Stage>,
    pitch: Option<(Stage, bool)>,
    variance: Option<VarianceStage>,
    dict_dir: PathBuf,
    dicts: Mutex<HashMap<String, &'static Dictionary>>,
    params: Vec<&'static str>,
}

fn read_text(path: &Path) -> Result<String, BackendError> {
    std::fs::read_to_string(path).map_err(|e| BackendError(format!("{}: {e}", path.display())))
}

fn yaml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BackendError> {
    let text = read_text(path)?;
    // Some voicebank files start with a byte-order mark.
    serde_saphyr::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| BackendError(format!("{}: {e}", path.display())))
}

impl DiffSinger {
    /// Loads a voicebank directory and a vocoder directory (an unpacked
    /// OpenUtau `.oudep` with `vocoder.yaml`).
    pub fn load(voicebank: &Path, vocoder_dir: &Path) -> Result<Self, BackendError> {
        let acoustic_cfg: AcousticConfig = yaml(&voicebank.join("dsconfig.yaml"))?;
        let voc_cfg: VocoderConfig = yaml(&vocoder_dir.join("vocoder.yaml"))?;
        if voc_cfg.sample_rate != acoustic_cfg.sample_rate
            || voc_cfg.hop_size != acoustic_cfg.hop_size
            || voc_cfg.num_mel_bins != acoustic_cfg.num_mel_bins
        {
            return Err(BackendError(format!(
                "vocoder {} ({} Hz, hop {}, {} mels) does not match the voicebank ({} Hz, hop {}, {} mels)",
                voc_cfg.model,
                voc_cfg.sample_rate,
                voc_cfg.hop_size,
                voc_cfg.num_mel_bins,
                acoustic_cfg.sample_rate,
                acoustic_cfg.hop_size,
                acoustic_cfg.num_mel_bins
            )));
        }
        if voc_cfg
            .name
            .as_deref()
            .is_some_and(|n| n != acoustic_cfg.vocoder)
        {
            tracing::warn!(
                "voicebank expects vocoder {}, using {}",
                acoustic_cfg.vocoder,
                voc_cfg.name.as_deref().unwrap_or("?")
            );
        }
        let base = |m: &Option<String>| m.clone().unwrap_or_else(|| "10".into());
        if base(&acoustic_cfg.mel_base) != base(&voc_cfg.mel_base) {
            return Err(BackendError(
                "vocoder and voicebank use different mel log bases".into(),
            ));
        }
        if !acoustic_cfg.use_continuous_acceleration {
            return Err(BackendError(
                "voicebanks without continuous acceleration (rectified flow) are not supported yet"
                    .into(),
            ));
        }
        let character: CharacterConfig =
            yaml(&voicebank.join("character.yaml")).unwrap_or(CharacterConfig {
                default_phonemizer: None,
            });
        let default_lang = match character.default_phonemizer.as_deref().unwrap_or("") {
            p if p.contains("Japanese") => "ja",
            p if p.contains("English") => "en",
            p if p.contains("Korean") => "ko",
            _ => "zh",
        }
        .to_owned();

        let acoustic_vocab = Vocab::load(
            voicebank,
            &acoustic_cfg.phonemes,
            acoustic_cfg.languages.as_deref(),
            acoustic_cfg.use_lang_id,
        )?;
        let acoustic = Model::load(&voicebank.join(&acoustic_cfg.acoustic))?;
        let vocoder = Model::load(&vocoder_dir.join(&voc_cfg.model))?;

        let sub = |name: &str| -> Result<Option<(PathBuf, VarianceConfig)>, BackendError> {
            let dir = voicebank.join(name);
            let cfg = dir.join("dsconfig.yaml");
            if !cfg.exists() {
                return Ok(None);
            }
            Ok(Some((dir.clone(), yaml(&cfg)?)))
        };
        let dur = match sub("dsdur")? {
            Some((dir, cfg)) if cfg.predict_dur => {
                let dur = cfg
                    .dur
                    .clone()
                    .ok_or_else(|| BackendError("dsdur: no `dur` model".into()))?;
                Some(Stage::load(&dir, &cfg, &dur)?)
            }
            _ => None,
        };
        let pitch = match sub("dspitch")? {
            Some((dir, cfg)) => match cfg.pitch.clone() {
                Some(p) => Some((Stage::load(&dir, &cfg, &p)?, cfg.use_expr)),
                None => None,
            },
            None => None,
        };
        let variance = match sub("dsvariance")? {
            Some((dir, cfg)) => match cfg.variance.clone() {
                Some(v) => {
                    let mut params = Vec::new();
                    // Output order follows OpenVPI's exporter.
                    for (on, p) in [
                        (cfg.predict_energy, "energy"),
                        (cfg.predict_breathiness, "breathiness"),
                        (cfg.predict_voicing, "voicing"),
                        (cfg.predict_tension, "tension"),
                    ] {
                        if on {
                            params.push(p);
                        }
                    }
                    Some(VarianceStage {
                        stage: Stage::load(&dir, &cfg, &v)?,
                        params,
                    })
                }
                None => None,
            },
            None => None,
        };

        let mut params = vec!["pitch"];
        for (on, p) in [
            (acoustic_cfg.use_energy_embed, "energy"),
            (acoustic_cfg.use_breathiness_embed, "breathiness"),
            (acoustic_cfg.use_voicing_embed, "voicing"),
            (acoustic_cfg.use_tension_embed, "tension"),
            (acoustic_cfg.use_key_shift_embed, "gender"),
            (acoustic_cfg.use_speed_embed, "velocity"),
        ] {
            if on {
                params.push(p);
            }
        }

        let id = voicebank
            .file_name()
            .map_or_else(|| "diffsinger".into(), |n| n.to_string_lossy().into_owned());
        let version = format!(
            "{}+{}+{}",
            acoustic_cfg.acoustic,
            voc_cfg.model,
            env!("CARGO_PKG_VERSION")
        );
        let dict_dir = if voicebank.join("dsdur").exists() {
            voicebank.join("dsdur")
        } else {
            voicebank.to_path_buf()
        };
        Ok(Self {
            root: voicebank.to_path_buf(),
            id: format!("diffsinger/{id}"),
            version,
            grid: Grid {
                sample_rate: acoustic_cfg.sample_rate,
                hop: acoustic_cfg.hop_size,
                mel_bins: acoustic_cfg.num_mel_bins,
            },
            default_lang,
            acoustic_vocab,
            acoustic,
            acoustic_cfg,
            vocoder,
            dur,
            pitch,
            variance,
            dict_dir,
            dicts: Mutex::new(HashMap::new()),
            params,
        })
    }

    /// The voicebank directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The dictionary for a language: `dsdict-<lang>.yaml`, else `dsdict.yaml`.
    /// Dictionaries are large and live for the process, so they are leaked
    /// once per language.
    fn dict(&self, lang: &str) -> Result<&'static Dictionary, BackendError> {
        let mut dicts = self.dicts.lock().expect("dictionary lock");
        if let Some(d) = dicts.get(lang) {
            return Ok(d);
        }
        let specific = self.dict_dir.join(format!("dsdict-{lang}.yaml"));
        let path = if specific.exists() {
            specific
        } else {
            self.dict_dir.join("dsdict.yaml")
        };
        let d = Dictionary::from_yaml(&read_text(&path)?)
            .map_err(|e| BackendError(format!("{}: {e}", path.display())))?;
        let d: &'static Dictionary = Box::leak(Box::new(d));
        dicts.insert(lang.to_owned(), d);
        Ok(d)
    }

    fn frame_secs(&self) -> f64 {
        self.grid.frame_secs()
    }
}

/// A syllable: one sung note (with any `+` continuation notes merged in).
#[derive(Debug)]
struct Syllable {
    start: f64,
    end: f64,
    cents: f64,
    lead: Vec<(String, Option<f64>)>,
    nucleus: Vec<(String, Option<f64>)>,
}

impl DiffSinger {
    fn syllables(
        &self,
        notes: &[PhraseNote],
        diags: &mut Vec<Diagnostic>,
    ) -> Result<Vec<Syllable>, BackendError> {
        let mut out: Vec<Syllable> = Vec::new();
        for n in notes {
            let lang = n
                .lang
                .as_ref()
                .map_or(self.default_lang.as_str(), |l| l.0.as_str());
            let target = Some(EntityRef::Note { id: n.id });
            let phones: Vec<(String, Option<f64>)> = match (&n.phonemes, parse_lyric(&n.lyric)) {
                (_, Lyric::Continue) if !out.is_empty() => {
                    let last = out.last_mut().expect("non-empty");
                    last.end = last.end.max(n.end_sec);
                    continue;
                }
                (Some(ovr), _) if ovr.iter().all(|p| p.alphabet == PhonemeAlphabet::Native) => ovr
                    .iter()
                    .map(|p| {
                        let sym = if self.acoustic_vocab.token(&p.symbol).is_some() {
                            p.symbol.clone()
                        } else {
                            format!("{lang}/{}", p.symbol)
                        };
                        (sym, p.length_ms.map(|ms| ms / 1000.0))
                    })
                    .collect(),
                (ovr, lyric) => {
                    if ovr.is_some() {
                        diags.push(Diagnostic::warning(
                            "phon.xsampa_unsupported",
                            "X-SAMPA phoneme overrides arrive in M5; the lyric is used instead",
                            target.clone(),
                        ));
                    }
                    match lyric {
                        Lyric::Rest | Lyric::Continue => vec![("SP".into(), None)],
                        Lyric::Breath => vec![("AP".into(), None)],
                        Lyric::Syllable(g) => {
                            match self.dict(lang)?.lookup(&g) {
                                Some(ph) => ph.iter().map(|p| (p.clone(), None)).collect(),
                                None => {
                                    diags.push(Diagnostic::warning(
                                    "phon.unknown_lyric",
                                    format!("`{}` is not in the {lang} dictionary; the note is silent", n.lyric),
                                    target.clone(),
                                ));
                                    vec![("SP".into(), None)]
                                }
                            }
                        }
                    }
                }
            };
            for (p, _) in &phones {
                if self.acoustic_vocab.token(p).is_none() {
                    return Err(BackendError(format!(
                        "phoneme `{p}` (note {}) is not in the voicebank's vocabulary",
                        n.id
                    )));
                }
            }
            let dict = self.dict(lang)?;
            let first_vowel = phones
                .iter()
                .position(|(p, _)| dict.kind(p) == PhoneKind::Vowel || p == "SP" || p == "AP")
                .unwrap_or(0);
            let (lead, nucleus) = phones.split_at(first_vowel);
            out.push(Syllable {
                start: n.start_sec,
                end: n.end_sec,
                cents: n.pitch_cents,
                lead: lead.to_vec(),
                nucleus: nucleus.to_vec(),
            });
        }
        Ok(out)
    }
}

/// A duration group: phones from one onset to the next.
struct Group {
    start: f64,
    end: f64,
    midi: i64,
    phones: Vec<(String, GroupPhone)>,
}

fn groups(syl: &[Syllable], t0: f64, t_end: f64, consonant_secs: f64) -> Vec<Group> {
    let mut gs = Vec::new();
    let lead = |s: &Syllable| -> Vec<(String, GroupPhone)> {
        s.lead
            .iter()
            .map(|(p, pin)| {
                (
                    p.clone(),
                    GroupPhone {
                        predicted: consonant_secs,
                        pinned: *pin,
                        leading: true,
                    },
                )
            })
            .collect()
    };
    let mut head = vec![(
        "SP".to_owned(),
        GroupPhone {
            predicted: 1.0,
            pinned: None,
            leading: false,
        },
    )];
    head.extend(lead(&syl[0]));
    gs.push(Group {
        start: t0,
        end: syl[0].start,
        midi: (syl[0].cents / 100.0).round() as i64,
        phones: head,
    });
    for (i, s) in syl.iter().enumerate() {
        let next = syl.get(i + 1);
        let end = next.map_or(s.end, |n| n.start);
        let mut phones: Vec<(String, GroupPhone)> = s
            .nucleus
            .iter()
            .map(|(p, pin)| {
                (
                    p.clone(),
                    GroupPhone {
                        predicted: s.end - s.start,
                        pinned: *pin,
                        leading: false,
                    },
                )
            })
            .collect();
        if let Some(n) = next {
            let lead_pred: f64 = n.lead.len() as f64 * consonant_secs;
            let gap = n.start - s.end;
            if gap > 1e-6 {
                phones.push((
                    "SP".into(),
                    GroupPhone {
                        predicted: gap,
                        pinned: Some((gap - lead_pred).max(0.0)),
                        leading: false,
                    },
                ));
            }
            phones.extend(lead(n));
        }
        gs.push(Group {
            start: s.start,
            end,
            midi: (s.cents / 100.0).round() as i64,
            phones,
        });
    }
    let last = syl.last().expect("phrases have notes");
    gs.push(Group {
        start: last.end,
        end: t_end,
        midi: (last.cents / 100.0).round() as i64,
        phones: vec![(
            "SP".into(),
            GroupPhone {
                predicted: t_end - last.end,
                pinned: None,
                leading: false,
            },
        )],
    });
    gs
}

impl Backend for DiffSinger {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn grid(&self) -> Grid {
        self.grid
    }

    fn params(&self) -> &[&'static str] {
        &self.params
    }

    fn param_default(&self, param: &str) -> f32 {
        match param {
            "breathiness" | "voicing" | "energy" => -18.0,
            "velocity" => 1.0,
            _ => 0.0,
        }
    }

    fn param_range(&self, param: &str) -> (f32, f32) {
        match param {
            "breathiness" | "voicing" | "energy" => (-96.0, 0.0),
            "tension" => (-10.0, 10.0),
            "velocity" => (0.25, 4.0),
            "gender" => {
                let r = self
                    .acoustic_cfg
                    .augmentation_args
                    .as_ref()
                    .and_then(|a| a.random_pitch_shifting.as_ref())
                    .map_or([-5.0, 5.0], |p| p.range);
                (r[0], r[1])
            }
            _ => (f32::MIN, f32::MAX),
        }
    }

    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        let fs = self.frame_secs();
        let frames = input.frames;
        let t_end = input.t0 + frames as f64 * fs;
        let mut diagnostics = Vec::new();
        let syl = self.syllables(&input.phrase.notes, &mut diagnostics)?;
        let mut gs = groups(&syl, input.t0, t_end, FALLBACK_CONSONANT_SECS);

        // Duration model: predicted lengths per phone, then fit each group.
        let flat: Vec<Phone> = gs
            .iter()
            .flat_map(|g| {
                g.phones.iter().map(|(s, _)| Phone {
                    symbol: s.clone(),
                    frames: 0,
                })
            })
            .collect();
        if let Some(dur) = &self.dur {
            let (tokens, langs) = dur.vocab.encode(&flat)?;
            let t = tokens.len();
            let word_div: Vec<i64> = gs.iter().map(|g| g.phones.len() as i64).collect();
            let bounds: Vec<f64> = gs.iter().map(|g| g.end - g.start).collect();
            let word_dur = to_frames(&bounds, fs);
            let ph_midi: Vec<i64> = gs
                .iter()
                .flat_map(|g| g.phones.iter().map(|_| g.midi))
                .collect();
            let w = word_div.len();
            let enc = dur.linguistic.run(
                vec![
                    ("tokens", tensor_i64(&[1, t], tokens)?),
                    ("languages", tensor_i64(&[1, t], langs)?),
                    ("word_div", tensor_i64(&[1, w], word_div)?),
                    ("word_dur", tensor_i64(&[1, w], word_dur)?),
                ],
                0,
                0,
                &["encoder_out"],
            )?;
            let masks = vec![true; t];
            let hidden = enc[0].len() / t;
            let pred = dur.predictor.run(
                vec![
                    ("encoder_out", tensor_f32(&[1, t, hidden], enc[0].clone())?),
                    ("x_masks", tensor_bool(&[1, t], masks)?),
                    ("ph_midi", tensor_i64(&[1, t], ph_midi)?),
                ],
                0,
                0,
                &["ph_dur_pred"],
            )?;
            let mut k = 0;
            for g in &mut gs {
                for (_, p) in &mut g.phones {
                    p.predicted = f64::from(pred[0][k]) * fs;
                    k += 1;
                }
            }
        }
        let mut lengths = Vec::with_capacity(flat.len());
        for g in &gs {
            let gp: Vec<GroupPhone> = g.phones.iter().map(|(_, p)| p.clone()).collect();
            let fitted = fit_group(&gp, g.end - g.start);
            if let Some(msg) = fitted.collision {
                diagnostics.push(Diagnostic::warning("phon.collision", msg, None));
            }
            lengths.extend(fitted.lengths);
        }
        let ph_frames = to_frames(&lengths, fs);
        let phones: Vec<Phone> = flat
            .into_iter()
            .zip(&ph_frames)
            .map(|(p, &f)| Phone {
                symbol: p.symbol,
                frames: f,
            })
            .collect();
        let total: i64 = ph_frames.iter().sum();
        if total != frames as i64 {
            return Err(BackendError(format!(
                "internal: phonemes cover {total} of {frames} frames"
            )));
        }

        // Pitch model.
        let base_midi: Vec<f32> = input
            .base_pitch
            .iter()
            .map(|c| (c / 100.0) as f32)
            .collect();
        let pitch = match &self.pitch {
            Some((stage, use_expr)) => {
                let enc = self.encode(stage, &phones)?;
                let (note_midi, note_rest, note_dur) =
                    pitch_notes(&input.phrase.notes, input.t0, t_end, fs);
                let n = note_midi.len();
                let mut inputs = vec![
                    ("encoder_out", enc),
                    ("ph_dur", tensor_i64(&[1, phones.len()], ph_frames.clone())?),
                    ("note_midi", tensor_f32(&[1, n], note_midi)?),
                    ("note_rest", tensor_bool(&[1, n], note_rest)?),
                    ("note_dur", tensor_i64(&[1, n], note_dur)?),
                    ("pitch", tensor_f32(&[1, frames], base_midi.clone())?),
                    ("retake", tensor_bool(&[1, frames], vec![true; frames])?),
                    ("steps", scalar_i64(i64::from(input.steps))?),
                ];
                if *use_expr {
                    inputs.push(("expr", tensor_f32(&[1, frames], vec![1.0; frames])?));
                }
                let out = stage.predictor.run(
                    inputs,
                    input.seed ^ SEED_PITCH,
                    VARIANCE_NOISE_PER_FRAME * frames,
                    &["pitch_pred"],
                )?;
                Some(out[0].iter().map(|m| m * 100.0).collect::<Vec<f32>>())
            }
            None => None,
        };

        // Variance model, conditioned on the predicted (or base) pitch.
        let mut variance = BTreeMap::new();
        if let Some(vs) = &self.variance
            && !vs.params.is_empty()
        {
            let enc = self.encode(&vs.stage, &phones)?;
            let pitch_midi: Vec<f32> = match &pitch {
                Some(p) => p.iter().map(|c| c / 100.0).collect(),
                None => base_midi.clone(),
            };
            let nv = vs.params.len();
            let mut inputs = vec![
                ("encoder_out", enc),
                ("ph_dur", tensor_i64(&[1, phones.len()], ph_frames.clone())?),
                ("pitch", tensor_f32(&[1, frames], pitch_midi)?),
                (
                    "retake",
                    tensor_bool(&[1, frames, nv], vec![true; frames * nv])?,
                ),
                ("steps", scalar_i64(i64::from(input.steps))?),
            ];
            for p in &vs.params {
                inputs.push((p, tensor_f32(&[1, frames], vec![0.0; frames])?));
            }
            let names: Vec<String> = vs.params.iter().map(|p| format!("{p}_pred")).collect();
            let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let out = vs.stage.predictor.run(
                inputs,
                input.seed ^ SEED_VARIANCE,
                VARIANCE_NOISE_PER_FRAME * frames,
                &name_refs,
            )?;
            for (p, v) in vs.params.iter().zip(out) {
                variance.insert((*p).to_owned(), v);
            }
        }
        Ok(Prediction {
            phones,
            pitch,
            variance,
            diagnostics,
        })
    }

    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        let frames = input.f0_ref_hz.len();
        let (tokens, langs) = self.acoustic_vocab.encode(input.phones)?;
        let t = tokens.len();
        let durations: Vec<i64> = input.phones.iter().map(|p| p.frames).collect();
        let curve = |p: &str, default: f32| {
            input
                .variance
                .get(p)
                .cloned()
                .unwrap_or_else(|| vec![default; frames])
        };
        // Cantor's gender is a formant shift (positive = brighter); the
        // model's key shift moves formants the other way.
        let gender: Vec<f32> = curve("gender", 0.0).iter().map(|g| -g).collect();
        let mut inputs = vec![
            ("tokens", tensor_i64(&[1, t], tokens)?),
            ("languages", tensor_i64(&[1, t], langs)?),
            ("durations", tensor_i64(&[1, t], durations)?),
            ("f0", tensor_f32(&[1, frames], input.f0_ref_hz.to_vec())?),
            ("gender", tensor_f32(&[1, frames], gender)?),
            (
                "velocity",
                tensor_f32(&[1, frames], curve("velocity", 1.0))?,
            ),
            ("depth", scalar_f32(self.acoustic_cfg.max_depth)?),
            ("steps", scalar_i64(i64::from(input.steps))?),
        ];
        for p in ["energy", "breathiness", "voicing", "tension"] {
            if self.acoustic.has_input(p) {
                inputs.push((
                    p,
                    tensor_f32(&[1, frames], curve(p, self.param_default(p)))?,
                ));
            }
        }
        let out = self.acoustic.run(
            inputs,
            input.seed ^ SEED_ACOUSTIC,
            ACOUSTIC_NOISE_PER_FRAME * frames,
            &["mel"],
        )?;
        Ok(out.into_iter().next().expect("one output"))
    }

    fn vocode(&self, mel: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        let frames = f0_hz.len();
        let bins = self.grid.mel_bins;
        if mel.len() != frames * bins {
            return Err(BackendError(format!(
                "mel has {} values; expected {frames} frames × {bins} bins",
                mel.len()
            )));
        }
        let out = self.vocoder.run(
            vec![
                ("mel", tensor_f32(&[1, frames, bins], mel.to_vec())?),
                ("f0", tensor_f32(&[1, frames], f0_hz.to_vec())?),
            ],
            0,
            0,
            &["waveform"],
        )?;
        Ok(out.into_iter().next().expect("one output"))
    }
}

impl DiffSinger {
    /// Runs a stage's linguistic encoder in phoneme mode (with ph_dur).
    fn encode(
        &self,
        stage: &Stage,
        phones: &[Phone],
    ) -> Result<ort::value::DynValue, BackendError> {
        let (tokens, langs) = stage.vocab.encode(phones)?;
        let t = tokens.len();
        let ph_dur: Vec<i64> = phones.iter().map(|p| p.frames).collect();
        let out = stage.linguistic.run(
            vec![
                ("tokens", tensor_i64(&[1, t], tokens)?),
                ("languages", tensor_i64(&[1, t], langs)?),
                ("ph_dur", tensor_i64(&[1, t], ph_dur)?),
            ],
            0,
            0,
            &["encoder_out"],
        )?;
        let hidden = out[0].len() / t;
        tensor_f32(&[1, t, hidden], out[0].clone())
    }
}

/// Notes for the pitch model: head rest, notes and gaps, tail rest. Rests
/// take the nearest note's pitch. Durations in frames sum to the window.
fn pitch_notes(
    notes: &[PhraseNote],
    t0: f64,
    t_end: f64,
    fs: f64,
) -> (Vec<f32>, Vec<bool>, Vec<i64>) {
    let mut midi = Vec::new();
    let mut rest = Vec::new();
    let mut lengths = Vec::new();
    let mut cursor = t0;
    for (i, n) in notes.iter().enumerate() {
        let m = (n.pitch_cents / 100.0) as f32;
        if n.start_sec > cursor + 1e-9 {
            midi.push(m);
            rest.push(true);
            lengths.push(n.start_sec - cursor);
        }
        let silent = matches!(parse_lyric(&n.lyric), Lyric::Rest | Lyric::Breath);
        let end = notes
            .get(i + 1)
            .map_or(n.end_sec, |nx| nx.start_sec.min(n.end_sec).max(n.start_sec));
        midi.push(m);
        rest.push(silent);
        lengths.push(end - n.start_sec.max(cursor));
        cursor = end;
    }
    let last = notes
        .last()
        .map_or(60.0, |n| (n.pitch_cents / 100.0) as f32);
    midi.push(last);
    rest.push(true);
    lengths.push(t_end - cursor);
    (midi, rest, to_frames(&lengths, fs))
}
