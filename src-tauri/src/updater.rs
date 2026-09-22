//! In-app self-update.
//!
//! Downloads the matching installer asset from the *latest published GitHub
//! release*, launches it, and exits so the installer can replace the app.
//!
//! Design notes:
//! - The frontend decides *whether* an update exists (GitHub releases API
//!   version check, see MainWindow) and what the menu shows; this module only
//!   does the mechanical part — fetch, stream, run — so the UX (no progress
//!   bar, a simple "Downloading…" state) stays entirely in the frontend.
//! - Downloads stream to a temp file with progress events emitted per chunk,
//!   but the frontend only listens for the terminal `update-install-ready`
//!   event; the interim progress events are cheap and make future progress
//!   UI possible without another protocol change.
//! - The app hands control to the installer and exits: a small watcher
//!   process waits for the installer to finish and then starts the freshly
//!   installed exe, so the update is seamless — no manual relaunch. The
//!   single-instance plugin would bounce a second launch to the old process,
//!   which is exactly why this process exits (early, to release the exe lock
//!   before the installer reaches the file-replacement step).

use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};

const GITHUB_LATEST_API: &str = "https://api.github.com/repos/emadmohagheghi/Pocket/releases/latest";
const USER_AGENT: &str = "pocket-updater";

/// Command-line marker passed to the freshly installed exe when it is
/// relaunched after a self-update. The new process detects it (lib.rs) and
/// guarantees the main window ends up visible — an installer-relaunched
/// Pocket must never surface as a tray-only ghost.
pub const POST_UPDATE_LAUNCH_MARKER: &str = "--pocket-updated";

/// Platform-specific asset suffix for the bundle the release workflow builds.
/// Windows uses the NSIS `x64-setup.exe`; Linux uses the amd64 `.deb`.
#[cfg(target_os = "windows")]
const ASSET_SUFFIXES: [&str; 2] = ["x64-setup.exe", "setup.exe"];
#[cfg(not(target_os = "windows"))]
const ASSET_SUFFIXES: [&str; 2] = ["amd64.deb", "AppImage"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total size in bytes, when known (`Content-Length`).
    pub total: Option<u64>,
}

/// Download the current latest release's installer for this platform, emit
/// `update-download-progress` while streaming, run it, and exit the app so
/// the installer can replace the running executable. Async so the blocking
/// download never freezes the main thread.
#[tauri::command]
pub async fn install_and_launch_update(app: AppHandle) -> AppResult<()> {
    let json: serde_json::Value = http_get_json(GITHUB_LATEST_API)?;

    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| AppError::Invalid("release response missing tag_name".into()))?
        .to_string();
    let assets = json["assets"]
        .as_array()
        .ok_or_else(|| AppError::Invalid("release response missing assets".into()))?;

    // Pick the asset whose name ends with the right suffix for this platform.
    let asset_url = assets
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            ASSET_SUFFIXES
                .iter()
                .find(|suffix| name.ends_with(*suffix))
                .map(|_| url.to_string())
        })
        .next()
        .ok_or_else(|| {
            AppError::Invalid(format!(
                "no installer asset found for this platform in release {tag}"
            ))
        })?;

    let download = download_to_temp(&app, &asset_url)?;
    spawn_installer(&download)?;

    // The watcher spawned with the installer relaunches the freshly
    // installed exe once the install finishes; single-instance hands the
    // new launch to us, so this process must exit for the new one to take
    // over. Exit quickly: the exe file lock must be released before the
    // installer's file-replacement step reaches pocket.exe.
    let _ = app.emit("update-install-ready", ());
    let exit_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        exit_app.exit(0);
    });
    Ok(())
}

/// GET a URL as JSON with a proper UA (GitHub API requires one).
fn http_get_json(url: &str) -> AppResult<serde_json::Value> {
    let agent = build_agent()?;
    let res = agent
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| AppError::Storage(format!("update check failed: {e}")))?;
    if res.status() != 200 {
        return Err(AppError::Storage(format!(
            "update check failed: HTTP {}",
            res.status()
        )));
    }
    let body = res
        .into_string()
        .map_err(|e| AppError::Storage(format!("update check failed: {e}")))?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| AppError::Storage(format!("update check failed: {e}")))?;
    Ok(json)
}

fn build_agent() -> AppResult<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build())
}

