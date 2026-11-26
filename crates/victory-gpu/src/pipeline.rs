//! Pipeline states. Both descriptions start as on PC: the root signature,
//! then the shader stages (five for graphics, one for compute); streams
//! carry the rest of the graphics state as subobjects of the standard
//! types.
//!
//! Host pipelines are made on a pipeline's first draw, not at creation:
//! The title creates some 60,000 pipelines in its first 20
//! seconds and draws with a few dozen (decision 0007, step 3). Shader
//! containers are shared by content hash and translated once, on first
//! use (decision 0003).
//!
//! shortcut: only vertex plus pixel pipelines from streams and compute
//! pipelines get host pipelines; mesh, tessellation and geometry stages,
//! and the plain graphics description, keep their containers only. The 32-byte
//! packet after the header stays zero: titles only copy it inline, which
//! our null command-list limits keep from running.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashMap;
use std::mem::offset_of;
use std::sync::{Arc, Mutex, OnceLock};

use crate::object::{self, E_INVALIDARG, HRESULT, Iid, Object, PIPELINE, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;

const PIPELINE_IIDS: [Iid; 4] = [
    guid(
        0x765a30f3,
        0xf624,
        0x4c6f,
        [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
pub(crate) struct Bytecode {
    code: *const u8,
    size: usize,
}

#[repr(C)]
pub(crate) struct GraphicsHead {
    root: This,
    /// Vertex, pixel, domain, hull, geometry.
    stages: [Bytecode; 5],
}

#[repr(C)]
pub(crate) struct ComputeHead {
    root: This,
    stage: Bytecode,
}

/// A shader container a title passed, shared by every pipeline with the
/// same bytes.
/// Guest shader container bytes: raw while hot, zlib-compressed once cold and
/// cached metadata (`walks`, `of_builder_spilled`, `translated`) is captured.
#[cfg(windows)]
enum ContainerBlob {
    Raw(Arc<[u8]>),
    Compressed { packed: Vec<u8>, orig_len: usize },
}

#[cfg(not(windows))]
enum ContainerBlob {
    Raw(Arc<[u8]>),
}

impl ContainerBlob {
    fn bytes_arc(&self) -> Arc<[u8]> {
        match self {
            Self::Raw(bytes) => Arc::clone(bytes),
            #[cfg(windows)]
            Self::Compressed { packed, orig_len } => {
                Arc::from(decompress_container(packed, *orig_len))
            }
        }
    }

    #[cfg(test)]
    fn to_vec(&self) -> Vec<u8> {
        self.bytes_arc().to_vec()
    }

    #[cfg(windows)]
    fn compress(&mut self) -> u64 {
        let Self::Raw(raw) = self else {
            return 0;
        };
        let orig_len = raw.len();
        if orig_len < 1024 {
            return 0;
        }
        let mut buf = vec![0u8; zlib_rs::compress_bound(orig_len)];
        let (packed, rc) =
            zlib_rs::compress_slice(&mut buf, raw, zlib_rs::DeflateConfig::new(1));
        if rc != zlib_rs::ReturnCode::Ok {
            return 0;
        }
        let packed_len = packed.len();
        if packed_len + 256 >= orig_len {
            return 0;
        }
        let saved = (orig_len - packed_len) as u64;
        let compressed = packed.to_vec();
        crate::ram_trim::COUNTERS
            .shader_container_retained_bytes
            .fetch_sub(saved, std::sync::atomic::Ordering::Relaxed);
        *self = Self::Compressed {
            packed: compressed,
            orig_len,
        };
        saved
    }
}

#[cfg(windows)]
fn decompress_container(packed: &[u8], orig_len: usize) -> Vec<u8> {
    if orig_len == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; orig_len];
    let mut state = zlib_rs::Inflate::new(true, 15);
    match state.decompress(packed, &mut out, zlib_rs::InflateFlush::Finish) {
        Ok(zlib_rs::Status::StreamEnd) if state.total_out() as usize == orig_len => out,
        _ => Vec::new(),
    }
}

pub(crate) struct Shader {
    pub(crate) stage: &'static str,
    /// The first 8 bytes of its BLAKE3, as logged.
    pub(crate) hash: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    container_len: usize,
    container: std::sync::Mutex<ContainerBlob>,
    /// Its result is replaced by the stand-in for indirect light
    /// ([`crate::ambient`]).
    #[cfg(windows)]
    pub(crate) resolves_light: bool,
    /// It draws the sky as the title reflects it ([`crate::ambient::SKY`]).
    #[cfg(windows)]
    pub(crate) draws_sky: bool,
    /// It writes the title's reflections for the last time in a frame
    /// ([`crate::ambient::REFLECTED`]).
    #[cfg(windows)]
    pub(crate) ends_reflections: bool,
    #[cfg(windows)]
    translated: OnceLock<Option<Translated>>,
    /// Whether its code walks the ray-tracing structures, once looked at
    /// ([`Shader::walks`]).
    #[cfg(windows)]
    walks: OnceLock<bool>,
    /// The kinds of work only for rays it is, of those whose switch is on
    /// ([`RtWork`], [`rt_work_of`]; compute only).
    #[cfg(windows)]
    rt_work: u8,
    /// Which program of the title's card-memory defragmenter it is
    /// ([`defrag_class`]; compute only, looked for only while
    /// [`defrag_watched`]).
    #[cfg(windows)]
    defrag: u8,
    /// Cached [`of_builder`] flag so raw containers can be compressed safely.
    #[cfg(windows)]
    builder_spilled: OnceLock<bool>,
    /// Number of active PSO builds currently borrowing this shader's DXIL.
    #[cfg(windows)]
    building_dxil: std::sync::atomic::AtomicU32,
    /// Timestamp (`host::now_ms`) when `dxil` was last needed.
    #[cfg(windows)]
    last_dxil_ms: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for Shader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.stage, self.hash)
    }
}

/// Every container seen, by full content hash.
static SHADERS: Mutex<Option<HashMap<[u8; 32], Arc<Shader>>>> = Mutex::new(None);

/// For the memory lines: containers kept and their bytes, translations
/// kept and the bytes of their host code. None of them is ever let go.
static KEPT: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// What the shader table and the pipeline library keep in memory:
/// containers and their bytes, translations and their bytes, the bytes of
/// the library's file (kept whole while the library lives) and what its
/// last save wrote.
#[cfg(windows)]
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn kept() -> [u64; 7] {
    use std::sync::atomic::Ordering::Relaxed;
    [
        KEPT[0].load(Relaxed),
        KEPT[1].load(Relaxed),
        KEPT[2].load(Relaxed),
        KEPT[3].load(Relaxed),
        library::BLOB.load(Relaxed),
        library::SAVED.load(Relaxed),
        library::TRIMMED_BYTES.load(Relaxed),
    ]
}

/// The shared container for `bytes`.
fn shader(stage: &'static str, bytes: &[u8]) -> Arc<Shader> {
    let full = blake3::hash(bytes);
    let mut shaders = SHADERS.lock().unwrap_or_else(|e| e.into_inner());
    shaders
        .get_or_insert_with(HashMap::new)
        .entry(*full.as_bytes())
        .or_insert_with(|| {
            #[cfg(windows)]
            let ambient = crate::ambient::level().is_some();
            KEPT[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            KEPT[1].fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
            crate::ram_trim::COUNTERS
                .shader_container_orig_bytes
                .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
            crate::ram_trim::COUNTERS
                .shader_container_retained_bytes
                .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
            Arc::new(Shader {
                stage,
                hash: full.to_hex()[..16].to_owned(),
                container_len: bytes.len(),
                container: std::sync::Mutex::new(ContainerBlob::Raw(Arc::from(bytes))),
                // The programs the ambient light acts after, by name: looked
                // for only while it is on (`VICTORY_AMBIENT`, read once), so
                // that off it costs no walk of each new shader's container.
                #[cfg(windows)]
                resolves_light: ambient && crate::ambient::resolves(bytes),
                #[cfg(windows)]
                draws_sky: ambient
                    && bytes
                        .windows(crate::ambient::SKY.len())
                        .any(|w| w == crate::ambient::SKY),
                #[cfg(windows)]
                ends_reflections: ambient
                    && bytes
                        .windows(crate::ambient::REFLECTED.len())
                        .any(|w| w == crate::ambient::REFLECTED),
                #[cfg(windows)]
                translated: OnceLock::new(),
                #[cfg(windows)]
                walks: OnceLock::new(),
                // Looked for only while one of the switches is on (read
                // once); the census only in diagnostics builds.
                #[cfg(windows)]
                rt_work: if stage == "compute" {
                    #[cfg(feature = "diagnostics")]
                    rt_census(bytes);
                    rt_work_of(bytes)
                } else {
                    0
                },
                #[cfg(windows)]
                defrag: if stage == "compute" && defrag_watched() { defrag_class(bytes) } else { 0 },
                #[cfg(windows)]
                builder_spilled: OnceLock::new(),
                #[cfg(windows)]
                building_dxil: std::sync::atomic::AtomicU32::new(0),
                #[cfg(windows)]
                last_dxil_ms: std::sync::atomic::AtomicU64::new(0),
            })
        })
        .clone()
}

/// A translated shader and the console resources its code reads.
#[cfg(windows)]
pub(crate) struct Translated {
    pub(crate) host: victory_shader::emit::HostShader,
    pub(crate) used: Vec<victory_shader::UsedResource>,
    /// Whether the shader's code can store to buffer memory ([`stores`]);
    /// true when its code is not known.
    pub(crate) stores: bool,
    /// Whether it can store to title memory by address ([`stores_by_address`]):
    /// only then are the host's copies of that memory compared after it.
    pub(crate) address_stores: bool,
    /// Post-PSO reclaimable DXIL bytecode (`HostShader::dxil` moved here so
    /// cold shaders can drop DXIL RAM and reload on demand).
    #[cfg(windows)]
    dxil: std::sync::Mutex<Option<std::sync::Arc<[u8]>>>,
}

#[cfg(windows)]
pub(crate) struct DxilPin<'a>(&'a Shader);

#[cfg(windows)]
impl Drop for DxilPin<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::AcqRel;
        // The last build done with it: cold blobs let go at once if the
        // policy says so, and only with `VICTORY_RAM_TRIM=1`.
        if self.0.building_dxil.fetch_sub(1, AcqRel) == 1 && crate::ram_trim::aggressive() {
            let policy = crate::ram_trim::TrimPolicy::cached();
            if policy.shader_dxil_idle_ms == 0 {
                self.0.release_dxil();
            }
            if policy.shader_container_idle_ms == 0 {
                self.0.compress_container_if_ready();
            }
        }
    }
}

