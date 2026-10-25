//! Content-addressed cache keys.
//!
//! Every field is written with a tag and a fixed-width encoding, so keys
//! cannot collide by concatenation and floats hash by their exact bits.

use std::fmt;

/// A BLAKE3 cache key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub [u8; 32]);

impl Key {
    /// Lowercase hex.
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({})", &self.hex()[..16])
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

/// Builds a key field by field.
#[derive(Debug)]
pub struct KeyBuilder(blake3::Hasher);

impl KeyBuilder {
    /// Starts a key for a domain such as `"mel"` or `"audio"`.
    pub fn new(domain: &str) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"cantor-key-v1\0");
        let mut b = Self(h);
        b.str("domain", domain);
        b
    }

    fn tag(&mut self, tag: &str) {
        self.0.update(&(tag.len() as u32).to_le_bytes());
        self.0.update(tag.as_bytes());
    }

    /// Adds a string.
    pub fn str(&mut self, tag: &str, v: &str) -> &mut Self {
        self.tag(tag);
        self.0.update(&(v.len() as u64).to_le_bytes());
        self.0.update(v.as_bytes());
        self
    }

    /// Adds an integer.
    pub fn u64(&mut self, tag: &str, v: u64) -> &mut Self {
        self.tag(tag);
        self.0.update(&v.to_le_bytes());
        self
    }

    /// Adds a signed integer.
    pub fn i64(&mut self, tag: &str, v: i64) -> &mut Self {
        self.u64(tag, v as u64)
    }

    /// Adds f32 samples by their exact bits.
    pub fn f32s(&mut self, tag: &str, v: &[f32]) -> &mut Self {
        self.tag(tag);
        self.0.update(&(v.len() as u64).to_le_bytes());
        for x in v {
            self.0.update(&x.to_bits().to_le_bytes());
        }
        self
    }

    /// Adds integers.
    pub fn i64s(&mut self, tag: &str, v: &[i64]) -> &mut Self {
        self.tag(tag);
        self.0.update(&(v.len() as u64).to_le_bytes());
        for x in v {
            self.0.update(&x.to_le_bytes());
        }
        self
    }

    /// Adds strings.
    pub fn strs(&mut self, tag: &str, v: &[String]) -> &mut Self {
        self.tag(tag);
        self.0.update(&(v.len() as u64).to_le_bytes());
        for s in v {
            self.0.update(&(s.len() as u64).to_le_bytes());
            self.0.update(s.as_bytes());
        }
        self
    }

    /// Adds another key.
    pub fn key(&mut self, tag: &str, k: &Key) -> &mut Self {
        self.tag(tag);
        self.0.update(&k.0);
        self
    }

    /// Finishes the key.
    pub fn finish(&self) -> Key {
        Key(*self.0.finalize().as_bytes())
    }
}

/// BLAKE3 of audio samples, used as the render's audio hash.
pub fn audio_hash(samples: &[f32]) -> Key {
    KeyBuilder::new("audio-hash")
        .f32s("samples", samples)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_do_not_run_together() {
        let a = KeyBuilder::new("t").str("a", "xy").str("b", "z").finish();
        let b = KeyBuilder::new("t").str("a", "x").str("b", "yz").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn floats_hash_by_bits() {
        let a = KeyBuilder::new("t").f32s("v", &[0.0]).finish();
        let b = KeyBuilder::new("t").f32s("v", &[-0.0]).finish();
        assert_ne!(a, b);
    }
}
