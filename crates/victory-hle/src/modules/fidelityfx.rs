//! `amd_fidelityfx_loader_xbox.dll` served by the host's PC FidelityFX
//! upscaler on the host GPU, when the user names the folder that holds AMD's
//! `amd_fidelityfx_upscaler_dx12.dll` (`VICTORY_FIDELITYFX_DIR`). Off by
//! default: without the variable, or when that DLL cannot serve, this
//! module is not registered at all and the title's own loader DLL runs as
//! before (its upscale dispatch stretched, [`super::ffx`]).
//!
//! Both builds have the same API (the FidelityFX SDK's, MIT): five functions
//! over chains of descriptions, each starting with a type id and the next
//! one's address; contexts are opaque pointers. The console-specific parts
//! are translated: the backend description names the console device (type
//! 0x0100_0002; on PC type 2 with the host device), and a dispatch names the
//! title's command list and resources (their host list and host copies, in
//! the states the title gives, see `victory_gpu::external`). The rest passes
//! through unchanged: sizes, flags, jitter, motion-vector scale, camera
//! values, the resource descriptions, the context. Only the upscaler: other
//! effects (frame generation) are refused. An upscaling the host cannot
//! record is stretched instead, as without the bridge.
//!
//! When it sees calls: the title calls this DLL only on the profile whose
//! upscaler picker chooses FidelityFX (7), which the Series S profile does
//! by itself; no console check is made here. `VICTORY_OWN_UPSCALER=1` (and
//! the diagnostics switch `VICTORY_UPSCALER=6`) make the title pick its own
//! upscaler instead: with them the bridge is registered but never called.
//! `VICTORY_RENDER_SCALE` changes the render size the title passes in each
//! dispatch; the bridge takes it as given, within the context's maximum.
//!
//! The DLL is never shipped or fetched: the user supplies AMD's signed
//! build (the SDK's `Kits/FidelityFX/signedbin`). Its Authenticode
//! signature is not checked here (nor upstream); its path and file version
//! are logged.
//!
//! Ported from Belobug/VIctory-Belobug-AMD cd8dfc06 and 2a8eed9b
//! (crates/victory-hle/src/modules/fidelityfx.rs, GPL-3.0; the bridge from
//! DexNom/VIctory ab34a8b2), without its Series X 4K title patch.
#![allow(non_snake_case)]
// Title code calls these through its import table.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::BTreeMap;
use std::ffi::{CStr, OsStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{HleModule, hle_export};

/// The variable naming the folder of the PC upscaler.
pub const DIR_ENV: &str = "VICTORY_FIDELITYFX_DIR";

/// The DLL whose imports are served here.
pub const LOADER: &str = "amd_fidelityfx_loader_xbox.dll";

/// The PC build's upscaler, which has the five functions itself.
const DLL: &str = "amd_fidelityfx_upscaler_dx12.dll";

// Return codes.
const OK: u32 = 0;
const UNKNOWN_TYPE: u32 = 2;
const NO_PROVIDER: u32 = 4;
const PARAMETER: u32 = 6;

// Description types.
const CREATE_UPSCALE: u64 = 0x1_0000;
const DISPATCH_UPSCALE: u64 = 0x1_0001;
const CONSOLE_BACKEND: u64 = 0x100_0002;
const HOST_BACKEND: u64 = 2;
const GET_VERSIONS: u64 = 4;
const OVERRIDE_VERSION: u64 = 5;
const GET_PROVIDER_VERSION: u64 = 6;
const MEMORY_USAGE_WITH_DEVICE: u64 = 0x1_0009;
const GET_RENDER_RESOLUTION: u64 = 0x1_0003;
const DISPLAY_RESOLUTION_MOTION_VECTORS: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct Header {
    kind: u64,
    next: *mut Header,
}

/// An upscaler context: flags, largest render and output sizes, a message
/// callback.
#[repr(C)]
#[derive(Clone, Copy)]
struct CreateUpscale {
    header: Header,
    flags: u32,
    max_render: [u32; 2],
    max_output: [u32; 2],
    message: usize,
}

/// The backend description: a device.
#[repr(C)]
struct Backend {
    header: Header,
    device: *mut c_void,
}

/// A resource, its description (type, format, sizes, mips, flags, usage)
/// and the state it is in.
#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: [u32; 8],
    state: u32,
}

/// One upscaling: the command list, then colour, depth, motion vectors,
/// exposure, reactive mask, transparency mask and output; then jitter,
/// motion-vector scale, sizes, sharpening, frame time, pre-exposure, reset,
/// camera and flags, which pass through.
#[repr(C)]
#[derive(Clone, Copy)]
struct DispatchUpscale {
    header: Header,
    list: *mut c_void,
    resources: [Resource; 7],
    jitter: [f32; 2],
    motion_scale: [f32; 2],
    render_size: [u32; 2],
    upscale_size: [u32; 2],
    settings: [u32; 10],
}

const _: () = assert!(size_of::<CreateUpscale>() == 0x30);
const _: () = assert!(size_of::<Resource>() == 0x30);
const _: () = assert!(size_of::<DispatchUpscale>() == 0x1b0);