#[cfg(windows)]
impl Shader {
    /// Pins DXIL bytecode in RAM while a PSO creation call is using it.
    pub(crate) fn pin_dxil(&self) -> DxilPin<'_> {
        use std::sync::atomic::Ordering::{AcqRel, Relaxed};
        self.building_dxil.fetch_add(1, AcqRel);
        self.last_dxil_ms.store(crate::host::now_ms(), Relaxed);
        DxilPin(self)
    }

    fn with_container<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let bytes = self
            .container
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bytes_arc();
        f(&bytes)
    }

    /// Returns translated DXIL bytecode, reloading/re-translating transparently
    /// from the guest container / disk cache if previously dropped.
    pub(crate) fn dxil(&self) -> Option<std::sync::Arc<[u8]>> {
        use std::sync::atomic::Ordering::Relaxed;
        let t = self.translated()?;
        self.last_dxil_ms.store(crate::host::now_ms(), Relaxed);
        {
            let guard = t.dxil.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(bytes) = guard.as_ref() {
                return Some(bytes.clone());
            }
        }
        let reloaded = self.with_container(|bytes| crate::shader_cache::host_shader(bytes).ok())?;
        let arc: std::sync::Arc<[u8]> = std::sync::Arc::from(reloaded.dxil);
        let bytes_len = arc.len() as u64;
        let mut guard = t.dxil.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard.as_ref() {
            return Some(existing.clone());
        }
        *guard = Some(arc.clone());
        KEPT[3].fetch_add(bytes_len, Relaxed);
        crate::ram_trim::COUNTERS
            .shader_dxil_reloaded_bytes
            .fetch_add(bytes_len, Relaxed);
        Some(arc)
    }

    /// Drops retained post-PSO DXIL bytecode if no build currently pins it.
    pub(crate) fn release_dxil(&self) -> u64 {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        if self.building_dxil.load(Acquire) > 0 {
            return 0;
        }
        let Some(Some(t)) = self.translated.get() else {
            return 0;
        };
        let mut guard = t.dxil.lock().unwrap_or_else(|e| e.into_inner());
        if self.building_dxil.load(Acquire) > 0 {
            return 0;
        }
        let Some(dropped) = guard.take() else {
            return 0;
        };
        let len = dropped.len() as u64;
        drop(dropped);
        KEPT[3].fetch_sub(len, Relaxed);
        crate::ram_trim::COUNTERS
            .shader_dxil_dropped_bytes
            .fetch_add(len, Relaxed);
        len
    }

    fn compress_container_if_ready(&self) -> u64 {
        use std::sync::atomic::Ordering::Acquire;
        if self.translated.get().is_none() || self.building_dxil.load(Acquire) > 0 {
            return 0;
        }
        let Ok(mut guard) = self.container.try_lock() else {
            return 0;
        };
        guard.compress()
    }

    /// The translation, made on first use; a shader that does not
    /// translate is logged once.
    pub(crate) fn translated(&self) -> Option<&Translated> {
        use std::sync::atomic::Ordering::Relaxed;
        let out = self.translated
            .get_or_init(|| {
                let done = self.with_container(|bytes| {
                    let done = translate(bytes);
                    // `VICTORY_SHADER_SAVE=<dir>`: each shader as the title gave it (`.cdxil`) and as
                    // translated (`.dxil`), by the hash crash reports name it with
                    if let Some(dir) = std::env::var_os("VICTORY_SHADER_SAVE") {
                        let dir = std::path::PathBuf::from(dir);
                        let _ = std::fs::create_dir_all(&dir);
                        let stem = format!("{}.{}", self.hash, self.stage);
                        let _ = std::fs::write(dir.join(format!("{stem}.cdxil")), bytes);
                        if let Ok(t) = &done
                            && let Some(dxil) = t.dxil.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                        {
                            let _ = std::fs::write(dir.join(format!("{stem}.dxil")), &**dxil);
                        }
                    }
                    done
                });
                if let Ok(t) = &done {
                    let dxil_len = t
                        .dxil
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref()
                        .map_or(0, |d| d.len() as u64);
                    KEPT[2].fetch_add(1, Relaxed);
                    KEPT[3].fetch_add(dxil_len, Relaxed);
                }
                match &done {
                    Ok(t) => tracing::info!(
                        stage = self.stage,
                        hash = %self.hash,
                        user_data = t.host.user_data.len(),
                        samplers = t.host.samplers.len(),
                        "shader translated"
                    ),
                    Err(e) => tracing::warn!(
                        stage = self.stage,
                        hash = %self.hash,
                        error = %e,
                        "shader does not translate; its pipelines' draws are dropped"
                    ),
                }
                done.ok()
            })
            .as_ref();
        if out.is_some() {
            self.last_dxil_ms.store(crate::host::now_ms(), Relaxed);
        }
        out
    }
}

#[cfg(windows)]
fn translate(container: &[u8]) -> Result<Translated, String> {
    use victory_shader::{Container, HwStage};
    // The translation depends on it (a view index past a capped heap).
    crate::descriptor::host::probe_view_limit();
    let c = Container::parse(container).map_err(|e| e.to_string())?;
    let programs = match victory_shader::program_part(&c) {
        Some(part) => victory_shader::native_programs(part).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let host = crate::shader_cache::host_shader(container)?;
    let stores = programs.is_empty() || programs.iter().any(stores);
    let address_stores = programs.is_empty() || programs.iter().any(stores_by_address);
    let used = programs
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Ms | HwStage::Cs
            )
        })
        .map(|p| p.used)
        .unwrap_or_default();
    let mut host = host;
    let dxil = std::sync::Mutex::new(Some(std::sync::Arc::from(std::mem::take(&mut host.dxil))));
    Ok(Translated {
        host,
        used,
        stores,
        address_stores,
        dxil,
    })
}

/// The compute pipelines of the console's ray-tracing structure builder
/// (`XboxBvhBuilder.dll`; its programs are the ones with a spilled
/// user-data table) run **all or none**. Its stages feed each other
/// through memory: a stage that runs on what a dropped stage never wrote
/// computes its store offsets from garbage. The structures lie in plain
/// title memory, between the title's own objects, and a root view has no
/// size, so those stores land on the neighbours (run 98, with 14 of 35
/// stages translated: the title died within a second of the first builder
/// submission, on pointers that had become bounding-box bit patterns).
///
/// So a builder dispatch runs only while every builder pipeline the title
/// has created has a host pipeline; the first builder dispatch makes them
/// all on the spot instead of over the next frames (a stage "still being
/// made" would be a dropped stage). One that cannot be made turns the
/// builder off for the rest of the run.
///
/// shortcut: and only with `VICTORY_BVH_BUILDER=1`, until a run has shown
/// the whole builder and the host side of title memory by address
/// (`memory.rs`) working on a card; nothing reads the structures before
/// the programs that walk them translate.
#[cfg(windows)]
struct Builder {
    pipelines: Vec<Arc<make::Pending>>,
    /// Every one of `pipelines` has a host pipeline.
    whole: bool,
    /// One could not be made.
    off: bool,
}

#[cfg(windows)]
static BUILDER: Mutex<Builder> = Mutex::new(Builder {
    pipelines: Vec::new(),
    whole: false,
    off: false,
});

/// Whether `shaders` are a pipeline of the structure builder.
#[cfg(windows)]
fn of_builder(shaders: &[Arc<Shader>]) -> bool {
    let [shader] = shaders else { return false };
    if shader.stage != "compute" {
        return false;
    }
    #[cfg(windows)]
    {
        *shader.builder_spilled.get_or_init(|| {
            shader.with_container(|bytes| {
                use victory_shader::{Container, FourCC};
                let Ok(c) = Container::parse(bytes) else {
                    return false;
                };
                c.part(FourCC::DXIL)
                    .and_then(|part| victory_shader::native_programs(part.data).ok())
                    .is_some_and(|programs| programs.iter().any(victory_shader::spill::is_spilled))
            })
        })
    }
    #[cfg(not(windows))]
    {
        let _ = shader;
        false
    }
}

/// `VICTORY_BVH_QUARANTINE=1`: what the structure builder's stages store
/// stays in the host's copies and is not copied back into title memory. A
/// check for a first run of the whole builder: a stage that stores where
/// it should not can then not reach the title's own objects (run 98), and
/// the programs that walk the structures read the host's copies anyway.
/// shortcut: not a way to run: the title also reads results back (sizes,
/// counters) and gets none, and a copy's pages are overwritten from title
/// memory when the title writes them.
#[cfg(windows)]
pub(crate) fn builder_quarantined(this: This) -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    is_builder(this) && *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_QUARANTINE").is_some_and(|v| v == "1"))
}

#[cfg(windows)]
impl Shader {
    /// Whether the program walks the ray-tracing structures: its code has
    /// an `image_bvh_intersect_ray` or `image_bvh64_intersect_ray`. Looked
    /// at once, when a dispatch first asks.
    fn walks(&self) -> bool {
        *self
            .walks
            .get_or_init(|| self.with_container(container_walks))
    }
}

