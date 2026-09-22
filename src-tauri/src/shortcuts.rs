use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Flags shared with the double-shift keyboard hook. Capture gestures are
/// fixed (double-shift for text, double-shift-hold for voice), so the only
/// shared state is the gaming-mode suppression flag.
pub struct AppFlags {
    pub gaming: AtomicBool,
    /// The main webview has loaded its state and is safe to reveal without
    /// showing an intermediate/blank frame.
    pub frontend_ready: AtomicBool,
    /// A tray click or second-instance launch happened while the frontend was
    /// still loading. Fulfil it as soon as the webview reports ready.
    pub show_requested: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureOpenPayload {
    pub mode: String,
}

/// Show the quick-capture window in voice mode.
pub fn show_voice_capture(app: &AppHandle) {
    debug_log("voice capture requested");
    show_capture(app, "voice");
}

fn show_capture(app: &AppHandle, mode: &str) {
    let Some(win) = app.get_webview_window("quick-capture") else {
        eprintln!("[pocket] quick-capture window not found!");
        return;
    };
    let _ = win.center();
    let shown = win.show();
    let focused = win.set_focus();
    diag_log(&format!(
        "show_capture(mode={mode}) show={shown:?} focus={focused:?}"
    ));
    let _ = app.emit_to(
        "quick-capture",
        "capture-open",
        CaptureOpenPayload { mode: mode.into() },
    );
}

/// Double-Shift tap path: grab the foreground app's selected text and save it
/// straight into the active workspace — no window opens. On success a dark
/// "Captured" pill is shown on the active monitor.
pub fn save_text_capture_from_hotkey(app: &AppHandle) {
    // A visible capture panel is voice-only. Ignore a text gesture rather than
    // hiding an active recording and accidentally leaving its microphone live.
    let visible = app
        .get_webview_window("quick-capture")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        debug_log("voice capture visible -> ignoring text capture gesture");
        return;
    }
    let app_handle = app.clone();
    std::thread::Builder::new()
        .name("grab-selection".into())
        .spawn(move || {
            grab_log("grab: worker started (double-shift tap -> direct save)");
            let grabbed = grab_selected_text();
            let for_main = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                let Some(text) = grabbed.filter(|t| !t.trim().is_empty()) else {
                    grab_log("grab: nothing selected -> nothing saved, no sound");
                    return;
                };
                match crate::commands::save_hotkey_text_capture(&for_main, text) {
                    Ok(Some(item)) => {
                        grab_log(&format!("grab: saved id={} -> HUD", item.id));
                        crate::hud::show_hud(&for_main, "Captured", false);
                    }
                    Ok(None) => grab_log("grab: duplicate of latest item -> skipped"),
                    Err(e) => grab_log(&format!("grab: direct save FAILED: {e}")),
                }
            });
        })
        .ok();
}

/// General diagnostics file log (same mechanism as the grab log): release GUI
/// builds drop stderr, so anything needed as evidence goes here.
/// Lives next to the executable as `pocket-diag.log`.
pub(crate) fn diag_log(message: &str) {
    use std::io::Write;
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
        .join("pocket-diag.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{}] {message}", now_ms());
    }
    debug_log(message);
}

/// File log for the grab flow (release GUI builds drop stderr, so eprintln
/// alone is invisible there). Lives next to the executable as
/// `pocket-grab.log` — the data directory beside the exe is proven writable,
/// unlike %TEMP% which this process demonstrably cannot create files in.
fn grab_log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
        .join("pocket-grab.log")
}

fn grab_log(message: &str) {
    use std::io::Write;
    let path = grab_log_path();
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            let _ = writeln!(f, "[{}] {message}", now_ms());
        }
        Err(e) => {
            eprintln!("[pocket] grab_log failed ({}): {e}", path.display());
        }
    }
    debug_log(message);
}

