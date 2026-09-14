This app's icon assets. All are compiled directly into the exe (via
`include_bytes!` in `aumid.rs`/`gui.rs`, and via `build.rs` for `icon.ico`),
so nothing extra needs to ship alongside the binary — but that also means
**a full rebuild is required** after replacing any of these files.

- `icon.png` — the app icon: window/taskbar icon while running, and (written
  out to a per-user cache dir at startup, since Windows toasts need a real
  file path) the icon shown on toast notifications
- `icon.ico` — the same icon as a real multi-resolution `.ico`, needed
  because embedding an exe resource icon (and the AUMID Start Menu shortcut's
  icon) requires that format specifically; regenerate it from `icon.png` if
  you change the art (16/32/48/256px)
- `tray-green.png` / `tray-orange.png` / `tray-red.png` — the system tray
  icon for each state (connected & all online / a UPS on battery /
  unreachable), resized to 32×32 at startup