/// The thread-group size (`numthreads`) of the compute program of the
/// title's pipeline `this`; none if it is no compute pipeline or its
/// program does not parse.
#[cfg(windows)]
pub(crate) fn compute_threads(this: This) -> Option<[u32; 3]> {
    use victory_shader::native::StageIo;
    use victory_shader::{Container, FourCC};
    if this.is_null() {
        return None;
    }
    let p = pipeline(this);
    let [shader] = &p.shaders[..] else { return None };
    if shader.stage != "compute" {
        return None;
    }
    shader.with_container(|bytes| {
        let c = Container::parse(bytes).ok()?;
        let programs = victory_shader::native_programs(c.part(FourCC::DXIL)?.data).ok()?;
        programs.iter().find_map(|n| match &n.io {
            StageIo::Cs(cs) => Some(cs.threads),
            _ => None,
        })
    })
}

/// Whether a console container's code tests rays against the nodes of a
/// ray-tracing structure.
#[cfg(windows)]
fn container_walks(container: &[u8]) -> bool {
    use victory_shader::{Container, FourCC};
    let Ok(c) = Container::parse(container) else {
        return false;
    };
    c.part(FourCC::DXIL)
        .and_then(|part| victory_shader::native_programs(part.data).ok())
        .is_some_and(|programs| programs.iter().any(|p| code_walks(&p.code)))
}

/// Whether native code has a ray-tracing node test.
#[cfg(windows)]
fn code_walks(code: &[u8]) -> bool {
    victory_shader::gfx10::decode(code).is_ok_and(|code| code.iter().any(|i| i.name().is_some_and(|n| n.starts_with("image_bvh"))))
}

/// Dispatches held back by [`walk_held`] since the last frame line.
#[cfg(windows)]
pub(crate) static WALKS_HELD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a dispatch with the title's pipeline `this` is held back
/// because its program walks ray-tracing structures nothing has built
/// (`VICTORY_BVH_HOLD=1`; off by default).
///
/// The structure builder does not run (see [`Builder`]: only with
/// `VICTORY_BVH_BUILDER=1`, and then only whole, which a host of 32-lane
/// waves does not manage yet). The title's ray-traced passes run all the
/// same: 64 programs of its shader pack test rays against structure nodes
/// (ray-traced shadows, screen probes, reflections, the irradiance cache;
/// one of them, `CS_IrradianceCache_ShadowTrace_WithFar` f607780aa8e502d0,
/// from the first world frame on in every run). Each walks, in a loop the
/// title bounds at 65,535 steps a wave, whatever bytes lie where the
/// structure would be: what it returns is not a picture of the scene, and
/// how long it runs depends on those bytes. Upstream's pull request 2311
/// measured a card hung in exactly that dispatch on an NVIDIA card, every
/// run, and both card losses of ours since the draw windows' own fault was
/// fixed (runs 208 and 224) came within a second of the first use of the
/// programs that shade those rays' hits (`CS_RaytracingResolve`).
///
/// With the switch such a dispatch is dropped until the builder has run
/// whole (`alt`: only in an ON turn, [`crate::AltTurn`]: twenty seconds
/// without, twenty with, in turn, each change said in the log). What the passes would have written stays as the title left it
/// (their targets are not cleared by us): the picture has to be looked at.
#[cfg(windows)]
pub(crate) fn walk_held(this: This) -> bool {
    /// 0 off, 1 on, 2 in turn.
    static MODE: OnceLock<u8> = OnceLock::new();
    let mode = *MODE.get_or_init(|| match std::env::var("VICTORY_BVH_HOLD").as_deref() {
        Ok("1") => 1,
        Ok("alt") => 2,
        _ => 0,
    });
    if this.is_null() || mode == 0 {
        return false;
    }
    let p = pipeline(this);
    if !p.shaders.iter().any(|s| s.stage == "compute" && s.walks()) {
        return false;
    }
    // (Asked only for the few programs that walk: the lock is the builder's.)
    if BUILDER.lock().unwrap_or_else(|e| e.into_inner()).whole {
        return false;
    }
    if mode == 2 {
        static TURN: crate::AltTurn = crate::AltTurn::new();
        if !TURN.on(
            "dispatches of programs that walk ray-tracing structures nothing has built are held back",
            "VICTORY_BVH_HOLD",
            "off, run as usual",
        ) {
            return false;
        }
    }
    if WALKS_HELD.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::info!(
                shaders = ?p.shaders,
                "a dispatch of a program that walks ray-tracing structures is held back: the structure builder has not                  built them (VICTORY_BVH_HOLD=1); the first one"
            );
        });
    }
    true
}

/// Whether the container `bytes` holds `name` anywhere.
#[cfg(windows)]
fn holds(bytes: &[u8], name: &[u8]) -> bool {
    bytes.windows(name.len()).any(|w| w == name)
}

/// A kind of work the title does only for rays, which a switch can leave
/// out while the structure builder is off (without `VICTORY_BVH_BUILDER=1`),
/// each read once (`1` on, `alt` in turn, anything else off: the default).
/// A compute container is of the kind if it holds `name`. Off, no container
/// is looked at for it. `alt`: the pipelines are marked as with `1`, and a
/// dispatch is left out only in an ON turn of the kind's own
/// [`crate::AltTurn`] (twenty seconds without, twenty with, in turn, each
/// change said in the log). What the passes would have written stays as
/// the title left it: the picture has to be looked at.
///
/// - `VICTORY_RT_SKIN_SKIP`: the `bvh_skinning` programs, which pose every
///   animated mesh (and its motion) once more, only for the ray-tracing
///   structures the builder makes from what they write. Meant with
///   `VICTORY_BVH_NO_HIT=1` or `VICTORY_BVH_HOLD=1`, under which no ray
///   reads the structures either. From edsuuu/VI-BR-PORT ab4571df
///   (GPL-3.0): the largest share of the card's time in a cutscene there
///   (~6 ms a dispatch, hundreds a frame). Matched by `bvh_skinning` with
///   no stage after it, so the hair's (`CSGenerateHairVB`) is one of them
///   too; the picture's own skinning, which may share `ComputeSkinning` in
///   its names, is not matched by that alone.
/// - `VICTORY_RT_RESOLVE_SKIP`: `CS_RaytracingResolve*`, which shade what a
///   ray hit (some 1,000 programs).
/// - `VICTORY_ACTIVATE_RAYS_SKIP`: `CS_ScreenProbes_ActivateRays*`, which
///   pick the screen probes that cast rays.
/// - `VICTORY_HAIR_VB_SKIP`: `CSGenerateHairVB`, the hair's geometry made
///   for the structures (8 % of the card's time in the opening cutscene
///   there).
///
/// - `VICTORY_RT_TRACE_SKIP`: the tracing itself against the structures,
///   the screen probes' `CS_ScreenProbes_TraceRays*_BVH*` and the
///   reflections' `CS_RaytraceVBuffer*` (with the hair's geometry, 8 % of
///   the card's time in the opening cutscene there). Their `Fallback` and
///   `Voxels` forms march through the title's voxels, not the structures,
///   and light the picture: they are not of the kind. Meant with
///   `VICTORY_BVH_NO_HIT=1`, under which every ray misses anyway;
///   `VICTORY_BVH_HOLD` already holds back the programs whose code walks
///   the structures.
/// - `VICTORY_SKIP_RT_SHADOW`: the ray-traced sun and light shadows,
///   `CS_GenerateShadowMasks_Raytraced*`, `CS_RTShadowTrace*` and
///   `CS_RaytraceShadows*`, which trace the shadows of the screen tiles the
///   title marks for rays (the edges of the cascades' shadows, hair). With
///   every ray a miss those tiles come out lit: stair-stepped sun patches on
///   walls and clothes, hair glowing in the sun. From edsuuu/VI-BR-PORT
///   a04f6012 (GPL-3.0), opt-in there too.
///
/// The four before adapted from edsuuu/VI-BR-PORT f1e5e454 / 24f8d2ed
/// (GPL-3.0; from codec-d/cool_fox `make.rs`), where the tracing is left
/// out by default with every ray a miss (`VICTORY_RT_TRACE=1` runs it);
/// here, as every kind, only with its switch.
///
/// None of the kinds ever takes a program [`crate::ambient`] follows
/// ([`ambient_follows`]), whatever its name holds besides.
#[cfg(windows)]
struct RtWork {
    switch: &'static str,
    /// What a dispatch left out is, as the log says it (no program names).
    what: &'static str,
    /// Whether a compute container is of the kind, by the names it holds.
    is: fn(&[u8]) -> bool,
    turn: crate::AltTurn,
    /// Pipelines of the kind known so far.
    #[cfg(feature = "diagnostics")]
    pipelines: std::sync::atomic::AtomicU64,
    /// Dispatches since the last frame line: left out direct, left out
    /// indirect, run direct, run indirect (run: in the off turns of `alt`).
    #[cfg(feature = "diagnostics")]
    dispatches: [std::sync::atomic::AtomicU64; 4],
}

#[cfg(windows)]
impl RtWork {
    const fn new(switch: &'static str, what: &'static str, is: fn(&[u8]) -> bool) -> Self {
        Self {
            switch,
            what,
            is,
            turn: crate::AltTurn::new(),
            #[cfg(feature = "diagnostics")]
            pipelines: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "diagnostics")]
            dispatches: [const { std::sync::atomic::AtomicU64::new(0) }; 4],
        }
    }
}

