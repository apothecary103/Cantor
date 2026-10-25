//! The WASM phonemizer host (docs/spec/20-architecture.md, cantor-phon).
//!
//! Phonemizers are WebAssembly components implementing the `phonemizer`
//! world in `crates/cantor-plugin-sdk/wit`. They run sandboxed and
//! deterministically:
//! - WASI p2 with no preopened directories, environment, arguments or
//!   network, and stdout/stderr discarded;
//! - frozen wall and monotonic clocks, and seeded randomness, so a plugin
//!   cannot make renders depend on when or where they run;
//! - a fuel budget per call and a memory cap, so a runaway plugin fails
//!   with an error instead of hanging the engine.
//!
//! WASI 0.3 is ratified, but Wasmtime's implementation is still
//! experimental and stable Rust cannot target it yet, so plugins target
//! WASI p2 (docs/decisions/0019-wasm-phonemizers.md).

use crate::PhoneKind;
use crate::xsampa::XSampaMap;
use rand::SeedableRng;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../cantor-plugin-sdk/wit",
        world: "phonemizer",
    });
}

use bindings::cantor::plugin::phonemizer_types as types;

/// Plugin API version this host speaks.
pub const API_VERSION: u32 = 1;
/// Fuel (roughly, WebAssembly instructions) a plugin may spend per call.
pub const FUEL_PER_CALL: u64 = 5_000_000_000;
/// Largest linear memory a plugin may grow to.
pub const MEMORY_LIMIT: usize = 512 << 20;

/// A note handed to a phonemizer.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginNote {
    /// Lyric as typed.
    pub lyric: String,
    /// Language tag.
    pub lang: String,
    /// Onset in seconds from the phrase start.
    pub start: f64,
    /// End in seconds from the phrase start.
    pub end: f64,
    /// Pitch in cents.
    pub pitch_cents: f64,
}

/// A phonemizer's answer for one note.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginPhonemes {
    /// Native symbols; empty lets the host decide from the lyric.
    pub phonemes: Vec<String>,
    /// Problems to report.
    pub warnings: Vec<String>,
}

/// A phonemizer's phoneme inventory for one language.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    kinds: HashMap<String, PhoneKind>,
    xsampa: XSampaMap,
}

impl Inventory {
    /// The kind of a symbol; unknown symbols count as consonants.
    pub fn kind(&self, symbol: &str) -> PhoneKind {
        self.kinds
            .get(symbol)
            .copied()
            .unwrap_or(PhoneKind::Consonant)
    }

    /// Whether the plugin declared the symbol.
    pub fn has_symbol(&self, symbol: &str) -> bool {
        self.kinds.contains_key(symbol)
    }

    /// The declared X-SAMPA mapping.
    pub fn xsampa(&self) -> &XSampaMap {
        &self.xsampa
    }
}

/// Why a plugin could not be loaded or called.
#[derive(Debug, thiserror::Error)]
#[error("phonemizer {plugin}: {message}")]
pub struct PluginError {
    /// The plugin file.
    pub plugin: String,
    /// What went wrong.
    pub message: String,
}

struct Host {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Clocks that never move.
struct Frozen;

impl HostWallClock for Frozen {
    fn resolution(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

impl HostMonotonicClock for Frozen {
    fn resolution(&self) -> u64 {
        1_000_000_000
    }
    fn now(&self) -> u64 {
        0
    }
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.consume_fuel(true);
        Engine::new(&config).expect("wasmtime engine configuration is valid")
    })
}

/// A loaded phonemizer plugin.
pub struct WasmPhonemizer {
    path: PathBuf,
    name: String,
    version: String,
    languages: Vec<String>,
    /// BLAKE3 of the component bytes, for cache keys.
    hash: String,
    component: Component,
    linker: Linker<Host>,
    instance: Mutex<(Store<Host>, bindings::Phonemizer)>,
    inventories: Mutex<HashMap<String, Arc<Inventory>>>,
}

impl std::fmt::Debug for WasmPhonemizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPhonemizer")
            .field("path", &self.path)
            .field("name", &self.name)
            .field("version", &self.version)
            .field("languages", &self.languages)
            .finish_non_exhaustive()
    }
}

