//! An icon of the user's own for the windows we show (opt-in).
//!
//! The project ships no icon. `VICTORY_WINDOW_ICON=<path>` points at a file
//! of the user's own (e.g. a local `.ico`); the title's top-level window (made
//! through the HLE `CreateWindowExW`/`CreateWindowExA`) and our host window show
//! it, big and small. The window title keeps the project's own name. Unset
//! or empty: nothing changes (no extra calls). An icon that does not load is
//! one WARN; window creation never fails because of it.

use std::path::PathBuf;

/// The variable naming the icon file.
pub const VARIABLE: &str = "VICTORY_WINDOW_ICON";

/// The icon path a value of [`VARIABLE`] names: trimmed, empty = none.
pub fn icon_path(value: Option<&str>) -> Option<PathBuf> {
    let value = value?.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// Gives `hwnd` the user's icon, if [`VARIABLE`] names one. Takes the raw
/// window handle so both `windows` and `windows-sys` callers can pass it.
#[cfg(windows)]
pub fn apply(hwnd: *mut std::ffi::c_void) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{ICON_BIG, ICON_SMALL, SendMessageW, WM_SETICON};

    if hwnd.is_null() {
        return;
    }
    let Some((big, small)) = icons() else {
        return;
    };
    let hwnd = HWND(hwnd);
    // SAFETY: a window just created; the icons live for the whole process.
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small as isize)),
        );
    }
}

/// The big and small icons, loaded once and kept for the process lifetime
/// (as raw handles, so they can be shared between threads).
#[cfg(windows)]
fn icons() -> Option<(usize, usize)> {
    static ICONS: std::sync::OnceLock<Option<(usize, usize)>> = std::sync::OnceLock::new();
    *ICONS.get_or_init(|| {
        let path = icon_path(std::env::var(VARIABLE).ok().as_deref())?;
        match load(&path) {
            Ok(icons) => {
                tracing::info!("window icon from {VARIABLE}: {}", path.display());
                Some(icons)
            }
            Err(error) => {
                tracing::warn!("{VARIABLE}: cannot load icon {}: {error}", path.display());
                None
            }
        }
    })
}

#[cfg(windows)]
fn load(path: &std::path::Path) -> windows::core::Result<(usize, usize)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, IMAGE_ICON, LR_LOADFROMFILE, LoadImageW, SM_CXICON, SM_CXSMICON,
        SM_CYICON, SM_CYSMICON, SYSTEM_METRICS_INDEX,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let one = |cx: SYSTEM_METRICS_INDEX, cy: SYSTEM_METRICS_INDEX| {
        // SAFETY: `wide` is a NUL-terminated path that outlives the call.
        unsafe {
            LoadImageW(
                None,
                PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                GetSystemMetrics(cx),
                GetSystemMetrics(cy),
                LR_LOADFROMFILE,
            )
        }
        .map(|icon| icon.0 as usize)
    };
    Ok((one(SM_CXICON, SM_CYICON)?, one(SM_CXSMICON, SM_CYSMICON)?))
}

#[cfg(test)]
mod tests {
    use super::icon_path;
    use std::path::PathBuf;

    #[test]
    fn the_path_is_trimmed_and_empty_is_none() {
        assert_eq!(icon_path(None), None);
        assert_eq!(icon_path(Some("")), None);
        assert_eq!(icon_path(Some("   ")), None);
        assert_eq!(
            icon_path(Some("  C:/icons/mine.ico \n")),
            Some(PathBuf::from("C:/icons/mine.ico"))
        );
    }
}
