//! WAV output and read-back.

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

/// Reads a stereo 32-bit float WAV written by [`write_stereo_f32`]:
/// interleaved samples and the sample rate.
pub fn read_stereo_f32(path: &Path) -> Result<(Vec<f32>, u32), hound::Error> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    if spec.channels != 2 || spec.sample_format != hound::SampleFormat::Float {
        return Err(hound::Error::Unsupported);
    }
    let samples = r.samples::<f32>().collect::<Result<_, _>>()?;
    Ok((samples, spec.sample_rate))
}
