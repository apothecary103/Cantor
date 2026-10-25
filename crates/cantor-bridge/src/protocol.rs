//! The bridge's shared-memory protocol: the engine publishes rendered
//! audio, the plugin plays it in sync with the host and reports the host's
//! transport back.
//!
//! Layout of a channel file (all little-endian, native atomics):
//!
//! ```text
//! [0, 4096)        header (u64 words, see `H_*`)
//! [4096, +cap)     slot 0: interleaved stereo f32, `cap` bytes
//! [4096+cap, ...)  slot 1
//! ```
//!
//! The engine writes into the slot that is not active, bracketing the
//! write with the slot's sequence number (odd while writing), then makes
//! it active. The plugin's audio thread reads the active slot's sequence,
//! copies samples, and re-reads the sequence; a change means the engine
//! overwrote the slot mid-copy, and the block is replaced by silence. The
//! plugin writes host transport under its own sequence number the same
//! way. No locks, no allocation, no syscalls on the audio thread.
//!
//! The protocol works on any `&[AtomicU64]` region, so it is tested with
//! Miri on ordinary memory; `Channel` maps it onto a file.

use std::sync::atomic::{AtomicU64, Ordering};

/// "CANTORBR" as a little-endian u64.
pub const MAGIC: u64 = u64::from_le_bytes(*b"CANTORBR");
/// Protocol version.
pub const VERSION: u64 = 1;
/// Header size in bytes.
pub const HEADER_BYTES: usize = 4096;
/// Header size in u64 words.
pub const HEADER_WORDS: usize = HEADER_BYTES / 8;

// Header word indices.
const H_MAGIC: usize = 0;
const H_VERSION: usize = 1;
/// Bytes per slot.
const H_SLOT_BYTES: usize = 2;
/// Index of the active slot (0 or 1), or u64::MAX before the first publish.
const H_ACTIVE: usize = 3;
/// Per slot: sequence, sample rate, frames, generation (4 words each).
const H_SLOTS: usize = 8;
const SLOT_WORDS: usize = 4;
/// Host → engine: sequence, flags, sample rate, tempo, position seconds,
/// position beats, heartbeat (f64 values stored as bits).
const H_HOST: usize = 32;

/// Host transport as the plugin last saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostState {
    /// The host is playing.
    pub playing: bool,
    /// The host's sample rate.
    pub sample_rate: f64,
    /// Tempo in BPM, if the host reports one.
    pub tempo: Option<f64>,
    /// Song position in seconds.
    pub position_secs: f64,
    /// Song position in quarter notes.
    pub position_beats: f64,
    /// Process calls since the plugin started; changes while it runs.
    pub heartbeat: u64,
}

/// A view of a channel region: header plus two slots of samples.
pub struct Region<'a> {
    words: &'a [AtomicU64],
}

impl std::fmt::Debug for Region<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Region")
            .field("words", &self.words.len())
            .finish()
    }
}

impl<'a> Region<'a> {
    /// Wraps a region of `HEADER_WORDS + 2 * slot_words` words.
    pub fn new(words: &'a [AtomicU64]) -> Self {
        Self { words }
    }

    /// Writes a fresh header for slots of `slot_bytes` each.
    pub fn init(&self, slot_bytes: u64) {
        for w in &self.words[..HEADER_WORDS] {
            w.store(0, Ordering::Relaxed);
        }
        self.words[H_SLOT_BYTES].store(slot_bytes, Ordering::Relaxed);
        self.words[H_ACTIVE].store(u64::MAX, Ordering::Relaxed);
        self.words[H_VERSION].store(VERSION, Ordering::Relaxed);
        self.words[H_MAGIC].store(MAGIC, Ordering::Release);
    }

    /// Whether the header is a valid channel of this version.
    pub fn is_valid(&self) -> bool {
        self.words.len() >= HEADER_WORDS
            && self.words[H_MAGIC].load(Ordering::Acquire) == MAGIC
            && self.words[H_VERSION].load(Ordering::Relaxed) == VERSION
    }

    fn slot_bytes(&self) -> usize {
        self.words[H_SLOT_BYTES].load(Ordering::Relaxed) as usize
    }

    /// Frames a slot can hold (stereo f32).
    pub fn capacity_frames(&self) -> usize {
        self.slot_bytes() / 8
    }

    fn slot_word(&self, slot: usize, field: usize) -> &AtomicU64 {
        &self.words[H_SLOTS + slot * SLOT_WORDS + field]
    }

    /// Samples of a slot as atomic words, each holding two f32 (L, R).
    fn slot_data(&self, slot: usize) -> &[AtomicU64] {
        let frames = self.capacity_frames();
        let start = HEADER_WORDS + slot * frames;
        &self.words[start..start + frames]
    }

