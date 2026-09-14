//! Registers this app's own AppUserModelID so Windows toasts show
//! "UPS Monitor Client" as the sender instead of "Windows PowerShell".
//!
//! Background: `notify.rs` sends toasts under `Toast::POWERSHELL_APP_ID`
//! because Windows silently drops toasts from an AUMID that isn't registered
//! (see the comment there) — registration means "a Start Menu shortcut whose
//! target exe has this AUMID set via the shell's property store". Once that
//! shortcut exists, toasts can be sent under our *own* AUMID and Windows will
//! actually show them, attributed to whatever the shortcut is named.
//!
//! This runs once at startup and is entirely best-effort: if it fails for any
//! reason we fall back to the (correctly-functioning, just generically
//! labelled) PowerShell identity rather than losing toasts altogether.

use std::path::PathBuf;

/// This app's AppUserModelID. Toasts are sent under this id once
/// registration succeeds.
pub const APP_ID: &str = "UpsMonitor.Client";
/// Also the Start Menu shortcut's file name (sans extension), which is what
/// Windows displays as the toast sender.
const DISPLAY_NAME: &str = "UPS Monitor Client";

use std::sync::atomic::{AtomicBool, Ordering};

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Idempotent: safe to call every startup.
pub fn ensure_registered() {
    let ok = match imp::try_register() {
        Ok(()) => {
            tracing::info!(app_id = APP_ID, "AppUserModelID registered for toasts");
            true
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not register AppUserModelID; toasts will show as \"Windows PowerShell\""
            );
            false
        }
    };
    REGISTERED.store(ok, Ordering::Relaxed);
}

/// Whether `ensure_registered` succeeded this run — `notify.rs` uses this to
/// decide which AUMID to send toasts under, since sending under an
/// unregistered one makes Windows silently drop them.
pub fn is_registered() -> bool {
    REGISTERED.load(Ordering::Relaxed)
}

/// The app icon, embedded in the exe so there's nothing extra to deploy.
/// `gui.rs` decodes these bytes directly for the window/taskbar icon; the
/// toast icon and the AUMID shortcut's icon (below) need an actual file on
/// disk instead (WinRT toast XML and `IShellLinkW::SetIconLocation` both
/// take a path, not bytes), so those write this out to a per-user cache dir
/// once at startup.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
const ICON_ICO: &[u8] = include_bytes!("../assets/icon.ico");

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    Some(PathBuf::from(base).join("UpsMonitorClient"))
}

/// Writes `bytes` to `<cache_dir>/<name>`, overwriting any existing file
/// (cheap — a few KB, once per process start), and returns the path.
fn write_cached(name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// The app icon as a real file, for the toast icon override in `notify.rs`.
pub fn toast_icon_path() -> Option<PathBuf> {
    write_cached("icon.png", ICON_PNG)
}

/// The app icon as a real .ico file — Explorer shell shortcuts (unlike
/// toasts or egui) only accept `.ico`/`.exe`/`.dll` as an icon source, so the
/// Start Menu shortcut registered for AUMID purposes needs this specifically.
fn shortcut_icon_path() -> Option<PathBuf> {
    write_cached("icon.ico", ICON_ICO)
}

#[cfg(windows)]
mod imp {
    use super::{shortcut_icon_path, APP_ID, DISPLAY_NAME};
    use anyhow::{Context, Result};
    use windows::core::{Interface, PCWSTR, PWSTR};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Build a `VT_LPWSTR` PROPVARIANT. The caller must not free the string
    /// separately — `PropVariantClear` (or dropping via `windows`' Drop impl,
    /// which we don't rely on here) would otherwise double-free it, so we
    /// intentionally leak the allocation: it's a handful of bytes, once per
    /// process start.
    fn string_propvariant(s: &str) -> PROPVARIANT {
        let mut wide = wide(s);
        let ptr = PWSTR(wide.as_mut_ptr());
        std::mem::forget(wide);
        let mut pv = PROPVARIANT::default();
        unsafe {
            let inner = &mut *(&mut pv as *mut PROPVARIANT).cast::<PropVariantLpwstr>();
            inner.vt = VT_LPWSTR.0;
            inner.reserved = [0; 3];
            inner.pwsz = ptr;
        }
        pv
    }

    /// Layout-compatible view of `PROPVARIANT { vt, reserved..., pwszVal }`
    /// for the `VT_LPWSTR` case — the real type is a deeply nested
    /// union-of-unions that's awkward to name field-by-field through Rust's
    /// union syntax, but its `repr(C)` layout is exactly this.
    #[repr(C)]
    struct PropVariantLpwstr {
        vt: u16,
        reserved: [u16; 3],
        pwsz: PWSTR,
    }

    fn shortcut_path() -> Result<std::path::PathBuf> {
        let appdata = std::env::var("APPDATA").context("APPDATA not set")?;
        Ok(std::path::PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{DISPLAY_NAME}.lnk")))
    }

    pub fn try_register() -> Result<()> {
        let exe = std::env::current_exe().context("failed to get current exe path")?;
        let link_path = shortcut_path()?;

        unsafe {
            // Ignore RPC_E_CHANGED_MODE: winit/eframe may already have
            // initialized COM in a different mode on this thread by the
            // time we run — either way COM is usable.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let shell_link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                    .context("CoCreateInstance(ShellLink) failed")?;

            shell_link
                .SetPath(PCWSTR(wide(&exe.to_string_lossy()).as_ptr()))
                .context("IShellLinkW::SetPath failed")?;
            if let Some(dir) = exe.parent() {
                let _ = shell_link.SetWorkingDirectory(PCWSTR(wide(&dir.to_string_lossy()).as_ptr()));
            }
            if let Some(icon) = shortcut_icon_path() {
                let _ = shell_link
                    .SetIconLocation(PCWSTR(wide(&icon.to_string_lossy()).as_ptr()), 0);
            }

            let props: IPropertyStore = shell_link
                .cast()
                .context("IShellLinkW -> IPropertyStore failed")?;
            let pv = string_propvariant(APP_ID);
            props
                .SetValue(&PKEY_AppUserModel_ID, &pv)
                .context("IPropertyStore::SetValue(AppUserModel_ID) failed")?;
            props.Commit().context("IPropertyStore::Commit failed")?;

            let persist: IPersistFile = shell_link
                .cast()
                .context("IShellLinkW -> IPersistFile failed")?;
            persist
                .Save(PCWSTR(wide(&link_path.to_string_lossy()).as_ptr()), true)
                .context("IPersistFile::Save failed")?;
        }

        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn try_register() -> anyhow::Result<()> {
        Ok(())
    }
}
