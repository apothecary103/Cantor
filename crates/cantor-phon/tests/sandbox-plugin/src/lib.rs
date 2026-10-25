//! Tries to escape the sandbox; each lyric names one attempt. The host
//! tests check that every attempt is contained.

use cantor_plugin_sdk::*;

struct Probe;

impl Phonemizer for Probe {
    fn info() -> Info {
        Info {
            name: "sandbox probe".into(),
            version: "0".into(),
            api_version: API_VERSION,
            languages: vec!["xx".into()],
        }
    }

    fn inventory(_: String) -> Vec<PhoneInfo> {
        Vec::new()
    }

    fn phonemize(notes: Vec<Note>) -> Result<Vec<NotePhonemes>, String> {
        let probe = |lyric: &str| -> String {
            match lyric {
                "loop" => {
                    let mut x: u64 = 0;
                    loop {
                        x = std::hint::black_box(x.wrapping_add(1));
                    }
                }
                "fs" => match std::fs::read_dir("/") {
                    Ok(d) => format!("listed {} entries", d.count()),
                    Err(e) => format!("denied: {e}"),
                },
                "env" => format!("{} vars", std::env::vars().count()),
                "time" => format!(
                    "{:?}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                ),
                "alloc" => {
                    let mut v: Vec<Vec<u8>> = Vec::new();
                    loop {
                        v.push(vec![1u8; 64 << 20]);
                    }
                }
                other => format!("unknown probe {other}"),
            }
        };
        Ok(notes
            .iter()
            .map(|n| NotePhonemes {
                phonemes: Vec::new(),
                warnings: vec![probe(&n.lyric)],
            })
            .collect())
    }
}

export_phonemizer!(Probe);