    /// Publishes interleaved stereo audio. Returns the generation, or an
    /// error when it does not fit.
    pub fn publish(&self, sample_rate: u32, interleaved: &[f32]) -> Result<u64, String> {
        let frames = interleaved.len() / 2;
        if frames > self.capacity_frames() {
            return Err(format!(
                "{frames} frames do not fit the channel ({} frames)",
                self.capacity_frames()
            ));
        }
        let active = self.words[H_ACTIVE].load(Ordering::Acquire);
        let slot = if active == 0 { 1 } else { 0 };
        let seq = self.slot_word(slot, 0);
        let s0 = seq.load(Ordering::Relaxed);
        seq.store(s0 | 1, Ordering::Relaxed); // odd: writing
        std::sync::atomic::fence(Ordering::Release);
        let data = self.slot_data(slot);
        for (w, f) in data.iter().zip(interleaved.as_chunks::<2>().0) {
            let bits = u64::from(f[0].to_bits()) | (u64::from(f[1].to_bits()) << 32);
            w.store(bits, Ordering::Relaxed);
        }
        self.slot_word(slot, 1)
            .store(u64::from(sample_rate), Ordering::Relaxed);
        self.slot_word(slot, 2)
            .store(frames as u64, Ordering::Relaxed);
        let generation = self
            .slot_word(1 - slot, 3)
            .load(Ordering::Relaxed)
            .max(self.slot_word(slot, 3).load(Ordering::Relaxed))
            + 1;
        self.slot_word(slot, 3).store(generation, Ordering::Relaxed);
        seq.store((s0 | 1) + 1, Ordering::Release); // even: stable
        self.words[H_ACTIVE].store(slot as u64, Ordering::Release);
        Ok(generation)
    }

