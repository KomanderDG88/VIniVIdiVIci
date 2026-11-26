//! The Windows SDK's shader compiler (`dxcompiler.dll`), loaded the first
//! time it is wanted, for the one shader of ours that needs a shader model
//! the system's compiler (`D3DCompile`, `crate::host::compile`: 5.1 at
//! most) does not know: the amplification stage of [`super`].
//!
//! The DLL is `VICTORY_DXCOMPILER` if set, else the one beside the newest
//! Windows SDK's `dxil.dll` (where `victory_shader::emit::validator` finds
//! that), else whatever `LoadLibrary` finds. A `dxil.dll` beside it is
//! loaded first, so that the compiler finds it and signs what it makes;
//! without one the container is unsigned and [`super::bytes`] gives it the
//! hash the translations carry.
//!
//! Called through its COM tables by slot, as `victory_shader`'s validator
//! calls `dxil.dll`: the `windows` crate's bindings would link the DLL to
//! the executable, which must start without it.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW, LoadLibraryW};

type Hresult = i32;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// `CLSID_DxcCompiler`.
const CLSID_DXC_COMPILER: Guid = Guid(0x73e2_2d93, 0xe6ce, 0x47f3, [0xb5, 0xbf, 0xf0, 0x66, 0x4f, 0x39, 0xc1, 0xb0]);
/// `IDxcCompiler3`.
const IID_IDXC_COMPILER3: Guid = Guid(0x228b_4687, 0x5a6a, 0x4730, [0x90, 0x0c, 0x97, 0x02, 0xb2, 0x20, 0x3f, 0x54]);
/// `IDxcResult` (an `IDxcOperationResult`: status, result, errors).
const IID_IDXC_RESULT: Guid = Guid(0x5834_6cda, 0xdde7, 0x4497, [0x94, 0x61, 0x6f, 0x87, 0xaf, 0x5e, 0x06, 0x59]);
/// `DXC_CP_UTF8`.
const CP_UTF8: u32 = 65001;

/// `DxcBuffer`: the source.
#[repr(C)]
struct DxcBuffer {
    ptr: *const c_void,
    size: usize,
    encoding: u32,
}

type CreateInstance = unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> Hresult;

/// COM object: a pointer to a table of method pointers.
type Object = *mut *const usize;

/// Method `slot` of `obj`'s table.
///
/// # Safety
/// `obj` is a live object whose interface has a method of type `F` there.
unsafe fn method<F: Copy>(obj: Object, slot: usize) -> F {
    // SAFETY: as the caller says.
    unsafe { std::mem::transmute_copy(&*(*obj).add(slot)) }
}

/// # Safety
/// `obj` is null or a live object of ours.
unsafe fn release(obj: Object) {
    if !obj.is_null() {
        // SAFETY: slot 2 of every COM interface is Release.
        unsafe { method::<unsafe extern "system" fn(Object) -> u32>(obj, 2)(obj) };
    }
}

