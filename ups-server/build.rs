//! Embeds `assets/icon.ico` as the .exe's own icon (File Explorer, taskbar,
//! Alt-Tab, shortcuts) if that file exists. Silently does nothing otherwise
//! so the build keeps working before an icon has been designed — see
//! `assets/README.md`.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        if std::path::Path::new("assets/icon.ico").exists() {
            let mut res = winres::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            if let Err(e) = res.compile() {
                println!("cargo:warning=failed to embed assets/icon.ico as the exe icon: {e}");
            }
        } else {
            println!("cargo:warning=assets/icon.ico not found; building with no custom exe icon");
        }
    }
}
