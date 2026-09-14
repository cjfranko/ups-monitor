Drop the server's application icon here as `icon.ico` (ideally multi-resolution:
16, 32, 48, 256px). Once present it's automatically used for:

- the compiled .exe's own icon (File Explorer, taskbar, Alt-Tab, shortcuts) —
  embedded at build time by `build.rs`
- the app window's icon while running — loaded at startup by `gui.rs`

Nothing needs to change in the code; both just check for this file and no-op
if it isn't there yet. Re-run `cargo build` after adding/replacing it.
