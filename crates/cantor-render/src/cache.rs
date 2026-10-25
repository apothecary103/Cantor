//! The on-disk render cache: mel spectrograms and phrase audio, addressed
//! by [`Key`].

use crate::key::Key;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What a cache entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Mel spectrogram from the acoustic model.
    Mel,
    /// Phrase audio from the vocoder.
    Audio,
    /// Predicted phonemes, durations and curves.
    Prediction,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Kind::Mel => "mel",
            Kind::Audio => "pcm",
            Kind::Prediction => "json",
        }
    }
}

/// A directory of cached blobs, `<dir>/<kind>/<k[0..2]>/<key>.<ext>`.
#[derive(Clone, Debug)]
pub struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    /// Uses (and creates) `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The cache directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, kind: Kind, key: &Key) -> PathBuf {
        let hex = key.hex();
        self.dir
            .join(kind.ext())
            .join(&hex[..2])
            .join(format!("{hex}.{}", kind.ext()))
    }

    /// Reads an entry.
    pub fn get(&self, kind: Kind, key: &Key) -> Option<Vec<u8>> {
        std::fs::read(self.path(kind, key)).ok()
    }

    /// Writes an entry atomically.
    pub fn put(&self, kind: Kind, key: &Key, bytes: &[u8]) -> std::io::Result<()> {
        let path = self.path(kind, key);
        std::fs::create_dir_all(path.parent().expect("cache paths have parents"))?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
        }
        std::fs::rename(tmp, path)
    }

    /// Reads f32 samples.
    pub fn get_f32(&self, kind: Kind, key: &Key) -> Option<Vec<f32>> {
        let b = self.get(kind, key)?;
        let (chunks, rest) = b.as_chunks::<4>();
        rest.is_empty()
            .then(|| chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
    }

    /// Writes f32 samples.
    pub fn put_f32(&self, kind: Kind, key: &Key, v: &[f32]) -> std::io::Result<()> {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put(kind, key, &bytes)
    }
}
