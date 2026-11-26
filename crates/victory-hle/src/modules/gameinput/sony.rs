//! A Sony DualSense, the PlayStation 5's pad, as a host gamepad.
//!
//! XInput does not list it: with only that pad connected the title got the
//! keyboard's stand-in pad and nothing the pad did. Here its HID input
//! reports are read as they are: the pad is found among the raw-input
//! devices by vendor and product, opened for reading and shared, and a
//! thread of its own reads report after report and keeps the last state.
//! No driver and no helper program is needed, and nothing is written to the
//! pad. `VICTORY_SONY_PAD=0` leaves it alone.
//!
//! The reports, as the pad sends them (the layout is widely documented):
//! over USB, report 0x01 of 64 bytes; over Bluetooth, report 0x31 of 78
//! bytes with the same fields one byte later, or, until some program has
//! asked the pad for its calibration, a short report 0x01 with the fields
//! in another order (Windows hands it over padded to 78 bytes).
//! shortcut: no rumble, adaptive triggers, touchpad or motion; the PS and
//! mute buttons are not passed on. A second Sony pad is not looked for.
//! shortcut: a read waits for the next report. A pad that goes to sleep
//! without its read failing keeps its last state (a stick held, say) until
//! a report comes again; a read with a time limit would let it go.

use super::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER, RIGHT_STICK,
    UP, VIEW, X, Y,
};

/// Sony's vendor number, and the pads read: DualSense, DualSense Edge.
#[cfg_attr(not(windows), allow(dead_code))]
const VENDOR: u32 = 0x054C;
#[cfg_attr(not(windows), allow(dead_code))]
const PRODUCTS: [u32; 2] = [0x0CE6, 0x0DF2];

/// An input report over USB: its id and its length.
const USB: (u8, usize) = (0x01, 64);
/// The full input report over Bluetooth.
const BLUETOOTH: u8 = 0x31;

/// The pad's state in the input report `report` (the report whole, its id
/// first, as the host hands it over); `None` if it is not one of the pad's
/// input reports or too short to hold the fields.
pub(super) fn parse(report: &[u8]) -> Option<PadState> {
    let (&id, fields) = report.split_first()?;
    if id == BLUETOOTH {
        // A sequence byte, then the fields as over USB.
        return full(fields.get(1..)?);
    }
    if id != USB.0 {
        return None;
    }
    if report.len() == USB.1 { full(fields) } else { short(fields) }
}

/// The fields of the full report: sticks, triggers, a counter, buttons.
fn full(f: &[u8]) -> Option<PadState> {
    let f = f.get(..10)?;
    Some(state([f[0], f[1], f[2], f[3]], [f[4], f[5]], [f[7], f[8], f[9]]))
}

/// The fields of the short Bluetooth report: sticks, buttons, triggers.
fn short(f: &[u8]) -> Option<PadState> {
    let f = f.get(..9)?;
    Some(state([f[0], f[1], f[2], f[3]], [f[7], f[8]], [f[4], f[5], f[6]]))
}

/// The console's pad state for the pad's sticks (left x, left y, right x,
/// right y: 0 to 255, 0 is left and up), triggers and three button bytes.
/// Raw values, no dead zone: titles apply their own.
fn state(sticks: [u8; 4], triggers: [u8; 2], buttons: [u8; 3]) -> PadState {
    let stick = |v: u8| ((f32::from(v) - 128.0) / 127.0).max(-1.0);
    // The first byte's low half is the direction pad: 0 up, then clockwise
    // by eighths, 8 and above at rest.
    let pad = match buttons[0] & 0x0F {
        0 => UP,
        1 => UP | RIGHT,
        2 => RIGHT,
        3 => RIGHT | DOWN,
        4 => DOWN,
        5 => DOWN | LEFT,
        6 => LEFT,
        7 => LEFT | UP,
        _ => 0,
    };
    // Square, cross, circle, triangle: where the console's X, A, B, Y sit.
    // Create is the console's View (the touchpad's click too: it is where
    // the hand looks for it), Options its Menu.
    const BITS: [(usize, u8, u32); 11] = [
        (0, 0x10, X),
        (0, 0x20, A),
        (0, 0x40, B),
        (0, 0x80, Y),
        (1, 0x01, LEFT_SHOULDER),
        (1, 0x02, RIGHT_SHOULDER),
        (1, 0x10, VIEW),
        (1, 0x20, MENU),
        (1, 0x40, LEFT_STICK),
        (1, 0x80, RIGHT_STICK),
        (2, 0x02, VIEW),
    ];
    PadState {
        buttons: BITS
            .iter()
            .filter(|(byte, bit, _)| buttons[*byte] & bit != 0)
            .fold(pad, |all, (_, _, button)| all | button),
        left_trigger: f32::from(triggers[0]) / 255.0,
        right_trigger: f32::from(triggers[1]) / 255.0,
        left_x: stick(sticks[0]),
        left_y: -stick(sticks[1]),
        right_x: stick(sticks[2]),
        right_y: -stick(sticks[3]),
    }
}