/// Snapshot → synthetic Ctrl+C → compare. Returns the newly selected text, or
/// `None` when nothing was selected (clipboard unchanged/empty).
#[cfg(windows)]
fn grab_selected_text() -> Option<String> {
    // The user's fingers may still hold Shift: wait (bounded) for physical
    // release first, otherwise we'd send Ctrl+Shift+C instead of Ctrl+C.
    let mut waited_ms: u64 = 0;
    loop {
        let down = unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LSHIFT, VK_RSHIFT,
            };
            let l = GetAsyncKeyState(VK_LSHIFT.0 as i32) as u16;
            let r = GetAsyncKeyState(VK_RSHIFT.0 as i32) as u16;
            (l & 0x8000) != 0 || (r & 0x8000) != 0
        };
        if !down || waited_ms >= 300 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited_ms += 10;
    }
    grab_log(&format!("grab: shift released after {waited_ms}ms"));

    // Wipe first: with the old content gone, anything we read afterwards
    // must be fresh output of THIS action — stale content can never leak
    // into the bar. (Overwriting the clipboard is accepted behavior.)
    clear_clipboard();
    grab_log("grab: clipboard cleared (sentinel)");

    if !send_ctrl_c() {
        grab_log("grab: SendInput(Ctrl+C) failed -> opening empty");
        return None;
    }
    grab_log("grab: synthetic Ctrl+C sent via SendInput");

    // The target app needs a moment to service the copy.
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Prefer the HTML flavor when the source app copied one: it carries the
    // formatting (bold, italics, links, lists) that plain text loses, and is
    // converted to markdown below. Plain text remains the fallback.
    if crate::clipboard_html::has_html() {
        match crate::clipboard_html::read_html() {
            Some(html) => {
                let md = crate::clipboard_html::html_to_markdown(&html);
                if !md.trim().is_empty() {
                    grab_log(&format!(
                        "grab: clipboard HTML FRESH md_len={} -> saving as markdown",
                        md.len()
                    ));
                    return Some(md);
                }
                grab_log("grab: HTML present but converted empty -> falling back to plain");
            }
            None => grab_log("grab: HTML present but unreadable -> falling back to plain"),
        }
    }

    let after = read_clipboard_text();
    match after {
        Some(t) if !t.trim().is_empty() => {
            // Never write captured user content to diagnostic logs. The length
            // is sufficient to confirm that the selection flow worked.
            grab_log(&format!("grab: clipboard FRESH len={} -> saving", t.len()));
            Some(t)
        }
        _ => {
            grab_log("grab: clipboard still empty (nothing selected) -> opening empty");
            None
        }
    }
}

#[cfg(not(windows))]
fn grab_selected_text() -> Option<String> {
    None
}

/// Empties the clipboard (best effort). Called before the synthetic Ctrl+C so
/// a later read can only ever see fresh output of this action.
#[cfg(windows)]
fn clear_clipboard() {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};

    unsafe {
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                let _ = EmptyClipboard();
                let _ = CloseClipboard();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// Current clipboard Unicode text, if any. Retries briefly — the clipboard is
/// often momentarily locked by the app that owns it.
#[cfg(windows)]
fn read_clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    /// CF_UNICODETEXT (13). The `windows` 0.61 metadata exposes
    /// `GetClipboardData` as taking a plain u32, so spell it out.
    const CF_UNICODETEXT: u32 = 13;

    let mut opened = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !opened {
        return None;
    }

    let result = unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            None
        } else {
            match GetClipboardData(CF_UNICODETEXT) {
                Ok(h) if !h.0.is_null() => {
                    let ptr = GlobalLock(HGLOBAL(h.0)) as *const u16;
                    if ptr.is_null() {
                        None
                    } else {
                        let mut len = 0usize;
                        while len < 1_000_000 && *ptr.add(len) != 0 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(ptr, len);
                        let s = String::from_utf16_lossy(slice);
                        let _ = GlobalUnlock(HGLOBAL(h.0));
                        Some(s)
                    }
                }
                _ => None,
            }
        }
    };
    let _ = unsafe { CloseClipboard() };
    result
}

/// Synthesizes a Ctrl+C keystroke into the foreground app.
#[cfg(windows)]
fn send_ctrl_c() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_C, VK_CONTROL,
    };

    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_C, false),
        key(VK_C, true),
        key(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent == inputs.len() as u32
}