/// The versions a backend offers: count in and out, ids, names.
#[repr(C)]
#[derive(Clone, Copy)]
struct GetVersions {
    header: Header,
    create_kind: u64,
    device: *mut c_void,
    count: *mut u64,
    ids: *mut u64,
    names: *mut *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryUsageWithDevice {
    header: Header,
    device: *mut c_void,
    max_render: [u32; 2],
    max_output: [u32; 2],
    flags: u32,
    usage: *mut c_void,
}

#[repr(C)]
struct ProviderVersion {
    header: Header,
    id: u64,
    name: *const c_char,
}

#[repr(C)]
struct OverrideVersion {
    header: Header,
    id: u64,
}

type Create = unsafe extern "C" fn(*mut *mut c_void, *mut Header, *const c_void) -> u32;
type Destroy = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;
/// Configure, query and dispatch.
type Call = unsafe extern "C" fn(*mut *mut c_void, *mut Header) -> u32;

struct Host {
    _library: Library,
    create: Create,
    destroy: Destroy,
    configure: Call,
    query: Call,
    dispatch: Call,
    /// The host `ID3D12Device`, which lives for the run.
    device: *mut c_void,
    provider_id: u64,
    /// The provider's version as the SDK names it (`4.1.2`, `3.1.5`).
    provider_name: String,
}

struct Library(windows_sys::Win32::Foundation::HMODULE);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: this owns the reference acquired by LoadLibraryExW.
        unsafe {
            windows_sys::Win32::Foundation::FreeLibrary(self.0);
        }
    }
}

// SAFETY: D3D12 devices are free-threaded; the rest is code.
unsafe impl Send for Host {}
// SAFETY: as above.
unsafe impl Sync for Host {}

static HOST: OnceLock<Host> = OnceLock::new();

struct Context {
    // Serializes recording and destruction for this particular temporal history.
    live: Mutex<bool>,
    /// A destroy has begun ([`ffxDestroyContext`]): a dispatch recorded later
    /// ([`dispatch_later`]) does not wait for `live`, which the destroy holds
    /// while it waits for the submitted lists, maybe this dispatch's.
    ending: std::sync::atomic::AtomicBool,
    first_dispatch: std::sync::Once,
    // The FFX API requires pointers in the creation descriptor to remain
    // live through destroy, including our replacement backend.
    creation: CreateChain,
}

struct CreateChain {
    description: Box<CreateUpscale>,
    _backend: Box<Backend>,
    _version: Box<OverrideVersion>,
}

// SAFETY: after Create, the boxed descriptors are retained immutably.
// The device is held by the host; the remaining extension pointers belong
// to the title under the FFX context lifetime contract.
unsafe impl Send for CreateChain {}
// SAFETY: as above, no thread mutates the retained descriptors.
unsafe impl Sync for CreateChain {}

static CONTEXTS: Mutex<BTreeMap<usize, Arc<Context>>> = Mutex::new(BTreeMap::new());

fn fits(extent: [u32; 2], maximum: [u32; 2]) -> bool {
    extent[0] != 0 && extent[1] != 0 && extent[0] <= maximum[0] && extent[1] <= maximum[1]
}

fn dispatch_output(
    render: [u32; 2],
    output: [u32; 2],
    max_render: [u32; 2],
    max_output: [u32; 2],
) -> Option<[u32; 2]> {
    let output = if output == [0, 0] { max_output } else { output };
    (fits(render, max_render) && fits(output, max_output) && fits(render, output)).then_some(output)
}

fn minimum_extent(index: usize, flags: u32, render: [u32; 2], output: [u32; 2]) -> [u32; 2] {
    match index {
        6 => output,
        2 if flags & DISPLAY_RESOLUTION_MOTION_VECTORS != 0 => output,
        3 => [1, 1], // Optional exposure texture.
        _ => render,
    }
}

macro_rules! once {
    ($level:ident, $($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::$level!($($arg)*));
    }};
}

/// The folder a value of [`DIR_ENV`] names; `None` when it is unset or
/// blank. Surrounding quotes and spaces are dropped.
fn host_dir(value: Option<&OsStr>) -> Option<PathBuf> {
    let value = value?;
    match value.to_str() {
        Some(text) => {
            let text = text.trim().trim_matches('"').trim();
            (!text.is_empty()).then(|| PathBuf::from(text))
        }
        None => (!value.is_empty()).then(|| PathBuf::from(value)),
    }
}

/// Serves the title's FidelityFX with the PC upscaler when [`DIR_ENV`]
/// names a folder that holds a usable one. Call before anything binds an
/// import (the import registry is built once, with this module in it only
/// if this served). Unset: nothing is read, loaded or logged.
pub fn init_from_env() -> bool {
    let Some(dir) = host_dir(std::env::var_os(DIR_ENV).as_deref()) else {
        return false;
    };
    match use_host_dir(&dir) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                folder = %dir.display(),
                error,
                "FidelityFX: {DIR_ENV} is set but the PC upscaler is not used; the title's own FidelityFX DLLs run as without it"
            );
            false
        }
    }
}

/// Select the PC provider before the title's import registry is initialized.
pub fn use_host_dir(dir: &Path) -> Result<(), String> {
    let (host, version) = load(dir)?;
    HOST.set(host)
        .map_err(|_| String::from("FidelityFX provider already initialized"))?;
    if !is_fsr_4_1_2(&version) {
        tracing::warn!(
            version,
            "FidelityFX: the SDK offers this adapter no FSR 4.1.2; FSR 3.1 is used in its place (its picture differs from 4's)"
        );
    }
    tracing::info!(folder = %dir.display(), version, "FidelityFX: selected PC upscaler");
    Ok(())
}