    /// Reads `out.len() / 2` stereo frames starting at `frame` of the
    /// active slot (zeros past its end). Returns the slot's sample rate
    /// and generation, or `None` when nothing is published or the engine
    /// overwrote the slot during the copy (then `out` is silence).
    /// Realtime-safe.
    pub fn read(&self, frame: usize, out: &mut [f32]) -> Option<(u32, u64)> {
        out.fill(0.0);
        let active = self.words[H_ACTIVE].load(Ordering::Acquire);
        if active > 1 {
            return None;
        }
        let slot = active as usize;
        let seq = self.slot_word(slot, 0);
        let s0 = seq.load(Ordering::Acquire);
        if s0 & 1 == 1 {
            return None;
        }
        let sr = self.slot_word(slot, 1).load(Ordering::Relaxed) as u32;
        let frames = self.slot_word(slot, 2).load(Ordering::Relaxed) as usize;
        let generation = self.slot_word(slot, 3).load(Ordering::Relaxed);
        let data = self.slot_data(slot);
        for (i, o) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let f = frame + i;
            if f >= frames {
                break;
            }
            let bits = data[f].load(Ordering::Relaxed);
            o[0] = f32::from_bits(bits as u32);
            o[1] = f32::from_bits((bits >> 32) as u32);
        }
        std::sync::atomic::fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) != s0 {
            out.fill(0.0);
            return None;
        }
        Some((sr, generation))
    }

    /// Sample rate, frame count and generation of the active slot, if
    /// anything is published. Realtime-safe.
    pub fn active_info(&self) -> Option<(u32, usize, u64)> {
        let active = self.words[H_ACTIVE].load(Ordering::Acquire);
        if active > 1 {
            return None;
        }
        let slot = active as usize;
        let seq = self.slot_word(slot, 0);
        let s0 = seq.load(Ordering::Acquire);
        let info = (
            self.slot_word(slot, 1).load(Ordering::Relaxed) as u32,
            self.slot_word(slot, 2).load(Ordering::Relaxed) as usize,
            self.slot_word(slot, 3).load(Ordering::Relaxed),
        );
        std::sync::atomic::fence(Ordering::Acquire);
        (s0 & 1 == 0 && seq.load(Ordering::Relaxed) == s0).then_some(info)
    }

    /// Records host transport (plugin side, realtime-safe).
    pub fn write_host(&self, h: &HostState) {
        let w = |i: usize| &self.words[H_HOST + i];
        let s0 = w(0).load(Ordering::Relaxed);
        w(0).store(s0 | 1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        w(1).store(
            u64::from(h.playing) | (u64::from(h.tempo.is_some()) << 1),
            Ordering::Relaxed,
        );
        w(2).store(h.sample_rate.to_bits(), Ordering::Relaxed);
        w(3).store(h.tempo.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
        w(4).store(h.position_secs.to_bits(), Ordering::Relaxed);
        w(5).store(h.position_beats.to_bits(), Ordering::Relaxed);
        w(6).store(h.heartbeat, Ordering::Relaxed);
        w(0).store((s0 | 1) + 1, Ordering::Release);
    }

    /// The host transport the plugin last recorded, if any (engine side).
    pub fn read_host(&self) -> Option<HostState> {
        let w = |i: usize| &self.words[H_HOST + i];
        for _ in 0..64 {
            let s0 = w(0).load(Ordering::Acquire);
            if s0 == 0 {
                return None;
            }
            if s0 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let flags = w(1).load(Ordering::Relaxed);
            let h = HostState {
                playing: flags & 1 == 1,
                sample_rate: f64::from_bits(w(2).load(Ordering::Relaxed)),
                tempo: (flags & 2 == 2).then(|| f64::from_bits(w(3).load(Ordering::Relaxed))),
                position_secs: f64::from_bits(w(4).load(Ordering::Relaxed)),
                position_beats: f64::from_bits(w(5).load(Ordering::Relaxed)),
                heartbeat: w(6).load(Ordering::Relaxed),
            };
            std::sync::atomic::fence(Ordering::Acquire);
            if w(0).load(Ordering::Relaxed) == s0 {
                return Some(h);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn region_words(slot_frames: usize) -> Vec<AtomicU64> {
        (0..HEADER_WORDS + 2 * slot_frames)
            .map(|_| AtomicU64::new(0))
            .collect()
    }

    #[test]
    fn publish_then_read_round_trips() {
        let words = region_words(64);
        let r = Region::new(&words);
        assert!(!r.is_valid());
        r.init(64 * 8);
        assert!(r.is_valid());
        assert_eq!(r.capacity_frames(), 64);
        let mut out = [1.0f32; 8];
        assert_eq!(r.read(0, &mut out), None, "nothing published yet");
        assert_eq!(out, [0.0; 8]);

        let audio: Vec<f32> = (0..40).map(|i| i as f32).collect(); // 20 frames
        let g1 = r.publish(44_100, &audio).unwrap();
        assert_eq!(r.active_info(), Some((44_100, 20, g1)));
        assert_eq!(r.read(2, &mut out), Some((44_100, g1)));
        assert_eq!(out, [4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
        // Past the end: zeros.
        assert_eq!(r.read(18, &mut out), Some((44_100, g1)));
        assert_eq!(out, [36.0, 37.0, 38.0, 39.0, 0.0, 0.0, 0.0, 0.0]);

        let g2 = r.publish(48_000, &[0.5; 10]).unwrap();
        assert!(g2 > g1);
        assert_eq!(r.read(0, &mut out), Some((48_000, g2)));
        assert_eq!(out[..2], [0.5, 0.5]);
        assert!(r.publish(44_100, &vec![0.0; 2 * 65]).is_err(), "too long");
    }

    #[test]
    fn host_state_round_trips() {
        let words = region_words(4);
        let r = Region::new(&words);
        r.init(32);
        assert_eq!(r.read_host(), None);
        let h = HostState {
            playing: true,
            sample_rate: 48_000.0,
            tempo: Some(128.0),
            position_secs: 3.5,
            position_beats: 7.4666,
            heartbeat: 42,
        };
        r.write_host(&h);
        assert_eq!(r.read_host(), Some(h));
        r.write_host(&HostState { tempo: None, ..h });
        assert_eq!(r.read_host().unwrap().tempo, None);
    }

    /// A reader racing a publisher never sees a mix of two publications:
    /// every block is either one publication's samples or silence.
    #[test]
    fn readers_never_see_torn_blocks() {
        let frames = if cfg!(miri) { 16 } else { 4096 };
        let rounds = if cfg!(miri) { 20 } else { 2000 };
        let words: Arc<Vec<AtomicU64>> = Arc::new(region_words(frames));
        Region::new(&words).init((frames * 8) as u64);
        let w = words.clone();
        let writer = std::thread::spawn(move || {
            let r = Region::new(&w);
            for i in 0..rounds {
                let v = (i % 1000) as f32;
                r.publish(44_100, &vec![v; frames * 2]).unwrap();
            }
        });
        let r = Region::new(&words);
        let mut out = vec![0.0f32; frames * 2];
        for _ in 0..rounds {
            if r.read(0, &mut out).is_some() {
                assert!(out.iter().all(|&x| x == out[0]), "torn block");
            } else {
                assert!(out.iter().all(|&x| x == 0.0));
            }
        }
        writer.join().unwrap();
    }
}