pub fn debug_log(message: &str) {
    if std::env::var("POCKET_DEBUG").as_deref() == Ok("1") {
        let t = now_ms() % 1_000_000;
        #[cfg(windows)]
        {
            let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            eprintln!(
                "[pocket {:>6}.{:03} t{:x}] {message}",
                t / 1000,
                t % 1000,
                tid
            );
        }
        #[cfg(not(windows))]
        {
            eprintln!("[pocket {:>6}.{:03}] {message}", t / 1000, t % 1000);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Double-shift global hotkey (low-level keyboard hook, Windows)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod double_shift {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LSHIFT, VK_RSHIFT, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, HC_ACTION, HHOOK,
        KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const LLKHF_INJECTED: KBDLLHOOKSTRUCT_FLAGS = KBDLLHOOKSTRUCT_FLAGS(0x10);
    const DOUBLE_SHIFT_WINDOW_MS: u64 = 550;
    const REPEAT_GUARD_MS: u64 = 60;
    /// Reconcile the hook state with the real keyboard often enough to catch
    /// a release even when Windows drops the corresponding low-level event.
    const RELEASE_RECONCILE_MS: u64 = 15;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShiftSide {
        Left,
        Right,
    }

    /// Windows silently drops low-level hooks whose callback misses the
    /// system timeout (e.g. while the process is saturated during startup).
    /// As a safety net the hook is re-registered on this interval; the cost
    /// is a few unprotected keystrokes once every 30 seconds.
    const REHOOK_INTERVAL_SECS: u64 = 30;

    /// Set once when the hook thread starts; per-event diagnostics stay off
    /// unless requested, because any I/O in the callback risks the timeout.
    static HOOK_DEBUG: AtomicBool = AtomicBool::new(false);
    static LAST_EVENT_MS: AtomicU64 = AtomicU64::new(0);
    static LEFT_RELEASE_OBSERVED: AtomicBool = AtomicBool::new(true);
    static RIGHT_RELEASE_OBSERVED: AtomicBool = AtomicBool::new(true);

    fn debug_enabled() -> bool {
        std::env::var("POCKET_DEBUG").as_deref() == Ok("1")
    }

    /// When set, injected key events are allowed to trigger the double-shift
    /// detection. Used only for automated end-to-end tests — real keyboards
    /// never need this, and it must stay off in normal runs so that games and
    /// macros cannot open the capture window.
    fn e2e_keys_enabled() -> bool {
        std::env::var("POCKET_E2E_KEYS").as_deref() == Ok("1")
    }

    struct PressState {
        last_shift_down_ms: u64,
        intervening_key: bool,
        /// True once Shift has been released since the previous Shift press.
        /// Starts true so the very first press can still begin a pair.
        /// Auto-repeat keydowns arrive with NO intervening key-up, so they
        /// can never look like a second press — this is what makes holding
        /// Shift safe regardless of repeat timing jitter.
        released_since_down: bool,
    }

    impl Default for PressState {
        fn default() -> Self {
            Self {
                last_shift_down_ms: 0,
                intervening_key: false,
                released_since_down: true,
            }
        }
    }

    struct HookState {
        app: AppHandle,
        left: PressState,
        right: PressState,
    }

    thread_local! {
        static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    }

    pub fn spawn(app: AppHandle) {
        // Install the hook only after the app has settled: during webview
        // initialization the process is saturated and the first slow callback
        // can get the low-level hook silently dropped.
        let delay_ms: u64 = std::env::var("POCKET_HOOK_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        std::thread::Builder::new()
            .name("double-shift-hook".into())
            .spawn(move || {
                start_rehook_watchdog();
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                unsafe { run_hook(app) }
            })
            .expect("failed to spawn keyboard hook thread");
    }

    unsafe fn install_hook() -> HHOOK {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0).unwrap_or_default()
    }

    unsafe fn run_hook(app: AppHandle) {
        STATE.with(|s| {
            *s.borrow_mut() = Some(HookState {
                app,
                left: PressState::default(),
                right: PressState::default(),
            });
        });
        HOOK_DEBUG.store(debug_enabled(), Ordering::Relaxed);
        LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
        HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::Relaxed);

        // Supervisor loop: run the blocking message pump; a watchdog posts
        // WM_QUIT periodically, and we then re-register the hook — this heals
        // the "system silently dropped the low-level hook" failure mode.
        loop {
            let hhook = install_hook();
            if hhook.is_invalid() {
                eprintln!(
                    "[pocket] failed to install keyboard hook (getlasterror={:?}); retrying in 2s",
                    std::io::Error::last_os_error()
                );
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            // A hook cycle can end between a Shift down and up. Never carry
            // that half-press into the new hook: it makes the next complete
            // double-shift act only as a state reset and the following one
            // appear to be the first gesture that works.
            let left_released = !is_shift_physically_down(ShiftSide::Left);
            let right_released = !is_shift_physically_down(ShiftSide::Right);
            LEFT_RELEASE_OBSERVED.store(left_released, Ordering::Release);
            RIGHT_RELEASE_OBSERVED.store(right_released, Ordering::Release);
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    state.left = PressState::default();
                    state.right = PressState::default();
                    state.left.released_since_down = left_released;
                    state.right.released_since_down = right_released;
                }
            });
            debug_log("low-level keyboard hook installed");

            let mut msg = MSG::default();
            // GetMessageW returns 0 on WM_QUIT / -1 on error; either way the
            // supervisor re-installs.
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 <= 0 {
                    debug_log(&format!(
                        "GetMessageW returned {:?} (lasterr={:?})",
                        ret.0,
                        std::io::Error::last_os_error()
                    ));
                    break;
                }
            }
            let _ = windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hhook);
            debug_log("keyboard hook cycle ended; reinstalling");
            // Keep this gap tiny: keystrokes landing inside it are invisible
            // to the double-shift detector (a 100ms blind window missed
            // roughly 1 in 250 double-shifts).
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    static HOOK_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn hook_file_log(msg: &str) {
        use std::io::Write;
        let path = std::env::temp_dir().join("pocket-hook-debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "[{}] {msg}", now_ms());
        }
    }

    fn release_observed(side: ShiftSide) -> &'static AtomicBool {
        match side {
            ShiftSide::Left => &LEFT_RELEASE_OBSERVED,
            ShiftSide::Right => &RIGHT_RELEASE_OBSERVED,
        }
    }

    /// Reconciles missed Shift-up events from outside the hook callback and
    /// periodically asks the supervisor to re-register a silently dropped
    /// hook. The physical-state polling also heals a stale held flag before
    /// the user's next gesture arrives.
    fn start_rehook_watchdog() {
        std::thread::Builder::new()
            .name("hook-watchdog".into())
            .spawn(move || {
                let rehook_interval = std::time::Duration::from_secs(REHOOK_INTERVAL_SECS);
                let mut next_rehook = std::time::Instant::now() + rehook_interval;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(RELEASE_RECONCILE_MS));

                    if !is_shift_physically_down(ShiftSide::Left) {
                        LEFT_RELEASE_OBSERVED.store(true, Ordering::Release);
                    }
                    if !is_shift_physically_down(ShiftSide::Right) {
                        RIGHT_RELEASE_OBSERVED.store(true, Ordering::Release);
                    }

                    if std::time::Instant::now() >= next_rehook {
                        next_rehook = std::time::Instant::now() + rehook_interval;
                        let tid = HOOK_THREAD_ID.load(Ordering::Relaxed);
                        debug_log(&format!("watchdog tick -> posting WM_QUIT to t{tid:x}"));
                        if tid != 0 {
                            unsafe {
                                let _ = PostThreadMessageW(
                                    tid,
                                    WM_QUIT,
                                    Default::default(),
                                    Default::default(),
                                );
                            }
                        }
                    }
                }
            })
            .expect("failed to spawn hook watchdog");
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let msg = wparam.0 as u32;
        let is_keydown = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_keyup = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if code as u32 == HC_ACTION && (is_keydown || is_keyup) {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
            // Ignore injected input (games / automation) unless E2E testing.
            let injected = (kb.flags & LLKHF_INJECTED).0 != 0;
            // NOTE: the LL hook reports VK_LSHIFT / VK_RSHIFT for real
            // keyboards — VK_SHIFT alone never matches.
            let shift_side = if kb.vkCode == VK_RSHIFT.0 as u32 {
                Some(ShiftSide::Right)
            } else if kb.vkCode == VK_LSHIFT.0 as u32 || kb.vkCode == VK_SHIFT.0 as u32 {
                // VK_SHIFT is only expected from injected E2E input. Treat it
                // as Left so the established gesture remains testable.
                Some(ShiftSide::Left)
            } else {
                None
            };
            let is_shift = shift_side.is_some();
            if is_shift {
                // Rare enough to be safe for diagnostics; proves whether the
                // callback sees shift events at all. Timestamped so reopen
                // delays can be correlated with hook (re)install cycles.
                hook_file_log(&format!(
                    "shift vk=0x{:02X} {} injected={} now={}",
                    kb.vkCode,
                    if is_keydown { "down" } else { "up" },
                    injected,
                    now_ms()
                ));
            }
            if !injected || e2e_keys_enabled() {
                if HOOK_DEBUG.load(Ordering::Relaxed) {
                    eprintln!(
                        "[pocket:hook] keydown vk=0x{:02X} injected={} shift={}",
                        kb.vkCode, injected, is_shift
                    );
                }
                STATE.with(|s| {
                    if let Some(state) = s.borrow_mut().as_mut() {
                        handle_key(state, shift_side, is_keydown);
                    }
                });
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    /// Pure double-press decision, unit-tested below. A press only counts when
    /// a Shift *release* happened since the previous press — auto-repeat
    /// keydowns (held key, no releases) can never satisfy this.
    fn is_double_press(
        last_down_ms: u64,
        now_ms: u64,
        intervening: bool,
        released_since_down: bool,
    ) -> bool {
        released_since_down
            && last_down_ms > 0
            && !intervening
            && now_ms.saturating_sub(last_down_ms) >= REPEAT_GUARD_MS
            && now_ms.saturating_sub(last_down_ms) <= DOUBLE_SHIFT_WINDOW_MS
    }

    /// Starts a distinct Shift press, or returns `None` for an auto-repeat.
    /// `physical_release_observed` is the watchdog's fallback for a key-up
    /// event that never reached the low-level hook.
    fn begin_shift_press(
        state: &mut PressState,
        now_ms: u64,
        physical_release_observed: bool,
    ) -> Option<bool> {
        if physical_release_observed {
            state.released_since_down = true;
        }
        if !state.released_since_down {
            return None;
        }

        let double_shift = is_double_press(
            state.last_shift_down_ms,
            now_ms,
            state.intervening_key,
            state.released_since_down,
        );
        state.last_shift_down_ms = now_ms;
        state.intervening_key = false;
        state.released_since_down = false;
        Some(double_shift)
    }

    fn handle_key(state: &mut HookState, shift_side: Option<ShiftSide>, is_keydown: bool) {
        let Some(side) = shift_side else {
            if is_keydown {
                state.left.intervening_key = true;
                state.right.intervening_key = true;
            }
            return;
        };

        let side_state = match side {
            ShiftSide::Left => &mut state.left,
            ShiftSide::Right => &mut state.right,
        };
        if !is_keydown {
            // Shift released: the next press is a genuinely new press.
            side_state.released_since_down = true;
            release_observed(side).store(true, Ordering::Release);
            return;
        }
        let ms = now_ms();
        let elapsed = ms.saturating_sub(side_state.last_shift_down_ms);
        let intervening = side_state.intervening_key;
        let physical_release_observed = release_observed(side).swap(false, Ordering::AcqRel);
        let recovered_missed_release = !side_state.released_since_down && physical_release_observed;
        let released_since_down = side_state.released_since_down || physical_release_observed;
        let Some(double_shift) = begin_shift_press(side_state, ms, physical_release_observed)
        else {
            // Shift is being held down (OS auto-repeat): not a new press.
            if HOOK_DEBUG.load(Ordering::Relaxed) {
                eprintln!("[pocket:hook] shift repeat ignored (held, no release yet)");
            }
            return;
        };
        if recovered_missed_release {
            hook_file_log(&format!(
                "recovered missed {side:?} Shift release before down now={ms}"
            ));
        }
        if HOOK_DEBUG.load(Ordering::Relaxed) {
            eprintln!(
                "[pocket:hook] shift down elapsed={elapsed}ms window_open={} intervening={}",
                elapsed <= DOUBLE_SHIFT_WINDOW_MS,
                intervening
            );
        }
        // Timestamped decision trace: lets a delayed reopen be diagnosed as
        // guard-rejected, window-expired, intervening-key-cancelled, or a
        // held-shift repeat — the four ways a press can silently no-op.
        hook_file_log(&format!(
            "decision {side:?} elapsed={elapsed} guard={} window={} intervening={} released={} -> double={double_shift} now={ms}",
            elapsed >= REPEAT_GUARD_MS,
            elapsed <= DOUBLE_SHIFT_WINDOW_MS,
            intervening,
            released_since_down,
        ));

        // A press of the other Shift key breaks that side's pair, preventing
        // Left-then-Right from being interpreted as a double press.
        match side {
            ShiftSide::Left => state.right.intervening_key = true,
            ShiftSide::Right => state.left.intervening_key = true,
        }

        if double_shift {
            hook_file_log(&format!("trigger {side:?} hold-watch now={}", now_ms()));
            try_trigger(&state.app, side);
        }
    }

    /// True while the requested physical Shift key is held down. Polled from
    /// watcher threads, never from inside the low-level hook callback.
    fn is_shift_physically_down(side: ShiftSide) -> bool {
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LSHIFT, VK_RSHIFT,
            };
            let key = match side {
                ShiftSide::Left => VK_LSHIFT,
                ShiftSide::Right => VK_RSHIFT,
            };
            (GetAsyncKeyState(key.0 as i32) as u16 & 0x8000) != 0
        }
    }

    fn try_trigger(app: &AppHandle, side: ShiftSide) {
        let shared = app.state::<super::AppFlags>();
        if shared.gaming.load(Ordering::Relaxed) {
            debug_log("double-shift suppressed (gaming mode)");
            return;
        }
        debug_log(&format!("DOUBLE {side:?} SHIFT detected"));
        // A double-Shift tap always saves the selected text directly. (The
        // old hold-to-record gesture was removed: recording has no hotkey.)
        // Never touch window APIs from inside the hook callback: dispatch
        // the save to a worker thread so the callback returns instantly (a
        // slow callback gets the hook removed by the system). A worker — not
        // the main thread — is important: the save ends by creating the HUD
        // webview window, and a window created from inside a
        // run_on_main_thread task builds but never paints.
        let handle = app.clone();
        std::thread::Builder::new()
            .name("hotkey-text-save".into())
            .spawn(move || {
                let shared = handle.state::<super::AppFlags>();
                if shared.gaming.load(Ordering::Relaxed) {
                    debug_log("double-shift suppressed after gaming mode enabled");
                    return;
                }
                super::save_text_capture_from_hotkey(&handle);
            })
            .ok();
    }

    #[cfg(test)]
    mod hook_tests {
        use super::{
            begin_shift_press, is_double_press, PressState, DOUBLE_SHIFT_WINDOW_MS,
            REPEAT_GUARD_MS,
        };

        #[test]
        fn two_quick_distinct_presses_trigger() {
            // Second press 150ms after the first, with a release in between.
            assert!(is_double_press(1000, 1150, false, true));
        }

        #[test]
        fn held_shift_repeat_never_triggers() {
            // Same timing, but no release happened between presses.
            assert!(!is_double_press(1000, 1150, false, false));
            // Even far apart in time, without a release it must not fire.
            assert!(!is_double_press(
                1000,
                1000 + DOUBLE_SHIFT_WINDOW_MS,
                false,
                false
            ));
        }

        #[test]
        fn missed_key_up_is_recovered_before_the_next_gesture() {
            let mut state = PressState::default();

            assert_eq!(begin_shift_press(&mut state, 1000, true), Some(false));
            // The low-level key-up is missing, but the physical-state watcher
            // observed that Shift was released while the user worked elsewhere.
            assert_eq!(begin_shift_press(&mut state, 5000, true), Some(false));
            state.released_since_down = true;
            assert_eq!(begin_shift_press(&mut state, 5150, true), Some(true));
        }

        #[test]
        fn auto_repeat_is_still_ignored_without_a_physical_release() {
            let mut state = PressState::default();

            assert_eq!(begin_shift_press(&mut state, 1000, true), Some(false));
            assert_eq!(begin_shift_press(&mut state, 1150, false), None);
        }

        #[test]
        fn too_slow_or_first_press_does_not_trigger() {
            // Outside the double-tap window.
            assert!(!is_double_press(
                1000,
                1000 + DOUBLE_SHIFT_WINDOW_MS + 1,
                false,
                true
            ));
            // Faster than humanly possible (repeat guard).
            assert!(!is_double_press(
                1000,
                1000 + REPEAT_GUARD_MS - 1,
                false,
                true
            ));
            // First press ever.
            assert!(!is_double_press(0, 1150, false, true));
        }

        #[test]
        fn intervening_key_cancels_the_pair() {
            assert!(!is_double_press(1000, 1150, true, true));
        }
    }
}

#[cfg(not(windows))]
pub mod double_shift {
    use tauri::AppHandle;
    pub fn spawn(_app: AppHandle) {}
}