/// Whether the host's FidelityFX serves the title's.
pub fn serving() -> bool {
    HOST.get().is_some()
}

/// The provider serving, as the SDK names it (`4.1.2`, `3.1.6`).
pub fn provider() -> Option<&'static str> {
    HOST.get().map(|h| h.provider_name.as_str())
}

/// A file version's four parts from its high and low words.
fn version_text(high: u32, low: u32) -> String {
    format!("{}.{}.{}.{}", high >> 16, high & 0xffff, low >> 16, low & 0xffff)
}

/// The file version of the DLL at `path`, from its version resource.
fn file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut handle = 0u32;
    // SAFETY: a NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u32; (size as usize).div_ceil(4)];
    // SAFETY: a buffer of at least `size` bytes.
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root = [u16::from(b'\\'), 0];
    let mut fixed = null_mut::<c_void>();
    let mut len = 0u32;
    // SAFETY: the version block just read; its root is VS_FIXEDFILEINFO.
    let found =
        unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut fixed, &mut len) };
    if found == 0 || fixed.is_null() || (len as usize) < 16 {
        return None;
    }
    // SAFETY: VS_FIXEDFILEINFO inside `data`: signature, structure
    // version, then the file version's high and low words.
    let words = unsafe { fixed.cast::<[u32; 4]>().read_unaligned() };
    (words[0] == 0xfeef_04bd).then(|| version_text(words[2], words[3]))
}

fn load(dir: &Path) -> Result<(Host, String), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    };
    // The search flags below want a full path.
    let path = std::path::absolute(dir.join(DLL)).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {DLL} at {}", path.display()));
    }
    // Not checked: its signature. What it is, said plainly.
    tracing::warn!(
        path = %path.display(),
        file_version = file_version(&path).as_deref().unwrap_or("none"),
        "FidelityFX: loading the PC upscaler; its Authenticode signature is not verified here (use AMD's signed build from the SDK's signedbin folder)"
    );
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // Its own folder and System32 only, never the current folder.
    let search = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    // SAFETY: a NUL-terminated path to the user's FidelityFX build.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), null_mut(), search) };
    if module.is_null() {
        return Err(format!(
            "{} did not load ({})",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let library = Library(module);
    let find = |name: &CStr| {
        // SAFETY: a loaded module and a NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }
            .map(|f| f as *const c_void)
            .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))
    };
    let device = victory_gpu::external::host_device().ok_or("there is no host GPU")?;
    // SAFETY: the SDK's ffxQuery export, with its declared signature.
    let query = unsafe { std::mem::transmute::<*const c_void, Call>(find(c"ffxQuery")?) };
    let (provider_id, version) = select_version(query, device)?;
    // SAFETY: the API's exports, with its signatures.
    let host = unsafe {
        Host {
            create: std::mem::transmute::<*const c_void, Create>(find(c"ffxCreateContext")?),
            destroy: std::mem::transmute::<*const c_void, Destroy>(find(c"ffxDestroyContext")?),
            configure: std::mem::transmute::<*const c_void, Call>(find(c"ffxConfigure")?),
            query,
            dispatch: std::mem::transmute::<*const c_void, Call>(find(c"ffxDispatch")?),
            _library: library,
            device,
            provider_id,
            provider_name: version.clone(),
        }
    };
    Ok((host, version))
}

fn is_fsr_4_1_2(name: &str) -> bool {
    name.strip_prefix("4.1.2")
        .is_some_and(|suffix| !suffix.starts_with(|c: char| c.is_ascii_digit()))
}

/// The SDK supplies the version id to put in the context creation chain.
fn select_version(query_fn: Call, device: *mut c_void) -> Result<(u64, String), String> {
    let mut count = 0u64;
    let mut query = GetVersions {
        header: Header {
            kind: GET_VERSIONS,
            next: null_mut(),
        },
        create_kind: CREATE_UPSCALE,
        device,
        count: &mut count,
        ids: null_mut(),
        names: null_mut(),
    };
    // SAFETY: a version query naming the host device.
    let code = unsafe { query_fn(null_mut(), &mut query.header) };
    if code != OK || count == 0 {
        return Err(format!("it offers no upscaler for the host GPU (code {code})"));
    }
    let mut names = vec![null::<c_char>(); count as usize];
    let mut ids = vec![0u64; names.len()];
    query.names = names.as_mut_ptr();
    query.ids = ids.as_mut_ptr();
    // SAFETY: as above, with room for `count` names.
    let code = unsafe { query_fn(null_mut(), &mut query.header) };
    if code != OK {
        return Err(format!("upscaler version names could not be queried (code {code})"));
    }
    let mut offered = Vec::new();
    for (id, name) in ids.into_iter().zip(names).take(count as usize) {
        if !name.is_null() {
            // SAFETY: NUL-terminated name owned by the SDK.
            let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
            offered.push((id, name));
        }
    }
    choose(offered)
}