/// The kinds ([`RtWork`]); bit `i` of [`Shader::rt_work`] and
/// [`Pipeline::rt_work`] is `RT_WORK[i]`.
#[cfg(windows)]
static RT_WORK: [RtWork; RT_KINDS] = [
    RtWork::new(
        "VICTORY_RT_SKIN_SKIP",
        "dispatches of skinning only for the ray-tracing structures are left out",
        |b| holds(b, b"bvh_skinning"),
    ),
    RtWork::new(
        "VICTORY_RT_RESOLVE_SKIP",
        "dispatches of programs that shade what rays hit are left out",
        |b| holds(b, b"CS_RaytracingResolve"),
    ),
    RtWork::new(
        "VICTORY_ACTIVATE_RAYS_SKIP",
        "dispatches of programs that pick the screen probes casting rays are left out",
        |b| holds(b, b"CS_ScreenProbes_ActivateRays"),
    ),
    RtWork::new(
        "VICTORY_HAIR_VB_SKIP",
        "dispatches of programs that make hair geometry for the ray-tracing structures are left out",
        |b| holds(b, b"CSGenerateHairVB"),
    ),
    RtWork::new(
        "VICTORY_RT_TRACE_SKIP",
        "dispatches of programs that trace rays through the ray-tracing structures are left out",
        |b| {
            (holds(b, b"CS_ScreenProbes_TraceRays") && holds(b, b"_BVH"))
                || (holds(b, b"CS_RaytraceVBuffer") && !holds(b, b"Voxels"))
        },
    ),
    RtWork::new(
        "VICTORY_SKIP_RT_SHADOW",
        "dispatches of programs that trace the shadows of the tiles marked for rays are left out",
        |b| holds(b, b"CS_GenerateShadowMasks_Raytraced") || holds(b, b"CS_RTShadowTrace") || holds(b, b"CS_RaytraceShadows"),
    ),
];

/// How many kinds of [`RtWork`] there are (bits of a `u8`).
#[cfg(windows)]
const RT_KINDS: usize = 6;

/// Each kind's switch ([`RtWork`]): 0 off (or the builder may run), 1 on,
/// 2 in turn.
#[cfg(windows)]
fn rt_skip_modes() -> [u8; RT_KINDS] {
    static MODES: OnceLock<[u8; RT_KINDS]> = OnceLock::new();
    *MODES.get_or_init(|| {
        if builder_allowed() {
            return [0; RT_KINDS];
        }
        RT_WORK.each_ref().map(|k| match std::env::var(k.switch).as_deref() {
            Ok("1") => 1,
            Ok("alt") => 2,
            _ => 0,
        })
    })
}

/// Whether [`crate::ambient`] follows the program in `bytes`: the stand-in
/// for indirect light hangs on `CS_ScreenProbes_TemporalResolve*`
/// ([`crate::ambient::resolves`]: its light image is replaced right after
/// it, in the dispatch hook these skips return before), and the
/// reflections' hold on `CS_UpdateHistory` ([`crate::ambient::REFLECTED`]);
/// the sky ([`crate::ambient::SKY`]) is a pixel program, taken out too in
/// case a container holds it. Looked at whether the ambient light is on or
/// not: those passes light the picture either way. No [`RtWork`] kind takes
/// such a program, so `VICTORY_RT_RESOLVE_SKIP` (whose `Resolve` is the rays'
/// hit shading, not the probes' temporal resolve) never leaves one out.
#[cfg(windows)]
fn ambient_follows(bytes: &[u8]) -> bool {
    crate::ambient::resolves(bytes) || holds(bytes, crate::ambient::REFLECTED) || holds(bytes, crate::ambient::SKY)
}

/// The kinds the compute container `bytes` is, of those whose switch is on
/// ([`rt_skip_modes`]); none for a program the ambient light follows.
#[cfg(windows)]
fn rt_work_of(bytes: &[u8]) -> u8 {
    let modes = rt_skip_modes();
    if modes == [0; RT_KINDS] || ambient_follows(bytes) {
        return 0;
    }
    RT_WORK
        .iter()
        .zip(modes)
        .enumerate()
        .filter(|(_, (k, mode))| *mode != 0 && (k.is)(bytes))
        .fold(0, |work, (i, _)| work | (1 << i))
}

/// Whether a dispatch with the title's pipeline `this` is left out because
/// it only serves rays nothing can use ([`RtWork`]): of a kind whose switch
/// is `1`, or `alt` in an ON turn of that kind.
#[cfg(windows)]
pub(crate) fn skips_rt_work(this: This, indirect: bool) -> bool {
    if this.is_null() {
        return false;
    }
    let work = pipeline(this).rt_work;
    if work == 0 {
        return false;
    }
    let modes = rt_skip_modes();
    let mut skip = false;
    for (i, (kind, mode)) in RT_WORK.iter().zip(modes).enumerate() {
        if work & (1 << i) == 0 || mode == 0 {
            continue;
        }
        let left = mode == 1 || kind.turn.on(kind.what, kind.switch, "off, run as usual");
        #[cfg(feature = "diagnostics")]
        kind.dispatches[usize::from(!left) * 2 + usize::from(indirect)].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        skip |= left;
    }
    #[cfg(not(feature = "diagnostics"))]
    let _ = indirect;
    skip
}

/// A count of compute containers by what their names hold, nothing left
/// out for it (diagnostics): `bvh_skinning`; `ComputeSkinning` without
/// `bvh` (the picture's own skinning, if it shares that name); and
/// `GenerateHairVB`. Each container once, when first seen.
#[cfg(all(windows, feature = "diagnostics"))]
static RT_CENSUS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// Counts the compute container `bytes` in [`RT_CENSUS`].
#[cfg(all(windows, feature = "diagnostics"))]
fn rt_census(bytes: &[u8]) {
    let kinds = [
        holds(bytes, b"bvh_skinning"),
        holds(bytes, b"ComputeSkinning") && !holds(bytes, b"bvh"),
        holds(bytes, b"GenerateHairVB"),
    ];
    for (count, is) in RT_CENSUS.iter().zip(kinds) {
        if is {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// The frame line's part of [`skips_rt_work`], per frame over `frames`: a
/// line for each kind whose switch is on, and the three numbers of
/// [`RT_CENSUS`], in its order, whenever they changed.
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn rt_work_line(frames: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    for (kind, mode) in RT_WORK.iter().zip(rt_skip_modes()) {
        let [left_direct, left_indirect, ran_direct, ran_indirect] =
            kind.dispatches.each_ref().map(|c| c.swap(0, Relaxed) as f64 / frames.max(1.0));
        if mode == 0 {
            continue;
        }
        tracing::info!(
            "ray-only dispatches per frame ({}): left out {left_direct:.1} direct, {left_indirect:.1} indirect; run {ran_direct:.1} direct, {ran_indirect:.1} indirect (the off turns of alt); {} such pipelines known",
            kind.switch,
            kind.pipelines.load(Relaxed)
        );
    }
    static SAID: Mutex<[u64; 3]> = Mutex::new([0; 3]);
    let census = RT_CENSUS.each_ref().map(|c| c.load(Relaxed));
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if census != *said {
        *said = census;
        let [a, b, c] = census;
        tracing::info!("ray census of compute containers (pipeline.rs RT_CENSUS): {a} {b} {c}");
    }
}

/// The title's card-memory defragmenter: compute programs whose container
/// holds [`DEFRAG_FAMILY`] in their names; the one that plans the moves
/// holds [`DEFRAG_PLANNER`]. Idea from the commit message of
/// voidopcode/vilocity 29111fec6 (no licence: no code taken): there the
/// planner took some 230 ms of the card's time a frame in some launches,
/// which ran at 3 to 5 fps with black faces, a dark bald head and red
/// jumpsuits; with its outputs cleared it planned no moves and those
/// launches were fast with the picture intact. Here faces come out black
/// or multicoloured, differently from session to session.
///
/// Diagnostics builds count its dispatches by class in the frame line
/// ([`defrag_line`]) and say, once per program, which unordered access
/// views its first dispatch writes through (`draw.rs` `defrag_report`);
/// nothing changes for it there. Its card time is in the per-pipeline
/// table of `VICTORY_CARD_METER_PIPELINES=1`, by name.
///
/// `VICTORY_SKIP_DEFRAG=1` (read once; anything else: off, the default)
/// leaves out every dispatch, direct or indirect, of the planner; the rest
/// of the family runs. What it would have written stays as it was: if the
/// title does not reset its plan before the planner runs, a plan of an
/// earlier frame may be carried out again (clearing its outputs, as there,
/// needs to know which of them is the plan: see [`skips_defrag`]). No
/// `alt`: what a turn changes in the title's heap outlasts it. The risks:
/// the title's management of its card memory changes; without
/// defragmenting, its card heap may fragment and run out of room in a long
/// session; resources it moved before stay where they went.
#[cfg(windows)]
const DEFRAG_FAMILY: &[u8] = b"DefragHeap";
/// The defragmenter's program that plans the moves ([`DEFRAG_FAMILY`]).
#[cfg(windows)]
const DEFRAG_PLANNER: &[u8] = b"DefragHeap_ComputeRelocations";

/// The defragmenter's classes as the log says them (no program names), in
/// the order of [`defrag_class`] from 1.
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) const DEFRAG_CLASSES: [&str; 2] = ["relocation planning", "rest of the family"];

/// Which program of the defragmenter the compute container `bytes` is: 1
/// the planner ([`DEFRAG_PLANNER`]), 2 another of [`DEFRAG_FAMILY`], 0
/// none.
#[cfg(windows)]
fn defrag_class(bytes: &[u8]) -> u8 {
    if holds(bytes, DEFRAG_PLANNER) {
        1
    } else if holds(bytes, DEFRAG_FAMILY) {
        2
    } else {
        0
    }
}

/// Whether a value of `VICTORY_SKIP_DEFRAG` turns it on: `1` only.
#[cfg(windows)]
fn skip_defrag_value(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// `VICTORY_SKIP_DEFRAG=1` ([`DEFRAG_FAMILY`]), read once and said in the
/// log then (with the first compute program the title makes).
#[cfg(windows)]
fn skip_defrag() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = skip_defrag_value(std::env::var("VICTORY_SKIP_DEFRAG").ok().as_deref());
        if on {
            tracing::info!(
                "VICTORY_SKIP_DEFRAG=1: dispatches of the relocation planning of the title's card-memory defragmenter are left out, direct and indirect"
            );
            tracing::warn!(
                "VICTORY_SKIP_DEFRAG=1 changes the title's management of its card memory (an experiment only): without defragmenting, its card heap may fragment and run out of room in a long session"
            );
        }
        on
    })
}

