# ups-monitor

A small Rust system for watching APC UPS units and alerting other PCs when the
power goes out — so you can shut machines down before the batteries die.

Two binaries:

- **`ups-server`** — runs on the PC in the machine room that has the UPS units
  plugged in over USB. Polls up to 4 APC UPSes via USB HID, caches their
  status, and serves it over a tiny local HTTP API. Has a system tray icon
  (green = all online, amber = a UPS on battery) and a small status window.
- **`ups-client`** — runs on PCs in other rooms. Polls the server every few
  seconds and pops native Windows toasts on power transitions. Tray icon shows
  both UPS health and server connectivity; status window lists every UPS plus
  a "Connected / Unreachable" line.

## Alert tiers (client)

| Tier | Trigger | Toast |
|------|---------|-------|
| 1 | Any UPS goes on battery | "UPS On Battery" — critical urgency, stays on screen |
| 2 | That UPS drops below `low_battery_threshold_pct` while on battery | "SHUT DOWN NOW — UPS Critical" with battery % and runtime — critical urgency, stays on screen |
| — | Power restored | Quiet recovery toast (auto-dismisses) |
| — | Server unreachable / recovered | Connectivity toasts |

## Building

```
cargo build --release
```

Binaries land in `target/release/`.

## Running the server (machine room)

1. Copy `ups-server/config.example.toml` to `config.toml` **next to
   `ups-server.exe`** and fill in your UPS serials. To discover serials, run
   the server once without mock mode — it logs every APC UPS it finds.
2. Run `ups-server.exe`. A green tray icon appears; right-click → "Show
   status" for the window.
3. The API listens on `http://<bind_ip>:<port>/status` (default port 8420).

### Mock mode (no hardware required)

```
ups-server.exe --mock                  # all units healthy
ups-server.exe --mock --mock-fail rack1-a   # rack1-a starts on battery
```

Mock failing units drain 1–3% per 2-second tick (deliberately fast) so the
client's Tier-2 threshold crossing is observable within a minute or two — this
is the way to test that clients alert correctly end to end.

## Running the client (other rooms)

1. Copy `ups-client/config.example.toml` to `config.toml` next to
   `ups-client.exe`, point `server.address` at the machine-room PC.
2. Run `ups-client.exe`. Tray icon: green = connected & all online, amber = a
   UPS on battery, red = server unreachable.

## End-to-end alert test

1. On the server PC: `ups-server.exe --mock --mock-fail rack1-a`
2. On any PC (can be the same one): `ups-client.exe` with
   `server.address = "127.0.0.1"`
3. Within one poll interval: Tier-1 "UPS On Battery" toast, tray turns amber.
4. Within ~1–2 minutes (as the mock battery drains past 20%): Tier-2
   "SHUT DOWN NOW" toast with live battery % and runtime.
5. Kill the server: after `unreachable_after_missed` failed polls, a
   "server unreachable" toast and the tray turns red. Restart it: recovery
   toast, tray back to green/amber.

## Deployment

Both apps are designed to auto-start at logon via Task Scheduler:

- Action: the exe path; **Start in**: the folder containing the exe (so
  `config.toml` is found).
- Trigger: "At log on" of the relevant user.
- No console window is shown; everything lives in the tray.

## Notes & non-goals

- Toasts use `notify-rust` (toast-notify on Windows). The notification code is
  isolated in `ups-client/src/notify.rs` as the single swap-in point if you
  later want native WinRT toasts with reminders/alarms.
- HID decoding targets APC Smart-UPS feature reports (usage page 0x84). If a
  unit reports garbage, the server falls back to "Unknown" for that UPS and
  keeps serving the others. NUT's `upsc` is the documented escape hatch but is
  not required.
- Not a shutdown orchestrator: the client *tells you* to shut down; it doesn't
  do it for you.