/// Stream the installer into a temp file, emitting progress events.
fn download_to_temp(app: &AppHandle, url: &str) -> AppResult<std::path::PathBuf> {
    let agent = build_agent()?;
    let res = agent
        .get(url)
        .call()
        .map_err(|e| AppError::Storage(format!("update download failed: {e}")))?;

    let total = res
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = res.into_reader();

    // Parse the file name from the URL so the installer extension is right.
    let file_name = url
        .rsplit('/')
        .next()
        .unwrap_or("pocket-setup.exe")
        .to_string();
    let temp_dir = std::env::temp_dir().join("pocket-update");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| AppError::Io(std::io::Error::new(e.kind(), e.to_string())))?;
    let dest = temp_dir.join(&file_name);
    // Remove a stale partial from an earlier attempt.
    let _ = std::fs::remove_file(&dest);

    let mut file = std::fs::File::create(&dest)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| AppError::Storage(format!("update download failed: {e}")))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;
        let _ = app.emit(
            "update-download-progress",
            UpdateProgress {
                downloaded,
                total,
            },
        );
    }
    std::io::Write::flush(&mut file)?;
    drop(file);

    if let Some(total) = total {
        if downloaded < total {
            return Err(AppError::Storage(format!(
                "download incomplete: got {downloaded} of {total} bytes"
            )));
        }
    }
    Ok(dest)
}

/// Run the installer detached from this process, and arm a relaunch watcher:
/// Windows uses a hidden PowerShell that waits for the installer to exit and
/// then starts the freshly installed exe (the per-user NSIS install replaces
/// it in place), so Pocket reopens by itself after the update. The installer
/// is also given `/R /ARGS <marker>` — its own restart path — so even if the
/// watcher fails, the silent installer relaunches the app with the
/// post-update visibility marker. `/NS` keeps the update hands-off: the
/// NSIS template would otherwise force-create a desktop shortcut for every
/// silent install (the installer hooks in nsis-hooks.nsh are the second
/// line of defense for installers run by *older* app versions).
#[cfg(target_os = "windows")]
fn spawn_installer(path: &std::path::Path) -> AppResult<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let installer = std::process::Command::new(path)
        // /S silent install, /NS create no shortcuts, /R restart app afterwards
        .args(["/S", "/NS", "/R", "/ARGS", POST_UPDATE_LAUNCH_MARKER])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| AppError::Storage(format!("could not launch installer: {e}")))?;
    spawn_relaunch_watcher_windows(installer.id());
    Ok(path.display().to_string())
}

/// Hidden PowerShell watcher: wait for the installer PID, then start the app
/// exe again. The path is the *currently running* exe — the installer swaps
/// that exact file, so this launches the new build. Watcher-spawn failure is
/// non-fatal: the installer still runs and the user can reopen Pocket.
#[cfg(target_os = "windows")]
fn spawn_relaunch_watcher_windows(installer_pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(app_exe) = std::env::current_exe() else {
        return;
    };
    let script = format!(
        "Wait-Process -Id {} -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 800; \
         Start-Process -FilePath '{}' -ArgumentList '{}'",
        installer_pid,
        app_exe.display(),
        POST_UPDATE_LAUNCH_MARKER
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Linux: `.deb` installs via pkexec (prompts for the user's password); a
/// shell watcher relaunches `pocket` once the install finishes, mirroring the
/// Windows behavior. AppImage self-replacement is out of scope.
#[cfg(not(target_os = "windows"))]
fn spawn_installer(path: &std::path::Path) -> AppResult<String> {
    if path.extension().and_then(|e| e.to_str()) == Some("deb") {
        let installer = std::process::Command::new("pkexec")
            .args(["apt", "install", "-y"])
            .arg(path)
            .spawn()
            .map_err(|e| AppError::Storage(format!("could not launch installer: {e}")))?;
        // Watcher: wait for pkexec to exit, then start the new build with
        // the post-update visibility marker (mirrors the Windows flow).
        let script = format!(
            "while kill -0 {} 2>/dev/null; do sleep 0.5; done; sleep 1; \
             nohup pocket {} >/dev/null 2>&1 &",
            installer.id(),
            POST_UPDATE_LAUNCH_MARKER
        );
        let _ = std::process::Command::new("sh").arg("-c").arg(&script).spawn();
        return Ok(path.display().to_string());
    }
    // AppImage fallback: chmod +x and tell the user where it is; AppImage
    // self-replacement is complex.
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(path)
        .status()
        .map_err(|e| AppError::Storage(format!("could not prepare AppImage: {e}")))?;
    Err(AppError::Invalid(
        "AppImage updates must replace the AppImage file manually".into(),
    ))
}