/// Whether compute containers are looked at for the defragmenter: in
/// diagnostics builds, to count it, and with its switch.
#[cfg(windows)]
fn defrag_watched() -> bool {
    cfg!(feature = "diagnostics") || skip_defrag()
}

/// Defragmenter pipelines known so far, by class ([`DEFRAG_CLASSES`]).
#[cfg(all(windows, feature = "diagnostics"))]
static DEFRAG_PIPELINES: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Defragmenter dispatches since the last frame line, by class: run
/// direct, run indirect, left out.
#[cfg(all(windows, feature = "diagnostics"))]
static DEFRAG_DISPATCHES: [[std::sync::atomic::AtomicU64; 3]; 2] =
    [const { [const { std::sync::atomic::AtomicU64::new(0) }; 3] }; 2];

/// Which program of the defragmenter the title's pipeline `this` is
/// ([`defrag_class`]): 0 none.
#[cfg(windows)]
pub(crate) fn defrag_class_of(this: This) -> u8 {
    if this.is_null() { 0 } else { pipeline(this).defrag }
}

/// Whether a dispatch with the title's pipeline `this` is left out as the
/// defragmenter's planner under `VICTORY_SKIP_DEFRAG=1`; counted by class
/// either way (diagnostics).
///
/// Not cleared instead: zeroing the planner's outputs (so that the title
/// reads "no moves planned") needs to know which of the views it writes
/// through is the plan and how far each reaches. A root view is an address
/// with no size; a table's view names its range, but whether the program
/// writes only the plan there (and not, say, the heap's own allocation
/// records, whose zeroing would lose every allocation) is in its code, not
/// looked at here. `draw.rs` `defrag_report` says what it binds, for that
/// choice.
#[cfg(windows)]
pub(crate) fn skips_defrag(this: This, indirect: bool) -> bool {
    let class = defrag_class_of(this);
    if class == 0 {
        return false;
    }
    let left = class == 1 && skip_defrag();
    #[cfg(feature = "diagnostics")]
    DEFRAG_DISPATCHES[usize::from(class - 1)][if left { 2 } else { usize::from(indirect) }]
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "diagnostics"))]
    let _ = indirect;
    left
}

/// The frame line's part of [`skips_defrag`], per frame over `frames`,
/// once a program of the defragmenter is known.
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn defrag_line(frames: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let [planner, rest] =
        DEFRAG_DISPATCHES.each_ref().map(|c| c.each_ref().map(|n| n.swap(0, Relaxed) as f64 / frames.max(1.0)));
    let [planners, others] = DEFRAG_PIPELINES.each_ref().map(|n| n.load(Relaxed));
    if planners + others == 0 {
        return;
    }
    tracing::info!(
        "the title's card-memory defragmenter, dispatches per frame: relocation planning run {:.1} direct, {:.1} indirect, left out {:.1} (VICTORY_SKIP_DEFRAG); rest of the family run {:.1} direct, {:.1} indirect; pipelines known: {planners} relocation planning, {others} rest",
        planner[0],
        planner[1],
        planner[2],
        rest[0],
        rest[1]
    );
}

/// Whether the title's pipeline `this` is one of the structure builder's.
#[cfg(windows)]
pub(crate) fn is_builder(this: This) -> bool {
    !this.is_null() && pipeline(this).builder
}

/// A stage of the builder was dropped after all: none runs from here on.
#[cfg(windows)]
pub(crate) fn builder_off(why: &'static str) {
    let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if !b.off {
        b.off = true;
        tracing::warn!(why, "the ray-tracing structure builder is turned off for the rest of the run: its stages run all or none");
    }
}

/// `VICTORY_BVH_BUILDER=1`: the builder may run (see [`Builder`]).
#[cfg(windows)]
fn builder_allowed() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| std::env::var_os("VICTORY_BVH_BUILDER").is_some_and(|v| v == "1"))
}

/// At a dispatch of a builder pipeline: whether the builder runs (see
/// [`Builder`]).
#[cfg(windows)]
pub(crate) fn builder_runs() -> bool {
    if !builder_allowed() {
        return false;
    }
    let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    if b.off {
        return false;
    }
    if b.whole {
        return true;
    }
    // How many were not made yet, and how long making them here took.
    #[cfg(feature = "diagnostics")]
    let (unmade, started) = (
        b.pipelines.iter().filter(|p| !make::made(p)).count(),
        std::time::Instant::now(),
    );
    let missing = b.pipelines.iter().filter(|p| !make::now(p)).count();
    #[cfg(feature = "diagnostics")]
    tracing::info!(
        unmade,
        of = b.pipelines.len(),
        ms = started.elapsed().as_millis() as u64,
        "the ray-tracing structure builder's first dispatch: pipelines not made yet, made here"
    );
    if missing > 0 {
        b.off = true;
        for p in b.pipelines.iter().filter(|p| !make::now(p)) {
            tracing::warn!(shaders = ?make::shaders(p), "a builder pipeline with no host pipeline");
        }
        tracing::warn!(
            missing,
            of = b.pipelines.len(),
            "the ray-tracing structure builder stays off: not all of its programs have a host pipeline, and its stages run all or none"
        );
        return false;
    }
    tracing::info!(
        pipelines = b.pipelines.len(),
        "the ray-tracing structure builder runs: all of its programs have host pipelines"
    );
    b.whole = true;
    true
}

/// Whether a console program can store to buffer memory: it has a buffer,
/// typed-buffer, scalar-memory, flat or global store or atomic, or code
/// that is not all known (an instruction of those kinds outside our table,
/// an indirect jump, code that does not decode). Stores to textures
/// (image instructions) and to the program's own scratch memory do not
/// count: neither reaches a buffer's host copy. After upstream #413,
/// which leaves scalar-memory stores out and counts image stores.
#[cfg(windows)]
fn stores(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::gfx10::{Encoding, decode};
    let Ok(code) = decode(&program.code) else {
        return true;
    };
    code.iter().any(|i| {
        let name = i.name();
        match i.encoding {
            Encoding::Mubuf
            | Encoding::Mtbuf
            | Encoding::Smem
            | Encoding::Flat
            | Encoding::Global => name.is_none_or(|n| {
                (n.contains("store") || n.contains("atomic")) && !n.contains("scratch")
            }),
            Encoding::Sop1 => name.is_none_or(|n| matches!(n, "s_setpc_b64" | "s_swappc_b64")),
            _ => false,
        }
    })
}

/// Whether a console program can store to title memory by address
/// (`crate::memory`): a flat, global or scalar-memory store or atomic (the
/// accesses that take a raw address), or code that is not all known, as in
/// [`stores`]. The programs that walk the ray-tracing structures read them
/// by address and store their results through views: before this, each of
/// their dispatches declared every numbered copy of title memory written,
/// which with the structure builder running was 12 GB snapshotted and
/// compared on the CPU a frame (3030 ranges of 4 MiB).
#[cfg(windows)]
fn stores_by_address(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::gfx10::{Encoding, decode};
    let Ok(code) = decode(&program.code) else {
        return true;
    };
    code.iter().any(|i| {
        let name = i.name();
        match i.encoding {
            Encoding::Smem | Encoding::Flat | Encoding::Global => name.is_none_or(|n| {
                (n.contains("store") || n.contains("atomic")) && !n.contains("scratch")
            }),
            Encoding::Sop1 => name.is_none_or(|n| matches!(n, "s_setpc_b64" | "s_swappc_b64")),
            _ => false,
        }
    })
}

#[repr(C)]
struct Pipeline {
    object: Object,
    packet: [u32; 8],
    /// Each used stage's container.
    shaders: Vec<Arc<Shader>>,
    /// Whether a list has bound it yet (its shaders are logged then).
    bound: std::sync::atomic::AtomicBool,
    /// What its host pipeline is made from, and the host pipeline once made
    /// (by a worker: the title may release the pipeline meanwhile).
    #[cfg(windows)]
    host: Arc<make::Pending>,
    /// One of the ray-tracing structure builder's ([`Builder`]).
    #[cfg(windows)]
    builder: bool,
    /// The kinds of work only for rays its compute stage is, whose
    /// dispatches can be left out ([`skips_rt_work`]): bit `i` is
    /// [`RT_WORK`]`[i]`.
    #[cfg(windows)]
    rt_work: u8,
    /// Which program of the title's card-memory defragmenter its compute
    /// stage is ([`defrag_class`]): 0 none.
    #[cfg(windows)]
    defrag: u8,
}
const _: () = assert!(offset_of!(Pipeline, packet) == 0x10);

/// Stage names, in description order.
const GRAPHICS_STAGES: [&str; 5] = ["vertex", "pixel", "domain", "hull", "geometry"];

fn pipeline<'a>(this: This) -> &'a Pipeline {
    // SAFETY: the title passes one of our pipelines.
    unsafe { &*this.cast::<Pipeline>() }
}

/// List method: logs each pipeline's shaders the first time a list binds
/// it, so a run lists the shaders its frames need first.
pub(crate) fn bound(this: This) {
    if this.is_null() {
        return;
    }
    let p = pipeline(this);
    if p.bound.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let frame = crate::frame::current();
    for shader in &p.shaders {
        tracing::info!(pipeline = ?this, frame, stage = shader.stage, size = shader.container_len, hash = %shader.hash, "first bind");
    }
}

/// The host pipeline of a title pipeline; `None` if one of its shaders does
/// not translate, the host refuses it, or it is still being made (its first
/// use starts that, see [`make`]).
#[cfg(windows)]
pub(crate) fn host_pipeline<'a>(this: This) -> Option<&'a host::HostPipeline> {
    if this.is_null() {
        return None;
    }
    make::ready(&pipeline(this).host)
}

