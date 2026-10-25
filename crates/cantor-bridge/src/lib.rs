//! The DAW bridge channel (docs/spec/70-milestones.md M6): Cantor renders
//! a project and publishes the audio into a shared-memory channel; the
//! Cantor plugin inside a DAW plays it in sync with the host's transport
//! and reports that transport (position, tempo, playing) back.
//!
//! [`protocol`] is the lock-free layout and logic on plain atomics;
//! [`Channel`] maps it onto a file both processes open.

pub mod protocol;

pub use protocol::{HostState, Region};

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

/// Default channel name.
pub const DEFAULT_CHANNEL: &str = "default";
/// Audio a channel can hold: 20 minutes of stereo at 48 kHz per slot. The
/// file is sparse, so only published audio uses disk or memory.
pub const CAPACITY_SECS: u64 = 20 * 60;
/// Highest sample rate the capacity is sized for.
pub const CAPACITY_RATE: u64 = 48_000;

/// Where channel files live: `$CANTOR_BRIDGE_DIR`, else a per-user
/// location both the app and the DAW can reach.
pub fn channel_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CANTOR_BRIDGE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos")
        && let Some(h) = home
    {
        return h.join("Library/Caches/Cantor/bridge");
    }
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(run).join("cantor-bridge");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "cantor".into());
    std::env::temp_dir().join(format!("cantor-bridge-{user}"))
}

/// The file of a named channel.
pub fn channel_path(name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    channel_dir().join(format!("{safe}.cantorbridge"))
}

/// A channel file mapped into this process.
pub struct Channel {
    path: PathBuf,
    map: memmap2::MmapRaw,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel").field("path", &self.path).finish()
    }
}

impl Channel {
    /// Opens a channel file, creating and initializing it if it does not
    /// exist or is not a channel of this protocol version.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let slot_bytes = CAPACITY_SECS * CAPACITY_RATE * 8;
        let len = protocol::HEADER_BYTES as u64 + 2 * slot_bytes;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        if file.metadata()?.len() != len {
            // Sparse: only written pages take space.
            file.set_len(len)?;
        }
        let map = memmap2::MmapOptions::new().map_raw(&file)?;
        let ch = Self {
            path: path.to_owned(),
            map,
        };
        if !ch.region().is_valid() {
            ch.region().init(slot_bytes);
        }
        Ok(ch)
    }

    /// Opens the named channel in [`channel_dir`].
    pub fn open_named(name: &str) -> std::io::Result<Self> {
        Self::open(&channel_path(name))
    }

    /// The channel file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The protocol view of the mapping.
    pub fn region(&self) -> Region<'_> {
        Region::new(self.words())
    }

    #[allow(unsafe_code)]
    fn words(&self) -> &[AtomicU64] {
        let ptr = self.map.as_mut_ptr();
        debug_assert_eq!(ptr as usize % std::mem::align_of::<AtomicU64>(), 0);
        // SAFETY: the mapping is page-aligned, lives as long as `self`, and
        // its length is a multiple of 8 (header and slots are). Every access
        // through the slice is atomic, so concurrent writes from the other
        // process are not data races.
        unsafe { std::slice::from_raw_parts(ptr.cast::<AtomicU64>(), self.map.len() / 8) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot mmap files")]
    fn two_mappings_share_one_channel() {
        let dir = std::env::temp_dir().join(format!("cantor-bridge-test-{}", std::process::id()));
        let path = dir.join("t.cantorbridge");
        let _ = std::fs::remove_file(&path);
        let a = Channel::open(&path).unwrap();
        let b = Channel::open(&path).unwrap();
        let audio: Vec<f32> = (0..200).map(|i| i as f32 / 200.0).collect();
        let g = a.region().publish(44_100, &audio).unwrap();
        let mut out = [0.0f32; 4];
        assert_eq!(b.region().read(10, &mut out), Some((44_100, g)));
        assert_eq!(out, [audio[20], audio[21], audio[22], audio[23]]);
        b.region().write_host(&HostState {
            playing: true,
            heartbeat: 1,
            ..HostState::default()
        });
        assert!(a.region().read_host().unwrap().playing);
        // Reopening keeps what was published.
        drop((a, b));
        let c = Channel::open(&path).unwrap();
        assert_eq!(c.region().active_info().map(|i| i.1), Some(100));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn channel_names_are_sanitized() {
        let p = channel_path("../evil name");
        assert_eq!(p.file_name().unwrap(), "___evil_name.cantorbridge");
        assert_eq!(p.parent().unwrap(), channel_dir());
    }
}