/// The bytes of an `IDxcBlob` (GetBufferPointer 3, GetBufferSize 4).
///
/// # Safety
/// `blob` is a live blob.
unsafe fn blob_bytes(blob: Object) -> Vec<u8> {
    // SAFETY: as the caller says.
    unsafe {
        let ptr: unsafe extern "system" fn(Object) -> *const u8 = method(blob, 3);
        let len: unsafe extern "system" fn(Object) -> usize = method(blob, 4);
        let (p, n) = (ptr(blob), len(blob));
        if p.is_null() || n == 0 { Vec::new() } else { std::slice::from_raw_parts(p, n).to_vec() }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The compiler's `DxcCreateInstance`, its DLL loaded once (and kept).
fn create_instance() -> Result<CreateInstance, String> {
    static CREATE: OnceLock<Result<CreateInstance, String>> = OnceLock::new();
    let create = CREATE.get_or_init(|| {
        let sdk = victory_shader::emit::validator::Validator::default_path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .filter(|d| d.join("dxcompiler.dll").exists());
        let path: Option<PathBuf> = std::env::var_os("VICTORY_DXCOMPILER")
            .map(PathBuf::from)
            .or_else(|| sdk.as_ref().map(|d| d.join("dxcompiler.dll")));
        // SAFETY: plain loader calls with NUL-terminated names; the
        // modules stay loaded for the rest of the run.
        let module = unsafe {
            match &path {
                Some(path) => {
                    // Its signer first: the compiler loads `dxil.dll` by
                    // name, which then finds this one loaded.
                    if let Some(dir) = path.parent() {
                        let dxil = dir.join("dxil.dll");
                        if dxil.exists() {
                            LoadLibraryExW(wide(&dxil).as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
                        }
                    }
                    LoadLibraryExW(wide(path).as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH)
                }
                None => LoadLibraryW(wide(Path::new("dxcompiler.dll")).as_ptr()),
            }
        };
        let name = path.as_ref().map_or_else(|| "dxcompiler.dll".to_owned(), |p| p.display().to_string());
        if module.is_null() {
            return Err(format!("cannot load {name}"));
        }
        // SAFETY: a loaded module and a NUL-terminated name.
        let create = unsafe { GetProcAddress(module, c"DxcCreateInstance".as_ptr().cast()) }
            .ok_or_else(|| format!("{name} has no DxcCreateInstance"))?;
        tracing::info!(dll = %name, "the shader compiler for the mesh launch stage");
        // SAFETY: the export's signature in `dxcapi.h`,
        // `HRESULT DxcCreateInstance(REFCLSID, REFIID, LPVOID *)`.
        Ok(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateInstance>(create) })
    });
    create.clone()
}

/// Compiles function `entry` of `source` for `profile` (`as_6_6`, say):
/// the container, or the compiler's complaint.
pub(super) fn compile(source: &str, entry: &str, profile: &str) -> Result<Vec<u8>, String> {
    let create = create_instance()?;
    let args: Vec<Vec<u16>> = ["-E", entry, "-T", profile]
        .iter()
        .map(|a| a.encode_utf16().chain([0]).collect())
        .collect();
    let pointers: Vec<*const u16> = args.iter().map(|a| a.as_ptr()).collect();
    let buffer = DxcBuffer { ptr: source.as_ptr().cast(), size: source.len(), encoding: CP_UTF8 };
    // SAFETY: COM calls by the slots of `dxcapi.h`: IDxcCompiler3::Compile
    // (3); IDxcOperationResult GetStatus (3), GetResult (4), GetErrorBuffer
    // (5). Every pointer passed lives through its call; every object got
    // is released once.
    unsafe {
        let mut compiler: *mut c_void = std::ptr::null_mut();
        let hr = create(&CLSID_DXC_COMPILER, &IID_IDXC_COMPILER3, &mut compiler);
        if hr < 0 || compiler.is_null() {
            return Err(format!("DxcCreateInstance(compiler) failed: {hr:#x}"));
        }
        let compiler: Object = compiler.cast();
        type Compile = unsafe extern "system" fn(
            Object,
            *const DxcBuffer,
            *const *const u16,
            u32,
            *mut c_void,
            *const Guid,
            *mut *mut c_void,
        ) -> Hresult;
        let compile: Compile = method(compiler, 3);
        let mut result: *mut c_void = std::ptr::null_mut();
        let hr = compile(
            compiler,
            &buffer,
            pointers.as_ptr(),
            pointers.len() as u32,
            std::ptr::null_mut(),
            &IID_IDXC_RESULT,
            &mut result,
        );
        release(compiler);
        if hr < 0 || result.is_null() {
            return Err(format!("Compile failed: {hr:#x}"));
        }
        let result: Object = result.cast();
        let mut status: Hresult = 0;
        let get_status: unsafe extern "system" fn(Object, *mut Hresult) -> Hresult = method(result, 3);
        get_status(result, &mut status);
        if status < 0 {
            let mut errors: Object = std::ptr::null_mut();
            let get_errors: unsafe extern "system" fn(Object, *mut Object) -> Hresult = method(result, 5);
            let text = if get_errors(result, &mut errors) >= 0 && !errors.is_null() {
                let text = String::from_utf8_lossy(&blob_bytes(errors)).into_owned();
                release(errors);
                text
            } else {
                String::new()
            };
            release(result);
            return Err(format!("{status:#x}: {}", text.trim_end_matches('\0').trim()));
        }
        let mut blob: Object = std::ptr::null_mut();
        let get_result: unsafe extern "system" fn(Object, *mut Object) -> Hresult = method(result, 4);
        let hr = get_result(result, &mut blob);
        release(result);
        if hr < 0 || blob.is_null() {
            return Err(format!("no compiled code: {hr:#x}"));
        }
        let code = blob_bytes(blob);
        release(blob);
        if code.len() < 32 || &code[..4] != b"DXBC" {
            return Err(format!("not a shader container ({} bytes)", code.len()));
        }
        Ok(code)
    }
}