/// As [`host_pipeline`], for a draw into render target and depth views of
/// the formats `bound`: if a bound view has another format than the title's
/// pipeline declares for its slot (`host::formats_differ`), the host
/// pipeline of the same shaders and state made for the bound formats
/// (`make::for_formats`). A draw whose views are as declared pays one
/// comparison of the nine words.
#[cfg(windows)]
pub(crate) fn host_pipeline_for<'a>(this: This, bound: &host::TargetFormats) -> Option<&'a host::HostPipeline> {
    if this.is_null() {
        return None;
    }
    let p = &pipeline(this).host;
    if p.formats != *bound && host::formats_differ(&p.formats, bound) {
        return make::for_formats(p, bound);
    }
    make::ready(p)
}

/// Host pipelines queued and not made yet.
#[cfg(windows)]
pub(crate) fn making() -> u64 {
    make::waiting()
}

/// Draws and dispatches dropped since the last [`take_drops`], by reason
/// (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
static DROPS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Shader hashes as printed in the pipeline diagnostics.
#[cfg(all(windows, feature = "diagnostics"))]
fn parse_skip_hashes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
        .filter_map(|hash| {
            if hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Some(hash.to_ascii_lowercase())
            } else {
                tracing::warn!(
                    hash,
                    "VICTORY_SKIP_PIPELINES expects 16-digit shader hashes"
                );
                None
            }
        })
        .collect()
}

#[cfg(all(windows, feature = "diagnostics"))]
fn skipped_shader(p: &Pipeline) -> Option<&str> {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    let hashes = HASHES.get_or_init(|| {
        let value = std::env::var("VICTORY_SKIP_PIPELINES").unwrap_or_default();
        parse_skip_hashes(&value)
    });
    if hashes.is_empty() {
        return None;
    }
    p.shaders
        .iter()
        .find(|shader| hashes.contains(&shader.hash))
        .map(|shader| shader.hash.as_str())
}

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn is_skipped(this: This) -> bool {
    !this.is_null() && skipped_shader(pipeline(this)).is_some()
}

/// Counts a draw or dispatch of `this` intentionally skipped or dropped
/// for want of a host pipeline (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_drop(this: This) {
    let why = if this.is_null() {
        Some("no pipeline set".to_owned())
    } else {
        let p = pipeline(this);
        skipped_shader(p)
            .map(|hash| format!("skipped shader {hash}"))
            .or_else(|| make::missing(&p.host))
    };
    if let Some(why) = why {
        note_why(why);
    }
}

/// Counts a draw or dispatch dropped for `why` (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_why(why: String) {
    crate::capture::op(0, || format!("DROPPED {why}"));
    let mut drops = DROPS.lock().unwrap_or_else(|e| e.into_inner());
    *drops.get_or_insert_with(HashMap::new).entry(why).or_default() += 1;
}

/// The drops counted since the last call, most first (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn take_drops() -> Vec<(String, u64)> {
    let drops = DROPS.lock().unwrap_or_else(|e| e.into_inner()).take();
    let mut drops: Vec<(String, u64)> = drops.into_iter().flatten().collect();
    // Frame reports only show the first 40 reasons. Keep ties ordered so
    // a HashMap iteration order change cannot change which reasons appear.
    drops.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    drops
}

/// The stage's container, if it has one.
fn copy(stage: &'static str, code: &Bytecode) -> Option<Arc<Shader>> {
    if code.code.is_null() || code.size == 0 {
        return None;
    }
    // SAFETY: the title passes `size` bytes of shader code.
    Some(shader(stage, unsafe {
        std::slice::from_raw_parts(code.code, code.size)
    }))
}

fn hand_out(
    shaders: Vec<Arc<Shader>>,
    #[cfg(windows)] graphics: Option<Box<host::GraphicsState>>,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    tracing::trace!(?shaders, "pipeline state");
    #[cfg(windows)]
    let host = Arc::new(make::Pending::new(shaders.clone(), graphics));
    #[cfg(windows)]
    make::early(&host);
    #[cfg(windows)]
    let builder = of_builder(&shaders);
    #[cfg(windows)]
    if builder {
        // Another stage: the group is looked at again at its next dispatch.
        let mut b = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
        b.pipelines.push(host.clone());
        b.whole = false;
        // Named once each (gfx41): which programs are the builder's, to
        // match against the refused and untranslated ones.
        tracing::info!(n = b.pipelines.len(), ?shaders, "a pipeline of the ray-tracing structure builder");
        // Made ahead of the pipelines draws ask for, long before the
        // first builder dispatch, which needs all of them at once.
        if builder_allowed() {
            make::ahead(&host);
        }
    }
    let pipeline = Box::new(Pipeline {
        object: Object::new(&FUNCTIONS),
        packet: [0; 8],
        #[cfg(windows)]
        host,
        #[cfg(windows)]
        builder,
        #[cfg(windows)]
        rt_work: {
            let work = shaders.iter().fold(0, |work, s| work | s.rt_work);
            #[cfg(feature = "diagnostics")]
            for (i, kind) in RT_WORK.iter().enumerate() {
                if work & (1 << i) != 0 {
                    let n = kind.pipelines.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let hash = shaders.iter().find(|s| s.stage == "compute").map_or("", |s| s.hash.as_str());
                    tracing::info!(shader = hash, switch = kind.switch, so_far = n, "a program of work only for rays known: its dispatches can be left out");
                }
            }
            work
        },
        #[cfg(windows)]
        defrag: {
            let class = shaders.iter().map(|s| s.defrag).max().unwrap_or(0);
            #[cfg(feature = "diagnostics")]
            if class != 0 {
                let n = DEFRAG_PIPELINES[usize::from(class - 1)].fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let hash = shaders.iter().find(|s| s.stage == "compute").map_or("", |s| s.hash.as_str());
                tracing::info!(
                    shader = hash,
                    class = DEFRAG_CLASSES[usize::from(class - 1)],
                    so_far = n,
                    "a program of the title's card-memory defragmenter known"
                );
            }
            class
        },
        shaders,
        bound: std::sync::atomic::AtomicBool::new(false),
    });
    object::hand_out(PIPELINE, pipeline, iid, &PIPELINE_IIDS, out)
}

/// Device method.
pub(crate) extern "system" fn create_graphics(
    _: This,
    desc: *const GraphicsHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    let stages = GRAPHICS_STAGES.iter().zip(&desc.stages);
    let shaders = stages.filter_map(|(s, c)| copy(s, c)).collect();
    hand_out(
        shaders,
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method.
pub(crate) extern "system" fn create_compute(
    _: This,
    desc: *const ComputeHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    hand_out(
        copy("compute", &desc.stage).into_iter().collect(),
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method 0x1f0: the plain compute description plus console
/// extensions `(desc, extension count, extensions, iid, out)`.
/// shortcut: the extensions (console shader settings) are ignored, as is
/// the rest of the description.
pub(crate) extern "system" fn create_compute_x(
    this: This,
    desc: *const ComputeHead,
    _extensions: u32,
    _extended: *const u8,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    create_compute(this, desc, iid, out)
}

/// A pipeline described as a stream: its size and the subobjects, each a
/// type then its data, padded to 8 bytes.
#[repr(C)]
pub(crate) struct StreamDesc {
    size: usize,
    stream: *const u8,
}

/// A subobject type's size (type included), and the stage it is if it is a
/// shader. The console numbers amplification and mesh shaders 23 and 24
/// (PC: 24 and 25); its own types from 25 on are not sized yet.
fn subobject(kind: u32) -> Option<(usize, Option<&'static str>)> {
    let stage = |name| Some((24, Some(name)));
    match kind {
        1 => stage("vertex"),
        2 => stage("pixel"),
        3 => stage("domain"),
        4 => stage("hull"),
        5 => stage("geometry"),
        6 => stage("compute"),
        23 => stage("amplification"),
        24 => stage("mesh"),
        0 => Some((16, None)),
        12 | 19 => Some((24, None)),
        7 | 15 => Some((40, None)),
        8 => Some((336, None)),
        10 => Some((48, None)),
        11 => Some((56, None)),
        17 => Some((16, None)),
        21 => Some((64, None)),
        22 => Some((32, None)),
        9 | 13 | 14 | 16 | 18 | 20 => Some((8, None)),
        _ => None,
    }
}

/// Device method: a pipeline from a stream; the shaders are kept, and the
/// graphics state for the host pipeline.
pub(crate) extern "system" fn create_stream(
    _: This,
    desc: *const StreamDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.stream.is_null() {
        return E_INVALIDARG;
    }
    let mut shaders = Vec::new();
    #[cfg(windows)]
    let mut graphics = host::GraphicsState::default();
    let mut at = 0;
    while at + 8 <= desc.size {
        // SAFETY: inside the stream the title passes.
        let kind = unsafe { desc.stream.add(at).cast::<u32>().read_unaligned() };
        let Some((size, stage)) = subobject(kind) else {
            tracing::warn!(
                kind,
                at,
                "pipeline subobject of unknown size; the rest is ignored"
            );
            break;
        };
        if let Some(stage) = stage {
            // SAFETY: a shader subobject has its bytecode at +8.
            let code = unsafe { &*desc.stream.add(at + 8).cast::<Bytecode>() };
            shaders.extend(copy(stage, code));
        }
        #[cfg(windows)]
        // SAFETY: a whole subobject of this kind lies at `at`.
        unsafe {
            graphics.read(kind, desc.stream.add(at))
        };
        at += size;
    }
    // The whole state is read: integer targets do not blend on the host
    // (`GraphicsState::integer_targets_do_not_blend`).
    #[cfg(windows)]
    if graphics.integer_targets_do_not_blend() {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            tracing::info!(
                "a pipeline of the title's blends into a render target of integers: blending is off for that target on the host (said once)"
            );
        });
    }
    hand_out(
        shaders,
        #[cfg(windows)]
        Some(Box::new(graphics)),
        iid,
        out,
    )
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = object::is(iid, &PIPELINE_IIDS).then_some(this);
    object::answer(PIPELINE, iid, found, out)
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Pipeline>(this)
}

/// IUnknown 3, object 4, device child 1, pipeline 3.
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<PIPELINE, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    Table(f)
};

#[cfg(windows)]
pub(crate) mod host;
#[cfg(windows)]
mod library;

/// Host pipelines created and loaded from the library since the start
/// (`crate::framelog`).
#[cfg(windows)]
pub(crate) fn frame_log_counts() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (library::MADE.load(Relaxed), library::LOADED.load(Relaxed))
}
#[cfg(windows)]
mod make;