/// The patch number of an FSR 3.1 provider's name (`3.1.5` → 5); `None` for
/// any other version (`3.10.0`, `2.3.4`, `4.1.2`).
fn fsr_3_1_patch(name: &str) -> Option<u32> {
    let patch = name.strip_prefix("3.1.")?;
    let digits = patch.len() - patch.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    patch[..digits].parse().ok()
}

/// The provider to ask for, of those the SDK offers for the adapter: FSR
/// 4.1.2 where there is one, otherwise the newest FSR 3.1 (which runs on any
/// D3D12 card; AMD offers 4 on few). With neither, what was offered is said.
fn choose(offered: Vec<(u64, String)>) -> Result<(u64, String), String> {
    if let Some(at) = offered.iter().position(|(_, name)| is_fsr_4_1_2(name)) {
        return Ok(offered.into_iter().nth(at).unwrap());
    }
    let newest = offered
        .iter()
        .enumerate()
        .filter_map(|(at, (_, name))| fsr_3_1_patch(name).map(|patch| (patch, at)))
        .max_by_key(|&(patch, _)| patch);
    match newest {
        Some((_, at)) => Ok(offered.into_iter().nth(at).unwrap()),
        None => {
            let names: Vec<&str> = offered.iter().map(|(_, name)| name.as_str()).collect();
            Err(format!(
                "the SDK offers this adapter neither FSR 4.1.2 nor FSR 3.1 (offered: {})",
                if names.is_empty() { "nothing".into() } else { names.join(", ") }
            ))
        }
    }
}

/// Only the upscaler is created on the host.
fn creates(kind: u64) -> bool {
    kind == CREATE_UPSCALE
}

/// Only upscaling is dispatched on the host.
fn dispatches(kind: u64) -> bool {
    kind == DISPATCH_UPSCALE
}

/// Puts the host backend after the title's upscaler description, in place
/// of the console's backend that follows it (where the title puts it);
/// anywhere else the console's is left for the host DLL to ignore, ours
/// coming first.
fn link(upscale: &mut CreateUpscale, backend: &mut Backend) {
    let after = upscale.header.next;
    // SAFETY: the title's chain.
    backend.header.next = match unsafe { after.as_ref() } {
        Some(h) if h.kind == CONSOLE_BACKEND => h.next,
        _ => after,
    };
    backend.header.kind = HOST_BACKEND;
    upscale.header.next = &raw mut backend.header;
}

/// A FidelityFX resource state as the host resource state it means (as the
/// PC backend reads it); `None` for generic read and indirect arguments,
/// which a host texture cannot be in.
fn host_state(state: u32) -> Option<u32> {
    Some(match state {
        0x1 | 0x80 => 0,  // common, present
        0x2 => 0x8,       // unordered access
        0x4 => 0x40,      // compute read
        0x8 => 0x80,      // pixel read
        0xc => 0xc0,      // both
        0x10 => 0x800,    // copy source
        0x20 => 0x400,    // copy destination
        0x100 => 0x4,     // render target
        0x200 => 0x10,    // depth write
        _ => return None, // generic read, indirect arguments
    })
}

extern "C" fn ffxCreateContext(
    context: *mut *mut c_void,
    desc: *mut Header,
    memory: *const c_void,
) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if context.is_null() || desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a chain of descriptions.
    let kind = unsafe { (*desc).kind };
    if !creates(kind) {
        once!(
            warn,
            kind = format_args!("{kind:#x}"),
            "FidelityFX: only the upscaler runs on the host; another effect was refused"
        );
        return NO_PROVIDER;
    }
    // SAFETY: an upscaler description, by its type.
    let mut upscale = Box::new(unsafe { desc.cast::<CreateUpscale>().read() });
    let mut backend = Box::new(Backend {
        header: Header {
            kind: HOST_BACKEND,
            next: null_mut(),
        },
        device: host.device,
    });
    link(&mut upscale, &mut backend);
    let mut requested = Box::new(OverrideVersion {
        header: Header {
            kind: OVERRIDE_VERSION,
            next: backend.header.next,
        },
        id: host.provider_id,
    });
    backend.header.next = &raw mut requested.header;
    // SAFETY: a PC chain; the title's later descriptions live for the call.
    let code = unsafe { (host.create)(context, &mut upscale.header, memory) };
    if code != OK {
        tracing::error!(code, render = ?upscale.max_render, output = ?upscale.max_output, flags = format_args!("{:#x}", upscale.flags), "FidelityFX: the host refused the title's upscaler");
        return code;
    }
    let mut version = ProviderVersion {
        header: Header {
            kind: GET_PROVIDER_VERSION,
            next: null_mut(),
        },
        id: 0,
        name: null(),
    };
    // SAFETY: a provider query on the context just made.
    let query_code = unsafe { (host.query)(context, &mut version.header) };
    // SAFETY: a NUL-terminated name the DLL keeps, if any.
    let name = (!version.name.is_null())
        .then(|| unsafe { CStr::from_ptr(version.name) }.to_string_lossy());
    if query_code != OK
        || version.id != host.provider_id
        || name.as_deref() != Some(host.provider_name.as_str())
    {
        tracing::warn!(
            query_code,
            provider = name.as_deref().unwrap_or("?"),
            requested = host.provider_name,
            "FidelityFX created a different provider than the one requested"
        );
        // SAFETY: the PC provider just created the context with this allocator.
        unsafe { (host.destroy)(context, memory) };
        return NO_PROVIDER;
    }
    tracing::info!(
        provider = name.as_deref().unwrap_or("?"),
        id = format_args!("{:#x}", version.id),
        render = ?upscale.max_render,
        output = ?upscale.max_output,
        flags = format_args!("{:#x}", upscale.flags),
        "FidelityFX: upscaler context on the host"
    );
    // SAFETY: a successful SDK creation wrote an opaque context into this slot.
    let handle = unsafe { *context } as usize;
    CONTEXTS.lock().unwrap_or_else(|e| e.into_inner()).insert(
        handle,
        Arc::new(Context {
            live: Mutex::new(true),
            ending: std::sync::atomic::AtomicBool::new(false),
            first_dispatch: std::sync::Once::new(),
            creation: CreateChain {
                description: upscale,
                _backend: backend,
                _version: requested,
            },
        }),
    );
    OK
}

