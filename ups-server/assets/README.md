This app's icon assets. All are compiled directly into the exe (via
`include_bytes!` in `gui.rs`, and via `build.rs` for `icon.ico`), so nothing
extra needs to ship alongside the binary — but that also means **a full
rebuild is required** after replacing any of these files.

- `icon.png` — the app icon: window/taskbar icon while running
- `icon.ico` — the same icon as a real multi-resolution `.ico`, needed
  because embedding an exe resource icon requires that format specifically;
  regenerate it from `icon.png` if you change the art (16/32/48/256px)
- `tray-green.png` / `tray-orange.png` / `tray-red.png` — the system tray
  icon for each health state (all online / a UPS on battery / a UPS
  unreachable), resized to 32×32 at startup