impl WasmPhonemizer {
    /// Loads and instantiates a plugin, and checks its API version.
    pub fn load(path: &Path) -> Result<Self, PluginError> {
        let err = |message: String| PluginError {
            plugin: path.display().to_string(),
            message,
        };
        let bytes = std::fs::read(path).map_err(|e| err(e.to_string()))?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let component = Component::new(engine(), &bytes).map_err(|e| err(format!("{e:#}")))?;
        let mut linker = Linker::<Host>::new(engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| err(format!("{e:#}")))?;
        let (mut store, plugin) =
            instantiate(&component, &linker).map_err(|e| err(format!("instantiate: {e:#}")))?;
        store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| err(format!("{e:#}")))?;
        let info = plugin
            .call_info(&mut store)
            .map_err(|e| err(format!("info(): {e:#}")))?;
        if info.api_version != API_VERSION {
            return Err(err(format!(
                "targets plugin API {}, this Cantor speaks {API_VERSION}",
                info.api_version
            )));
        }
        Ok(Self {
            path: path.to_owned(),
            name: info.name,
            version: info.version,
            languages: info.languages,
            hash,
            component,
            linker,
            instance: Mutex::new((store, plugin)),
            inventories: Mutex::new(HashMap::new()),
        })
    }
}

/// A fresh store and instance: sandboxed WASI, frozen clocks, seeded
/// randomness, memory cap.
fn instantiate(
    component: &Component,
    linker: &Linker<Host>,
) -> wasmtime::Result<(Store<Host>, bindings::Phonemizer)> {
    {
        let wasi = WasiCtx::builder()
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .wall_clock(Frozen)
            .monotonic_clock(Frozen)
            .secure_random(rand::rngs::StdRng::seed_from_u64(0))
            .insecure_random(rand::rngs::StdRng::seed_from_u64(1))
            .insecure_random_seed(0)
            .build();
        let mut store = Store::new(
            engine(),
            Host {
                wasi,
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(MEMORY_LIMIT)
                    .instances(16)
                    .build(),
            },
        );
        store.limiter(|h| &mut h.limits);
        store.set_fuel(FUEL_PER_CALL)?;
        let plugin = bindings::Phonemizer::instantiate(&mut store, component, linker)?;
        Ok((store, plugin))
    }
}

impl WasmPhonemizer {
    /// The plugin file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Plugin version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Languages the plugin phonemizes.
    pub fn languages(&self) -> &[String] {
        &self.languages
    }

    /// Identity for cache keys: name, version and a hash of the bytes.
    pub fn fingerprint(&self) -> String {
        format!("{}@{}#{}", self.name, self.version, &self.hash[..16])
    }

    /// Whether the plugin handles a language (region subtags ignored).
    pub fn handles(&self, lang: &str) -> bool {
        let base = |l: &str| l.split(['-', '_']).next().unwrap_or(l).to_owned();
        self.languages.iter().any(|l| base(l) == base(lang))
    }