extern "C" fn ffxDestroyContext(context: *mut *mut c_void, memory: *const c_void) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if context.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes its context. Capture its value before destroy clears it.
    let handle = unsafe { *context } as usize;
    let ctx = CONTEXTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .cloned();
    let Some(ctx) = ctx else { return PARAMETER };
    ctx.ending.store(true, std::sync::atomic::Ordering::Release);
    let mut live = ctx.live.lock().unwrap_or_else(|e| e.into_inner());
    if !*live {
        return PARAMETER;
    }
    // The guest releases a context after submitting its frames. The host
    // may still be executing their lists, even after guest Execute returns.
    victory_gpu::external::finish_submitted();
    // SAFETY: a context the PC provider made, with the title's allocator.
    let code = unsafe { (host.destroy)(context, memory) };
    if code == OK {
        *live = false;
        let mut contexts = CONTEXTS.lock().unwrap_or_else(|e| e.into_inner());
        // A concurrent Create may already have reused the released SDK address.
        if contexts
            .get(&handle)
            .is_some_and(|current| Arc::ptr_eq(current, &ctx))
        {
            contexts.remove(&handle);
        }
    } else {
        ctx.ending.store(false, std::sync::atomic::Ordering::Release);
    }
    code
}

/// Settings (key-value pairs, debug messages) name nothing console-specific.
extern "C" fn ffxConfigure(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    // SAFETY: the title passes its context and descriptions.
    HOST.get()
        .map_or(NO_PROVIDER, |h| unsafe { (h.configure)(context, desc) })
}

extern "C" fn ffxQuery(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a description.
    let kind = unsafe { (*desc).kind };
    // Substitute the native device; the SDK returns values through the
    // title's original output pointers in both request structures.
    if kind == GET_VERSIONS {
        // SAFETY: a version query, by its type.
        let mut query = unsafe { *desc.cast::<GetVersions>() };
        query.device = host.device;
        // SAFETY: the same query naming the host device.
        return unsafe { (host.query)(context, &mut query.header) };
    }
    if kind == MEMORY_USAGE_WITH_DEVICE {
        // SAFETY: a memory query, by its type.
        let mut query = unsafe { *desc.cast::<MemoryUsageWithDevice>() };
        query.device = host.device;
        // SAFETY: the same query naming the host device.
        return unsafe { (host.query)(context, &mut query.header) };
    }
    // SAFETY: as above.
    let code = unsafe { (host.query)(context, desc) };
    if kind == GET_RENDER_RESOLUTION && code != OK {
        tracing::warn!(code, "FidelityFX provider refused the render-resolution query");
    }
    code
}

/// An upscaling that did not reach the provider: the colour input
/// stretched to the output, as without the bridge.
fn stretch_instead(title: &DispatchUpscale, code: u32) -> u32 {
    let stretched = victory_gpu::list::upscale_stretch(
        title.list,
        title.resources[0].resource,
        title.resources[6].resource,
        title.render_size,
    );
    if stretched {
        once!(warn, "FidelityFX: an upscaling the host could not record was stretched instead");
        OK
    } else {
        code
    }
}

