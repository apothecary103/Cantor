//! WAV output.

use std::path::Path;

/// Writes interleaved stereo f32 samples as a 32-bit float WAV.
pub fn write_stereo_f32(
    path: &Path,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        w.write_sample(*s)?;
    }
    w.finalize()
}