    /// Calls into the plugin with a fresh fuel budget. A trap (fuel or
    /// memory exhausted, a guest panic) poisons a component instance, so
    /// the instance is replaced before the error is returned.
    fn call<R>(
        &self,
        f: impl FnOnce(&mut Store<Host>, &bindings::Phonemizer) -> wasmtime::Result<R>,
    ) -> Result<R, String> {
        let mut guard = self.instance.lock().expect("plugin lock");
        let (store, plugin) = &mut *guard;
        store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| format!("{e:#}"))?;
        match f(store, plugin) {
            Ok(r) => Ok(r),
            Err(e) => {
                let out_of_fuel = store.get_fuel().is_ok_and(|f| f == 0);
                if let Ok(fresh) = instantiate(&self.component, &self.linker) {
                    *guard = fresh;
                }
                if out_of_fuel {
                    Err(format!(
                        "ran out of fuel ({FUEL_PER_CALL} units); the plugin may loop forever"
                    ))
                } else {
                    Err(format!("{e:#}"))
                }
            }
        }
    }

    fn err(&self, message: String) -> PluginError {
        PluginError {
            plugin: self.path.display().to_string(),
            message,
        }
    }

    /// The plugin's inventory for a language, cached.
    pub fn inventory(&self, lang: &str) -> Result<Arc<Inventory>, PluginError> {
        if let Some(inv) = self.inventories.lock().expect("inventory lock").get(lang) {
            return Ok(inv.clone());
        }
        let phones = self
            .call(|store, plugin| plugin.call_inventory(store, lang))
            .map_err(|e| self.err(format!("inventory({lang}): {e}")))?;
        let mut kinds = HashMap::new();
        let mut map = String::new();
        for p in phones {
            let kind = match p.kind {
                types::PhoneKind::Vowel => PhoneKind::Vowel,
                types::PhoneKind::Consonant => PhoneKind::Consonant,
            };
            if let Some(x) = &p.xsampa {
                map.push_str(&format!("{}\t{x}\n", p.symbol));
            }
            kinds.insert(p.symbol, kind);
        }
        let xsampa = XSampaMap::parse(&map)
            .map_err(|e| self.err(format!("inventory({lang}) X-SAMPA: {e}")))?;
        let inv = Arc::new(Inventory { kinds, xsampa });
        self.inventories
            .lock()
            .expect("inventory lock")
            .insert(lang.to_owned(), inv.clone());
        Ok(inv)
    }

    /// Phonemizes a phrase: one result per note.
    pub fn phonemize(&self, notes: &[PluginNote]) -> Result<Vec<PluginPhonemes>, PluginError> {
        let input: Vec<types::Note> = notes
            .iter()
            .map(|n| types::Note {
                lyric: n.lyric.clone(),
                lang: n.lang.clone(),
                start: n.start,
                end: n.end,
                pitch_cents: n.pitch_cents,
            })
            .collect();
        let out = self
            .call(|store, plugin| plugin.call_phonemize(store, &input))
            .map_err(|e| self.err(format!("phonemize: {e}")))?
            .map_err(|e| self.err(format!("phonemize failed: {e}")))?;
        if out.len() != notes.len() {
            return Err(self.err(format!(
                "returned {} results for {} notes",
                out.len(),
                notes.len()
            )));
        }
        Ok(out
            .into_iter()
            .map(|r| PluginPhonemes {
                phonemes: r.phonemes,
                warnings: r.warnings,
            })
            .collect())
    }
}

/// Every plugin found on a search path, by language.
#[derive(Debug, Default)]
pub struct PluginSet {
    plugins: Vec<Arc<WasmPhonemizer>>,
}

impl PluginSet {
    /// Loads every `*.wasm` file in `dirs` (sorted by path, so the choice
    /// between plugins for one language is stable). Plugins that fail to
    /// load are returned as errors next to the set.
    pub fn scan(dirs: &[PathBuf]) -> (Self, Vec<PluginError>) {
        let mut files: Vec<PathBuf> = dirs
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wasm"))
            .collect();
        files.sort();
        let mut set = Self::default();
        let mut errors = Vec::new();
        for f in files {
            match WasmPhonemizer::load(&f) {
                Ok(p) => set.plugins.push(Arc::new(p)),
                Err(e) => errors.push(e),
            }
        }
        (set, errors)
    }

    /// A set with the given plugins.
    pub fn new(plugins: Vec<Arc<WasmPhonemizer>>) -> Self {
        Self { plugins }
    }

    /// The first plugin that handles a language.
    pub fn for_lang(&self, lang: &str) -> Option<&Arc<WasmPhonemizer>> {
        self.plugins.iter().find(|p| p.handles(lang))
    }

    /// Every loaded plugin.
    pub fn plugins(&self) -> &[Arc<WasmPhonemizer>] {
        &self.plugins
    }

    /// Whether no plugins are loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Identity of the whole set for cache keys; empty when no plugins.
    pub fn fingerprint(&self) -> String {
        let parts: BTreeMap<String, String> = self
            .plugins
            .iter()
            .map(|p| (p.fingerprint(), p.languages.join(",")))
            .collect();
        parts
            .into_iter()
            .map(|(k, v)| format!("{k}[{v}]"))
            .collect::<Vec<_>>()
            .join(";")
    }
}