/// Lets the pipeline library's mapped file go from memory when no pipeline
/// has loaded for a while (`library::trim`); called by the host's sweep.
#[cfg(windows)]
pub(crate) fn trim_library() {
    library::trim();
}

/// Releases cold post-PSO shader DXIL blobs and zlib-compresses idle guest
/// containers (`SHADERS`), keeping full hash/container reloadability on demand.
#[cfg(windows)]
pub(crate) fn trim_shaders_with(policy: &crate::ram_trim::TrimPolicy, now: u64) {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    let list: Vec<Arc<Shader>> = {
        let guard = SHADERS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(map) = guard.as_ref() else {
            return;
        };
        map.values().cloned().collect()
    };
    for shader in list {
        if shader.translated.get().is_none() || shader.building_dxil.load(Acquire) > 0 {
            continue;
        }
        let idle = now.saturating_sub(shader.last_dxil_ms.load(Relaxed));
        if policy.panic_memory || idle >= policy.shader_dxil_idle_ms {
            shader.release_dxil();
        }
        if policy.panic_memory || idle >= policy.shader_container_idle_ms {
            shader.compress_container_if_ready();
        }
    }
}

/// The pipeline library's part of the report when the host device is
/// removed (`library::removal_note`).
#[cfg(windows)]
pub(crate) fn library_removal_note() -> String {
    library::removal_note()
}

/// Keeps compiled host pipelines, and the list of title pipelines to start
/// early, in `dir` (a folder of the cache) across runs.
#[cfg(windows)]
pub fn use_pipeline_cache(dir: std::path::PathBuf) {
    library::use_pipeline_cache(dir);
}

/// Saves the pipeline cache now if anything was added (at the end of a run).
#[cfg(windows)]
pub fn save_pipeline_cache() {
    library::save(true);
}

#[cfg(test)]
mod tests {
    #[cfg(all(windows, feature = "diagnostics"))]
    #[test]
    fn skip_hashes_accepts_full_hashes_only() {
        assert!(super::parse_skip_hashes(" , ").is_empty());
        assert_eq!(
            super::parse_skip_hashes(" 0123456789ABCDEF, 0123456789abcde, xyz, 0011223344556677,"),
            ["0123456789abcdef", "0011223344556677"]
        );
    }

    use super::*;
    use crate::object::{S_OK, slot};

    /// The kinds of work only for rays are known by the names in their
    /// container; the picture's own skinning and the programs the ambient
    /// light follows are none of them. (Every switch is off in tests: the
    /// names are matched as [`rt_work_of`] does with all of them on.)
    #[cfg(windows)]
    #[test]
    fn ray_work_programs_are_known_by_name() {
        let kinds = |bytes: &[u8]| -> u8 {
            if ambient_follows(bytes) {
                return 0;
            }
            RT_WORK.iter().enumerate().filter(|(_, k)| (k.is)(bytes)).fold(0, |w, (i, _)| w | (1 << i))
        };
        assert_eq!(kinds(b"..\0bvh_skinningCSComputeSkinningRigidVelocity_1c34db0a_Wrapped\0.."), 0b0001);
        assert_eq!(kinds(b"bvh_skinningCS"), 0b0001);
        assert_eq!(kinds(b"bvh_skinning_smh"), 0b0001);
        assert_eq!(kinds(b"..\0CSComputeSkinning_0_Wrapped\0.."), 0);
        assert_eq!(kinds(b"PS_PedSkin_0001"), 0);
        assert_eq!(kinds(b"..\0CS_RaytracingResolve_Opaque_12345678_Wrapped\0.."), 0b0010);
        assert_eq!(kinds(b"..\0CS_ScreenProbes_ActivateRays_0_Wrapped\0.."), 0b0100);
        assert_eq!(kinds(b"bvh_skinning_smh\0CSGenerateHairVB_c88a8f19_Wrapped\0.."), 0b1001);
        assert_eq!(kinds(b"..\0CS_ScreenProbes_TemporalResolve_Wave64\0.."), 0);
        assert_eq!(kinds(b"..\0CS_ScreenProbes_TraceRays_RayBinning_BVH_Mixed\0.."), 0b1_0000);
        assert_eq!(kinds(b"..\0CS_ScreenProbes_TraceRays_Voxels\0.."), 0);
        assert_eq!(kinds(b"..\0CS_ScreenProbes_TraceRays_Fallback\0.."), 0);
        assert_eq!(kinds(b"..\0CS_RaytraceVBuffer_ShaderBinning_WithFar\0.."), 0b1_0000);
        assert_eq!(kinds(b"..\0CS_RaytraceVBuffer_Voxels_RayBinning\0.."), 0);
        assert_eq!(kinds(b"..\0CS_GenerateShadowMasks_Raytraced_Hair\0.."), 0b10_0000);
        assert_eq!(kinds(b"..\0CS_RTShadowTrace_0\0.."), 0b10_0000);
        assert_eq!(kinds(b"..\0CS_RaytraceShadows_Local\0.."), 0b10_0000);
        assert_eq!(kinds(b"..\0CS_GenerateShadowMasks_Cascades\0.."), 0);
        assert_eq!(kinds(b"CS_RaytracingResolve\0CS_ScreenProbes_TemporalResolve\0"), 0);
        assert_eq!(kinds(b"CS_RaytracingResolve\0CS_UpdateHistory\0"), 0);
        assert_eq!(kinds(b""), 0);
    }

    /// The defragmenter's programs are known by the names in their
    /// container, the planner apart; only `1` turns its switch on.
    #[cfg(windows)]
    #[test]
    fn defrag_programs_are_known_by_name() {
        assert_eq!(defrag_class(b"..\0CS_DefragHeap_ComputeRelocations_0_Wrapped\0.."), 1);
        assert_eq!(defrag_class(b"DefragHeap_ComputeRelocations"), 1);
        assert_eq!(defrag_class(b"..\0CS_DefragHeap_Move_Wrapped\0.."), 2);
        assert_eq!(defrag_class(b"DefragHeap_ComputeRelocation"), 2);
        assert_eq!(defrag_class(b"CS_Defrag\0Heap_ComputeRelocations"), 0);
        assert_eq!(defrag_class(b"defragheap_computerelocations"), 0);
        assert_eq!(defrag_class(b""), 0);
        assert!(skip_defrag_value(Some("1")));
        assert!(skip_defrag_value(Some(" 1 ")));
        for off in [None, Some(""), Some("0"), Some("alt"), Some("skip"), Some("clear"), Some("true"), Some("11")] {
            assert!(!skip_defrag_value(off), "{off:?}");
        }
    }

    /// Which stores give title-memory copies as written: a vector store
    /// through a buffer descriptor does not; flat, global and scalar-memory
    /// stores do, the scalar buffer forms too (kept conservative: their
    /// descriptor may alias title memory). Each case starts with an
    /// `s_load_dword` by address, which stores nothing.
    #[cfg(windows)]
    #[test]
    fn only_stores_that_can_take_an_address_count_as_address_stores() {
        use victory_shader::native::{ComputeIo, HwStage, NativeProgram, StageIo};
        let program = |store: [u32; 2]| {
            let words = [0xF400_0202, 0xFA00_0000, store[0], store[1], 0xBF81_0000];
            NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 8,
                layout: vec![],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            }
        };
        for (store, name, by_address) in [
            ([0xE070_1000, 0x8001_0100], "buffer_store_dword", false),
            ([0xF440_0202, 0xFA00_0000], "s_store_dword", true),
            ([0xF460_0202, 0xFA00_0000], "s_buffer_store_dword", true),
            ([0xDC70_8000, 0x007D_0200], "global_store_dword", true),
            ([0xF400_0202, 0xFA00_0000], "s_load_dword", false),
        ] {
            let p = program(store);
            let code = victory_shader::gfx10::decode(&p.code).unwrap();
            assert_eq!(code[1].name(), Some(name));
            assert!(stores(&p) || !by_address, "{name}");
            assert_eq!(stores_by_address(&p), by_address, "{name}");
        }
    }

    fn shaders(p: This) -> Vec<(&'static str, Vec<u8>)> {
        pipeline(p)
            .shaders
            .iter()
            .map(|s| {
                (
                    s.stage,
                    s.container
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .to_vec(),
                )
            })
            .collect()
    }

    #[test]
    fn pipelines_keep_their_shaders() {
        let (vs, ps) = ([1u8, 2, 3], [4u8; 5]);
        let none = || Bytecode {
            code: std::ptr::null(),
            size: 0,
        };
        let desc = GraphicsHead {
            root: std::ptr::null(),
            stages: [
                Bytecode {
                    code: vs.as_ptr(),
                    size: vs.len(),
                },
                Bytecode {
                    code: ps.as_ptr(),
                    size: ps.len(),
                },
                none(),
                none(),
                none(),
            ],
        };
        let mut p = std::ptr::null();
        assert_eq!(
            create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p),
            S_OK
        );
        assert_eq!(
            shaders(p),
            [("vertex", vs.to_vec()), ("pixel", ps.to_vec())]
        );
        // A second pipeline with the same pixel shader shares its container.
        let mut q = std::ptr::null();
        create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut q);
        assert!(Arc::ptr_eq(
            &pipeline(p).shaders[1],
            &pipeline(q).shaders[1]
        ));
        assert_eq!(slot::<extern "system" fn(This) -> u32>(p, 0x10)(p), 0);
    }

    /// The stream layout the title builds (title+0x86f090): root
    /// signature, five shader slots (the first a mesh shader), blend, sample
    /// mask, rasterizer, depth-stencil 1, sample description, input layout,
    /// topology, render target and depth formats: 0x2b0 bytes.
    #[test]
    fn streams_yield_their_shaders() {
        let (ms, ps) = ([9u8; 7], [4u8; 5]);
        let mut stream = vec![0u8; 0x2b0];
        let mut put = |at: usize, v: u64| stream[at..at + 8].copy_from_slice(&v.to_le_bytes());
        for (at, kind) in [
            (0x10, 24),
            (0x28, 2),
            (0x40, 3),
            (0x58, 4),
            (0x70, 5),
            (0x88, 8),
            (0x1d8, 9),
            (0x1e0, 10),
            (0x210, 21),
            (0x250, 17),
            (0x260, 12),
            (0x278, 14),
            (0x280, 15),
            (0x2a8, 16),
        ] {
            put(at, kind);
        }
        put(0x18, ms.as_ptr() as u64);
        put(0x20, ms.len() as u64);
        put(0x30, ps.as_ptr() as u64);
        put(0x38, ps.len() as u64);
        // Topology type 3 (triangles), one render target of format 28.
        put(0x278, 14 | 3 << 32);
        put(0x280, 15 | 28 << 32);
        put(0x2a0, 1 << 32);
        let desc = StreamDesc {
            size: stream.len(),
            stream: stream.as_ptr(),
        };
        let mut p = std::ptr::null();
        let hr = create_stream(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p);
        assert_eq!(hr, S_OK);
        assert_eq!(shaders(p), [("mesh", ms.to_vec()), ("pixel", ps.to_vec())]);
        #[cfg(windows)]
        {
            let g = pipeline(p).host.graphics.as_deref().unwrap();
            assert_eq!(g.topology.0, 3);
            assert_eq!(
                (g.targets.NumRenderTargets, g.targets.RTFormats[0].0),
                (1, 28)
            );
        }
    }
}

