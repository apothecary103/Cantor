//! WAV writing (hound).

use std::io::{Seek, Write};
use std::path::Path;

/// Sample format of a written WAV file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavFormat {
    /// 16-bit PCM (TPDF-free plain rounding).
    #[default]
    Pcm16,
    /// 24-bit PCM.
    Pcm24,
    /// 32-bit IEEE float.
    Float32,
}

/// Writes interleaved samples (nominally -1..=1; PCM clips) to `w`.
pub fn write<W: Write + Seek>(
    w: W,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    format: WavFormat,
) -> Result<(), hound::Error> {
    let (bits, sample_format) = match format {
        WavFormat::Pcm16 => (16, hound::SampleFormat::Int),
        WavFormat::Pcm24 => (24, hound::SampleFormat::Int),
        WavFormat::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bits,
        sample_format,
    };
    let mut out = hound::WavWriter::new(w, spec)?;
    for &s in samples {
        match format {
            WavFormat::Pcm16 => out.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16)?,
            WavFormat::Pcm24 => {
                out.write_sample((s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32)?
            }
            WavFormat::Float32 => out.write_sample(s)?,
        }
    }
    out.finalize()
}

/// Writes a WAV file.
pub fn write_file(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    format: WavFormat,
) -> Result<(), hound::Error> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write(f, samples, sample_rate, channels, format)
}

/// Encodes a WAV file in memory.
pub fn to_bytes(samples: &[f32], sample_rate: u32, channels: u16, format: WavFormat) -> Vec<u8> {
    let mut c = std::io::Cursor::new(Vec::new());
    write(&mut c, samples, sample_rate, channels, format).expect("in-memory WAV writes succeed");
    c.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip() {
        let s = [0.0f32, 0.5, -0.5, 1.5];
        for (f, bits) in [
            (WavFormat::Pcm16, 16),
            (WavFormat::Pcm24, 24),
            (WavFormat::Float32, 32),
        ] {
            let r = hound::WavReader::new(std::io::Cursor::new(to_bytes(&s, 44100, 1, f))).unwrap();
            assert_eq!((r.spec().bits_per_sample, r.len()), (bits, 4));
        }
    }
}