extern "C" fn ffxDispatch(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(host) = HOST.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: the title passes a description.
    let kind = unsafe { (*desc).kind };
    if !dispatches(kind) {
        once!(
            warn,
            kind = format_args!("{kind:#x}"),
            "FidelityFX: only upscaling runs on the host; a dispatch was refused"
        );
        return UNKNOWN_TYPE;
    }
    // SAFETY: an upscaling, by its type.
    let title = unsafe { desc.cast::<DispatchUpscale>().read() };
    if context.is_null() {
        once!(warn, "FidelityFX dispatch arrived without a context");
        return stretch_instead(&title, PARAMETER);
    }
    if let Some(index) = [0, 1, 2, 6]
        .into_iter()
        .find(|&i| title.resources[i].resource.is_null())
    {
        once!(warn, index, "FidelityFX dispatch is missing a required texture");
        return stretch_instead(&title, PARAMETER);
    }
    // SAFETY: the title passes the same opaque context slot as for creation.
    let handle = unsafe { *context } as usize;
    let ctx = CONTEXTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .cloned();
    let Some(ctx) = ctx else {
        once!(warn, "FidelityFX dispatch referenced an unregistered context");
        return stretch_instead(&title, PARAMETER);
    };
    let live = ctx.live.lock().unwrap_or_else(|e| e.into_inner());
    if !*live {
        return stretch_instead(&title, PARAMETER);
    }
    let create = &ctx.creation.description;
    let Some(output_size) = dispatch_output(
        title.render_size,
        title.upscale_size,
        create.max_render,
        create.max_output,
    ) else {
        once!(warn, render = ?title.render_size, output = ?title.upscale_size,
            max_render = ?create.max_render, max_output = ?create.max_output,
            "FidelityFX rejected dispatch dimensions outside the context's allocations");
        return stretch_instead(&title, PARAMETER);
    };
    // Lists written down (`VICTORY_THIN=1`): recorded when the list's calls
    // are translated, on a thread of ours.
    if victory_gpu::external::later(title.list) {
        drop(live);
        return dispatch_later(host, context, ctx, title, output_size);
    }
    let mut dispatch = title;
    let recorded = victory_gpu::external::record(title.list, |list| {
        record_dispatch(list, host, context, &ctx, &mut dispatch, output_size)
    });
    drop(live);
    match recorded {
        Some(Some(code)) => code,
        Some(None) => stretch_instead(&title, PARAMETER),
        None => {
            once!(warn, "FidelityFX could not record to the title command list");
            stretch_instead(&title, PARAMETER)
        }
    }
}

/// What a dispatch recorded later takes along: plain copies of the title's
/// description and of its context's handle.
struct Later {
    dispatch: DispatchUpscale,
    handle: usize,
}

// SAFETY: plain values; the pointers in them name the title's command list
// and textures (alive until the list has run) and the provider's context
// (alive until its destroy, which is looked at before it is used).
unsafe impl Send for Later {}

/// The upscaling of a title list whose calls are written down: the work is
/// written down at its place (`victory_gpu::external::record_later`) with
/// copies of what it reads, and done on the list of ours the calls are
/// translated on. The title is told it went; what does not reach the
/// provider then is stretched instead, as [`stretch_instead`] does, and a
/// provider's refusal is logged by [`record_dispatch`].
fn dispatch_later(
    host: &'static Host,
    context: *mut *mut c_void,
    ctx: Arc<Context>,
    title: DispatchUpscale,
    output_size: [u32; 2],
) -> u32 {
    let mut dispatch = title;
    if !dispatch.header.next.is_null() {
        // (The title's chained descriptions need not live until then.)
        once!(warn, "FidelityFX: a dispatch recorded later leaves out the descriptions chained to it");
        dispatch.header.next = null_mut();
    }
    // SAFETY: the title passes its context slot, read at the call.
    let later = Later { dispatch, handle: unsafe { *context } as usize };
    let (colour, output, render) =
        (title.resources[0].resource as usize, title.resources[6].resource as usize, title.render_size);
    let placed = victory_gpu::external::record_later(title.list, move |list| {
        // (The whole of it is taken along, not its fields one by one.)
        let later = later;
        let Later { mut dispatch, handle } = later;
        let stretch = || list.stretch(colour as *const c_void, output as *const c_void, render);
        let Some(live) = live_later(&ctx) else {
            return stretch();
        };
        let mut slot = handle as *mut c_void;
        let recorded = record_dispatch(list, host, &raw mut slot, &ctx, &mut dispatch, output_size);
        drop(live);
        match recorded {
            Some(_) => true,
            None => {
                once!(warn, "FidelityFX: an upscaling recorded later could not reach the provider; stretched instead");
                stretch();
                true
            }
        }
    });
    if placed { OK } else { stretch_instead(&title, PARAMETER) }
}

/// The context's lock for a dispatch recorded later, on a thread of ours;
/// `None` once its destroy has begun (which holds the lock while it waits
/// for the lists submitted, perhaps for the one this thread translates) or
/// it is gone. Another such dispatch of the context holds the lock only
/// while it records.
fn live_later(ctx: &Context) -> Option<std::sync::MutexGuard<'_, bool>> {
    use std::sync::TryLockError;
    loop {
        if ctx.ending.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let live = match ctx.live.try_lock() {
            Ok(live) => live,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                std::thread::yield_now();
                continue;
            }
        };
        return (*live).then_some(live);
    }
}