#[cfg(all(test, windows))]
mod ram_trim_tests {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    fn minimal_stage_container(slot_offset: usize, extra_padding: usize) -> Vec<u8> {
        // Minimal Xbox `DXIL` part carrying one valid `s_endpgm` native program at `slot_offset`.
        const ABSENT: u64 = 0xDEAD_BEEF_DEAD_BEEF;
        const SLOT_CS: usize = 0x60;
        const RECORD_SIZE: usize = 0x54;
        const STAGE_DATA: usize = 0x14c + 8 * (4 + 10 * 10 * 4);
        let code = 0xbf81_0000u32.to_le_bytes(); // s_endpgm
        let mut payload = 6u64.to_le_bytes().to_vec();
        payload.resize(SLOT_CS + 8, 0);
        for at in [0x08usize, 0x10, 0x18, 0x20, 0x28, 0x30, 0x48, SLOT_CS] {
            payload[at..at + 8].copy_from_slice(&ABSENT.to_le_bytes());
        }
        let record = payload.len() as u64;
        payload[slot_offset..slot_offset + 8].copy_from_slice(&record.to_le_bytes());
        let code_at = record + RECORD_SIZE as u64 + STAGE_DATA as u64;
        payload.extend_from_slice(&code_at.to_le_bytes());
        payload.extend_from_slice(&(code.len() as u64).to_le_bytes());
        payload.resize(record as usize + 0x50, 0);
        payload.extend_from_slice(&(1u32).to_le_bytes()); // 1 VGPR
        let mut stage_data = vec![0u8; STAGE_DATA];
        if slot_offset == SLOT_CS {
            // CS thread group (1, 1, 1) at `STAGE_DATA + 0x28`.
            for (i, v) in [1u32, 1, 1].into_iter().enumerate() {
                stage_data[0x28 + i * 4..0x28 + (i + 1) * 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        payload.extend_from_slice(&stage_data);
        payload.extend_from_slice(&code);

        let mut fssm = b"FSSM".to_vec();
        fssm.extend_from_slice(&0x101u32.to_le_bytes());
        fssm.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        fssm.extend_from_slice(&payload);

        let mut buf = vec![0u8; zlib_rs::compress_bound(fssm.len())];
        let (stream, rc) =
            zlib_rs::compress_slice(&mut buf, &fssm, zlib_rs::DeflateConfig::new(6));
        assert_eq!(rc, zlib_rs::ReturnCode::Ok);
        let mut xbch = b"XBCH".to_vec();
        xbch.extend_from_slice(&3u32.to_le_bytes());
        xbch.extend_from_slice(&(stream.len() as u64).to_le_bytes());
        xbch.extend_from_slice(&(fssm.len() as u64).to_le_bytes());
        xbch.extend_from_slice(stream);

        let pad = vec![0x42u8; extra_padding];
        let blobs: Vec<(u32, &[u8])> = if extra_padding > 0 {
            vec![(0x0a, b"xbop"), (0x0d, &xbch), (0x12, &pad)]
        } else {
            vec![(0x0a, b"xbop"), (0x0d, &xbch)]
        };
        let mut dxil_part = vec![0u8; 0x18];
        dxil_part.extend_from_slice(b"XDAT");
        dxil_part.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
        dxil_part.extend_from_slice(&0x24u32.to_le_bytes());
        let mut off = dxil_part.len() + 12 * blobs.len();
        for &(kind, blob) in &blobs {
            for w in [kind, off as u32, blob.len() as u32] {
                dxil_part.extend_from_slice(&w.to_le_bytes());
            }
            off += blob.len();
        }
        for (_, blob) in blobs {
            dxil_part.extend_from_slice(blob);
        }
        let dxbc_parts = [(victory_shader::FourCC::DXIL, dxil_part.as_slice())];
        let table = 32 + 4 * dxbc_parts.len();
        let total = table + dxbc_parts.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"DXBC");
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(dxbc_parts.len() as u32).to_le_bytes());
        let mut offset = table;
        for (_, d) in &dxbc_parts {
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += 8 + d.len();
        }
        for (fourcc, d) in dxbc_parts {
            out.extend_from_slice(&fourcc.0);
            out.extend_from_slice(&(d.len() as u32).to_le_bytes());
            out.extend_from_slice(d);
        }
        out
    }

    #[test]
    fn real_shader_drops_dxil_post_pso_compresses_and_reloads_on_demand() {
        let orig = minimal_stage_container(0x60, 64 << 10);
        assert!(orig.len() > (64 << 10));
        let shader = super::shader("compute", &orig);

        let t = shader.translated().expect("minimal CS translates");
        let _ = t;
        assert!(!shader.walks());
        assert!(!super::of_builder(std::slice::from_ref(&shader)));

        let dxil_first = shader.dxil().expect("DXIL available");
        let dxil_len = dxil_first.len() as u64;
        assert!(dxil_len > 0);
        drop(dxil_first);

        // Pinning DXIL during PSO creation prevents release.
        let pin = shader.pin_dxil();
        assert_eq!(shader.release_dxil(), 0, "pinned DXIL must stay in RAM");
        drop(pin);

        let kept_before = KEPT[3].load(Relaxed);
        let dropped_before = crate::ram_trim::COUNTERS
            .shader_dxil_dropped_bytes
            .load(Relaxed);
        assert_eq!(shader.release_dxil(), dxil_len);
        assert!(KEPT[3].load(Relaxed) <= kept_before.saturating_sub(dxil_len));
        assert!(
            crate::ram_trim::COUNTERS
                .shader_dxil_dropped_bytes
                .load(Relaxed)
                >= dropped_before + dxil_len
        );

        // Cold container compression shrinks RAM significantly and roundtrips losslessly.
        let saved = shader.compress_container_if_ready();
        assert!(saved > (60 << 10), "expected >60 KiB saved, got {saved}");
        shader.with_container(|decoded| assert_eq!(decoded, orig.as_slice()));

        // A second PSO later requesting DXIL transparently reloads identical bytes.
        let reloaded_before = crate::ram_trim::COUNTERS
            .shader_dxil_reloaded_bytes
            .load(Relaxed);
        let dxil_again = shader.dxil().expect("DXIL reloads after drop + compression");
        assert_eq!(dxil_again.len() as u64, dxil_len);
        assert!(
            crate::ram_trim::COUNTERS
                .shader_dxil_reloaded_bytes
                .load(Relaxed)
                >= reloaded_before + dxil_len
        );
        drop(dxil_again);
        let _ = shader.release_dxil();
    }

    #[test]
    fn graphics_shader_compresses_and_reloads_dxil_without_walks_or_builder() {
        let orig = minimal_stage_container(0x30, 64 << 10);
        let shader = super::shader("pixel", &orig);

        // Translate only — never call `walks()` or `of_builder()`, matching default runs.
        assert!(shader.translated().is_some(), "minimal PS translates");
        assert!(shader.walks.get().is_none());
        assert!(shader.builder_spilled.get().is_none());

        let dxil_len = shader.dxil().expect("PS DXIL available").len() as u64;
        assert!(dxil_len > 0);
        assert_eq!(shader.release_dxil(), dxil_len);

        let saved = shader.compress_container_if_ready();
        assert!(saved > (60 << 10), "graphics shader container must compress (saved={saved})");
        let dxil_reloaded = shader.dxil().expect("PS DXIL reloads from compressed container");
        assert_eq!(dxil_reloaded.len() as u64, dxil_len);
        drop(dxil_reloaded);
        let _ = shader.release_dxil();
    }
}