/// The pad's last state, read by a thread of its own; `None` while no such
/// pad is connected (or with `VICTORY_SONY_PAD=0`).
#[cfg(windows)]
pub(super) fn connected() -> Option<PadState> {
    reader::state()
}

#[cfg(windows)]
mod reader {
    use std::sync::{Mutex, Once, OnceLock};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
    };
    use windows_sys::Win32::UI::Input::{
        GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RID_DEVICE_INFO,
        RIDI_DEVICEINFO, RIDI_DEVICENAME, RIM_TYPEHID,
    };

    use super::super::{PadState, lock};

    /// How often a pad is looked for while none is read.
    const LOOK_AGAIN: Duration = Duration::from_secs(2);

    static STATE: Mutex<Option<PadState>> = Mutex::new(None);

    fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("VICTORY_SONY_PAD").is_none_or(|v| v != "0"))
    }

    pub(super) fn state() -> Option<PadState> {
        if !on() {
            return None;
        }
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            if let Err(e) = std::thread::Builder::new().name("victory-sony-pad".into()).spawn(run) {
                tracing::warn!(error = %e, "no thread to read a Sony pad: it is not read");
            }
        });
        *lock(&STATE)
    }

    /// What [`open`] found.
    enum Found {
        /// No Sony pad is connected.
        Absent,
        /// One, opened.
        Opened(HANDLE),
        /// One that could not be opened, and why.
        Unopened(std::io::Error),
    }

    /// Looks for a pad, reads it until it goes, looks again. A pad that
    /// cannot be opened, or gives no report, is looked at again every
    /// [`LOOK_AGAIN`] but told of once while the cause stays the same.
    fn run() {
        // The trouble last told of: which (1 not opened, 2 no report) and
        // the system's error number.
        let mut told: Option<(u8, Option<i32>)> = None;
        let tell = |told: &mut Option<(u8, Option<i32>)>, which: u8, error: &std::io::Error, what: &str| {
            let cause = Some((which, error.raw_os_error()));
            if *told != cause {
                *told = cause;
                tracing::warn!(%error, "{what}: it is not read (told once while this lasts)");
            }
        };
        loop {
            match open() {
                Found::Absent => told = None,
                Found::Unopened(error) => {
                    tell(&mut told, 1, &error, "a Sony pad is connected but could not be opened");
                }
                Found::Opened(pad) => {
                    let (reports, error) = read(pad);
                    *lock(&STATE) = None;
                    // SAFETY: our handle, no longer used.
                    unsafe { CloseHandle(pad) };
                    if reports > 0 {
                        tracing::info!(%error, reports, "the Sony DualSense is gone");
                        told = None;
                    } else {
                        tell(&mut told, 2, &error, "a Sony pad was opened but no report could be read from it");
                    }
                }
            }
            std::thread::sleep(LOOK_AGAIN);
        }
    }

    /// Reads input reports until a read fails (the pad was unplugged or
    /// went to sleep), keeping the state each one holds. Returns the
    /// reports read and why the last read failed. The pad is told of as
    /// read at its first report, not before.
    fn read(pad: HANDLE) -> (u64, std::io::Error) {
        // Longer than the longest report (78 bytes over Bluetooth).
        let mut report = [0u8; 128];
        let mut reports = 0u64;
        loop {
            let mut got = 0u32;
            // SAFETY: our handle; room for `report.len()` bytes; a read that
            // waits for the next report.
            let read = unsafe {
                ReadFile(pad, report.as_mut_ptr(), report.len() as u32, &mut got, std::ptr::null_mut())
            };
            if read == 0 {
                return (reports, std::io::Error::last_os_error());
            }
            if let Some(state) = super::parse(&report[..(got as usize).min(report.len())]) {
                *lock(&STATE) = Some(state);
                if reports == 0 {
                    tracing::info!("a Sony DualSense is read as the first gamepad (VICTORY_SONY_PAD=0: not)");
                }
                reports += 1;
            }
        }
    }

    /// The first Sony pad among the raw-input devices, opened for reading
    /// and shared with whoever else reads it.
    fn open() -> Found {
        let entry = size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut count = 0u32;
        // SAFETY: a count is asked for, nothing is written but it.
        if unsafe { GetRawInputDeviceList(std::ptr::null_mut(), &mut count, entry) } != 0 || count == 0 {
            return Found::Absent;
        }
        // (Room for a few more: devices come and go between the two calls.)
        let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize + 8];
        let mut room = devices.len() as u32;
        // SAFETY: room for `room` entries.
        let listed = unsafe { GetRawInputDeviceList(devices.as_mut_ptr(), &mut room, entry) };
        if listed == u32::MAX {
            return Found::Absent;
        }
        let mut unopened = None;
        devices.truncate(listed as usize);
        for device in devices.iter().filter(|d| d.dwType == RIM_TYPEHID) {
            let mut info = RID_DEVICE_INFO { cbSize: size_of::<RID_DEVICE_INFO>() as u32, ..Default::default() };
            let mut size = info.cbSize;
            // SAFETY: room for the info, its size said.
            let got = unsafe {
                GetRawInputDeviceInfoW(device.hDevice, RIDI_DEVICEINFO, (&raw mut info).cast(), &mut size)
            };
            if got == u32::MAX || got == 0 || info.dwType != RIM_TYPEHID {
                continue;
            }
            // SAFETY: a HID device's info is the `hid` member.
            let hid = unsafe { info.Anonymous.hid };
            // (Usage page 1, usage 5: the pad itself, not another function of it.)
            if hid.dwVendorId != super::VENDOR
                || !super::PRODUCTS.contains(&hid.dwProductId)
                || (hid.usUsagePage, hid.usUsage) != (1, 5)
            {
                continue;
            }
            let mut length = 0u32;
            // SAFETY: the name's length in characters is asked for.
            unsafe { GetRawInputDeviceInfoW(device.hDevice, RIDI_DEVICENAME, std::ptr::null_mut(), &mut length) };
            let mut name = vec![0u16; length as usize + 1];
            // SAFETY: room for `length` characters and the end.
            let got = unsafe {
                GetRawInputDeviceInfoW(device.hDevice, RIDI_DEVICENAME, name.as_mut_ptr().cast(), &mut length)
            };
            if got == u32::MAX || got == 0 {
                continue;
            }
            // SAFETY: a device path that ends in 0; read access, shared.
            let pad = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if pad != INVALID_HANDLE_VALUE && !pad.is_null() {
                return Found::Opened(pad);
            }
            unopened = Some(std::io::Error::last_os_error());
        }
        unopened.map_or(Found::Absent, Found::Unopened)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full report's fields: sticks at rest, nothing pressed.
    fn rest() -> [u8; 10] {
        [128, 128, 128, 128, 0, 0, 7, 8, 0, 0]
    }

    fn usb(fields: [u8; 10]) -> Vec<u8> {
        let mut report = vec![0u8; 64];
        report[0] = 0x01;
        report[1..11].copy_from_slice(&fields);
        report
    }

    fn bluetooth(fields: [u8; 10]) -> Vec<u8> {
        let mut report = vec![0u8; 78];
        report[0] = 0x31;
        report[1] = 0x41; // sequence and flags
        report[2..12].copy_from_slice(&fields);
        report
    }

    #[test]
    fn a_pad_at_rest_reads_as_nothing() {
        for report in [usb(rest()), bluetooth(rest())] {
            assert_eq!(parse(&report), Some(PadState::default()));
        }
    }

    #[test]
    fn sticks_and_triggers_keep_their_range_and_up_is_positive() {
        // Left stick right and up to the stop, right stick left and down.
        let mut f = rest();
        (f[0], f[1], f[2], f[3], f[4], f[5]) = (255, 0, 0, 255, 255, 51);
        for report in [usb(f), bluetooth(f)] {
            let pad = parse(&report).unwrap();
            assert_eq!((pad.left_x, pad.left_y), (1.0, 1.0));
            assert_eq!((pad.right_x, pad.right_y), (-1.0, -1.0));
            assert_eq!(pad.left_trigger, 1.0);
            assert!((pad.right_trigger - 0.2).abs() < 1e-6);
            assert_eq!(pad.buttons, 0);
        }
    }

    #[test]
    fn buttons_go_to_the_consoles_places() {
        let pressed = |b0: u8, b1: u8, b2: u8| {
            let mut f = rest();
            (f[7], f[8], f[9]) = (b0, b1, b2);
            parse(&usb(f)).unwrap().buttons
        };
        // Square, cross, circle, triangle.
        assert_eq!(pressed(0x18, 0, 0), X);
        assert_eq!(pressed(0x28, 0, 0), A);
        assert_eq!(pressed(0x48, 0, 0), B);
        assert_eq!(pressed(0x88, 0, 0), Y);
        // L1, R1, Create, Options, L3, R3; the trigger bits are not buttons.
        assert_eq!(pressed(8, 0x01, 0), LEFT_SHOULDER);
        assert_eq!(pressed(8, 0x02, 0), RIGHT_SHOULDER);
        assert_eq!(pressed(8, 0x0C, 0), 0);
        assert_eq!(pressed(8, 0x10, 0), VIEW);
        assert_eq!(pressed(8, 0x20, 0), MENU);
        assert_eq!(pressed(8, 0x40, 0), LEFT_STICK);
        assert_eq!(pressed(8, 0x80, 0), RIGHT_STICK);
        // The touchpad's click is View; PS and mute are not passed on.
        assert_eq!(pressed(8, 0, 0x02), VIEW);
        assert_eq!(pressed(8, 0, 0x05), 0);
        // Several at once.
        assert_eq!(pressed(0x20 | 4, 0x21, 0), A | DOWN | MENU | LEFT_SHOULDER);
    }

    #[test]
    fn the_direction_pad_goes_round_by_eighths() {
        let around = [UP, UP | RIGHT, RIGHT, RIGHT | DOWN, DOWN, DOWN | LEFT, LEFT, LEFT | UP, 0, 0];
        for (hat, &expected) in around.iter().enumerate() {
            let mut f = rest();
            f[7] = hat as u8;
            assert_eq!(parse(&usb(f)).unwrap().buttons, expected, "hat {hat}");
        }
    }

    #[test]
    fn the_short_bluetooth_report_has_its_own_order() {
        // Sticks, three button bytes, triggers; padded to the long length.
        let mut report = vec![0u8; 78];
        report[..10].copy_from_slice(&[0x01, 255, 128, 128, 0, 0x20 | 2, 0x20, 0x02 | 0xA4, 255, 0]);
        let pad = parse(&report).unwrap();
        assert_eq!((pad.left_x, pad.left_y, pad.right_x, pad.right_y), (1.0, 0.0, 0.0, 1.0));
        assert_eq!(pad.buttons, A | RIGHT | MENU | VIEW);
        assert_eq!((pad.left_trigger, pad.right_trigger), (1.0, 0.0));
        // The same without the padding.
        assert_eq!(parse(&report[..10]), Some(pad));
    }

    #[test]
    fn other_reports_and_short_ones_are_not_the_pads() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0x05, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), None);
        assert_eq!(parse(&[0x01, 128, 128, 128]), None);
        assert_eq!(parse(&[0x31, 0, 128, 128, 128, 128]), None);
        let mut cut = usb(rest());
        cut.truncate(9);
        assert_eq!(parse(&cut), None);
    }

    /// With a DualSense connected to this PC: its state as the layer reads
    /// it, for a look by eye (`cargo test -p victory-hle sony -- --ignored
    /// --nocapture`). Hold a button or a stick while it runs.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a DualSense connected"]
    fn a_connected_pad_is_read() {
        let mut seen = None;
        for _ in 0..50 {
            seen = connected();
            if seen.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let first = seen.expect("no DualSense was found in five seconds");
        println!("at first: {first:?}");
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!("two seconds on: {:?}", connected());
    }
}