/// The provider's dispatch on host objects, recorded on the host list;
/// `None` if it did not get that far (a texture with no host copy in the
/// state given, or too small).
fn record_dispatch(
    list: &victory_gpu::external::Recording<'_>,
    host: &Host,
    context: *mut *mut c_void,
    ctx: &Context,
    dispatch: &mut DispatchUpscale,
    output_size: [u32; 2],
) -> Option<u32> {
    let output = dispatch.resources[6].resource;
    let mut formats = Vec::new();
    let mut backing_extents = [None; 7];
    for (index, r) in dispatch.resources.iter_mut().enumerate() {
        if r.resource.is_null() {
            continue;
        }
        if index < 6 && list.prepare_input(r.resource).is_none() {
            once!(warn, index, "FidelityFX input could not be prepared on the title command list");
            return None;
        }
        let wanted = host_state(r.state);
        let texture = wanted.and_then(|w| list.texture(r.resource, w).filter(|t| t.state == w));
        let Some(texture) = texture else {
            once!(
                warn,
                index,
                state = r.state,
                "FidelityFX: a resource has no host texture in its state; the upscaling did not run"
            );
            return None;
        };
        backing_extents[index] = Some((texture.width, texture.height));
        let needed = minimum_extent(
            index,
            ctx.creation.description.flags,
            dispatch.render_size,
            output_size,
        );
        if texture.width < u64::from(needed[0]) || texture.height < needed[1] {
            once!(
                warn,
                index,
                ?needed,
                width = texture.width,
                height = texture.height,
                "FidelityFX resource cannot cover the active dispatch rectangle"
            );
            return None;
        }
        r.resource = texture.raw;
        formats.push(texture.format);
    }
    once!(info, ?formats, "FidelityFX: first upscaling, host formats of its textures");
    dispatch.list = list.commands();
    // SAFETY: the PC dispatch on host objects. The context is live and locked.
    let code = unsafe { (host.dispatch)(context, &raw mut dispatch.header) };
    if code == OK {
        victory_gpu::external::output_written(output);
        ctx.first_dispatch.call_once(|| {
            tracing::info!(render = ?dispatch.render_size, output = ?output_size, ?backing_extents,
                jitter = ?dispatch.jitter, motion_vector_scale = ?dispatch.motion_scale,
                provider = host.provider_name,
                "FidelityFX recorded the title's temporal dispatch on the host");
        });
    } else {
        once!(warn, code, "FidelityFX provider rejected the title dispatch");
    }
    Some(code)
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "fidelityfx",
        dlls: &[LOADER],
        exports: vec![
            hle_export!(ffxConfigure),
            hle_export!(ffxCreateContext),
            hle_export!(ffxDestroyContext),
            hle_export!(ffxDispatch),
            hle_export!(ffxQuery),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_logical_rectangles_against_the_current_context() {
        for display in [[1920, 1080], [2560, 1440], [3840, 2160]] {
            for divisor in [1.0_f32, 1.5, 1.7, 2.0, 3.0] {
                let render = display.map(|v| (v as f32 / divisor) as u32);
                assert_eq!(dispatch_output(render, display, render, display), Some(display));
                assert_eq!(dispatch_output(render, [0, 0], render, display), Some(display));
                assert_eq!(minimum_extent(2, 0x29, render, display), render);
                assert_eq!(minimum_extent(2, 0x2b, render, display), display);
                assert_eq!(minimum_extent(6, 0x29, render, display), display);
                assert_eq!(minimum_extent(3, 0x29, render, display), [1, 1]);
            }
        }
        assert_eq!(dispatch_output([0, 720], [0, 0], [1280, 720], [2560, 1440]), None);
        assert_eq!(dispatch_output([1281, 720], [0, 0], [1280, 720], [2560, 1440]), None);
        assert_eq!(dispatch_output([1280, 720], [0, 1440], [1280, 720], [2560, 1440]), None);
        assert_eq!(dispatch_output([1280, 720], [3000, 1440], [1280, 720], [2560, 1440]), None);
        assert_eq!(dispatch_output([1280, 720], [960, 540], [1280, 720], [2560, 1440]), None);
    }

    fn header(kind: u64, next: *mut Header) -> Header {
        Header { kind, next }
    }

    /// The title's chain: upscaler, console backend, version.
    #[test]
    fn the_console_backend_becomes_the_hosts() {
        let mut version = header(0x1_000b, null_mut());
        let mut console = Backend {
            header: header(CONSOLE_BACKEND, &raw mut version),
            device: 0x1234 as *mut c_void,
        };
        let mut upscale = CreateUpscale {
            header: header(CREATE_UPSCALE, &raw mut console.header),
            flags: 0x29,
            max_render: [1280, 720],
            max_output: [1920, 1080],
            message: 0,
        };
        let mut backend = Backend {
            header: header(0, null_mut()),
            device: 0x5678 as *mut c_void,
        };
        link(&mut upscale, &mut backend);
        assert_eq!(upscale.header.next, &raw mut backend.header);
        assert_eq!(backend.header.kind, HOST_BACKEND);
        assert_eq!(backend.header.next, &raw mut version);
        assert_eq!(backend.device, 0x5678 as *mut c_void);
        // Without a console backend right after it, ours goes in front.
        let mut upscale = CreateUpscale {
            header: header(CREATE_UPSCALE, &raw mut version),
            ..upscale
        };
        link(&mut upscale, &mut backend);
        assert_eq!(backend.header.next, &raw mut version);
    }

    /// Unset (the tests never set it), the loader's imports are not ours:
    /// the module is not in the registry and the title's own DLL serves
    /// them, as before the bridge.
    #[test]
    fn the_loader_is_served_only_with_the_variable() {
        assert!(!serving());
        assert!(provider().is_none());
        assert!(!crate::registry().iter().any(|m| m.serves(LOADER)));
        assert!(!crate::modules::all().iter().any(|m| m.serves(LOADER)));
        let resolver = crate::HleResolver::new(crate::HostPolicy::none());
        for name in ["ffxCreateContext", "ffxDestroyContext", "ffxConfigure", "ffxQuery", "ffxDispatch"] {
            let symbol = victory_pe::ImportSymbol::Name {
                hint: 0,
                name: name.into(),
            };
            assert_eq!(resolver.classify(LOADER, &symbol), None, "{name}");
        }
        // Registered, the module serves exactly the loader's five functions.
        let module = module();
        assert!(module.serves("AMD_FidelityFX_Loader_Xbox.dll"));
        let mut names: Vec<&str> = module.exports.iter().map(|e| e.name).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["ffxConfigure", "ffxCreateContext", "ffxDestroyContext", "ffxDispatch", "ffxQuery"]
        );
    }

    /// Without the PC build in the folder nothing is served, and the
    /// reason names the path looked at.
    #[test]
    fn without_the_dll_the_titles_own_runs() {
        let dir = tempfile::tempdir().unwrap();
        let error = use_host_dir(dir.path()).unwrap_err();
        assert!(error.contains(DLL), "{error}");
        assert!(error.contains(&dir.path().display().to_string()), "{error}");
        assert!(!serving());
    }

    #[test]
    fn the_folder_comes_from_the_variable_alone() {
        assert_eq!(host_dir(None), None);
        assert_eq!(host_dir(Some(OsStr::new(""))), None);
        assert_eq!(host_dir(Some(OsStr::new("   "))), None);
        assert_eq!(host_dir(Some(OsStr::new("\"\""))), None);
        assert_eq!(
            host_dir(Some(OsStr::new(r"D:\sdk\signedbin"))),
            Some(PathBuf::from(r"D:\sdk\signedbin"))
        );
        assert_eq!(
            host_dir(Some(OsStr::new(r#" "D:\FidelityFX SDK\signedbin" "#))),
            Some(PathBuf::from(r"D:\FidelityFX SDK\signedbin"))
        );
    }

    /// Frame generation (its context, swap chain and dispatch types) is
    /// refused; the upscaler alone is created and dispatched. Unserved,
    /// every call says there is no provider.
    #[test]
    fn only_the_upscaler_runs_on_the_host() {
        assert!(creates(CREATE_UPSCALE));
        assert!(dispatches(DISPATCH_UPSCALE));
        for other in [0x2_0001, 0x3_0001, 0x3_0005, DISPATCH_UPSCALE, CONSOLE_BACKEND] {
            assert!(!creates(other), "{other:#x}");
        }
        for other in [0x2_0003, 0x2_0004, CREATE_UPSCALE, GET_VERSIONS] {
            assert!(!dispatches(other), "{other:#x}");
        }
        let mut context = null_mut();
        let mut frame_generation = header(0x2_0001, null_mut());
        assert_eq!(ffxCreateContext(&mut context, &mut frame_generation, null()), NO_PROVIDER);
        assert_eq!(ffxDispatch(&mut context, &mut frame_generation), NO_PROVIDER);
        assert_eq!(ffxQuery(&mut context, &mut frame_generation), NO_PROVIDER);
        assert_eq!(ffxConfigure(&mut context, &mut frame_generation), NO_PROVIDER);
        assert_eq!(ffxDestroyContext(&mut context, null()), NO_PROVIDER);
    }

    fn offered(names: &[&str]) -> Vec<(u64, String)> {
        names.iter().zip(1..).map(|(name, id)| (id, name.to_string())).collect()
    }

    /// FSR 4.1.2 where the SDK offers it; otherwise the newest 3.1, which
    /// is what a card without FSR 4 is offered; with neither, said so.
    #[test]
    fn the_provider_is_4_1_2_or_else_the_newest_3_1() {
        assert_eq!(choose(offered(&["3.1.5", "4.1.2", "2.3.4"])), Ok((2, "4.1.2".into())));
        assert_eq!(choose(offered(&["4.1.2 (ML)", "3.1.5"])), Ok((1, "4.1.2 (ML)".into())));
        assert_eq!(choose(offered(&["2.3.4", "3.1.4", "3.1.5"])), Ok((3, "3.1.5".into())));
        assert_eq!(choose(offered(&["3.1.10", "3.1.9"])), Ok((1, "3.1.10".into())));
        // Other versions are not taken for either.
        assert_eq!(choose(offered(&["4.1.20", "3.1.5", "3.10.0"])), Ok((2, "3.1.5".into())));
        let none = choose(offered(&["2.3.4", "4.0.3"])).unwrap_err();
        assert!(none.contains("2.3.4, 4.0.3"), "{none}");
        assert!(choose(Vec::new()).unwrap_err().contains("nothing"));
        assert_eq!(fsr_3_1_patch("3.1.5"), Some(5));
        assert_eq!(fsr_3_1_patch("3.1.5-beta"), Some(5));
        assert_eq!(fsr_3_1_patch("3.1."), None);
        assert_eq!(fsr_3_1_patch("3.10.0"), None);
        assert_eq!(fsr_3_1_patch("4.1.2"), None);
    }

    #[test]
    fn resource_states_read_as_the_pc_backend_reads_them() {
        assert_eq!(host_state(0xc), Some(0xc0));
        assert_eq!(host_state(0x2), Some(0x8));
        assert_eq!(host_state(0x200), Some(0x10));
        assert_eq!(host_state(0x14), None);
    }

    #[test]
    fn file_versions_read_as_four_parts() {
        assert_eq!(version_text(0x0004_0001, 0x0002_11cb), "4.1.2.4555");
        assert_eq!(version_text(0, 0), "0.0.0.0");
    }
}
