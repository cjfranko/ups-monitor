# UPS Monitor — Build Plan for Coding Agent

## Goal
Two small Rust binaries in one workspace, each with a small status GUI:
1. **`ups-server`** — runs on a PC in the machine room, polls 4 APC UPS units over USB HID, serves their status over a local HTTP API, and shows a small window/tray view of all 4 units' current state.
2. **`ups-client`** — runs on PCs in a separate room, polls the server, fires a native Windows toast the moment any UPS transitions to on-battery, and shows a small window/tray view of the 4 UPS statuses plus whether it's currently able to reach the server.

No cloud, no broker — pure LAN, polling-based, Rust end to end.

---

## Workspace layout
```
ups-monitor/
├── Cargo.toml              # workspace root
├── ups-server/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── hid.rs          # UPS enumeration + HID parsing
│       ├── state.rs        # shared cached status, one entry per UPS
│       ├── api.rs          # axum routes
│       └── gui.rs          # status window/tray showing all 4 UPS states
└── ups-client/
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── poller.rs       # HTTP polling loop + state-transition detection
        ├── notify.rs       # toast firing
        └── gui.rs          # status window/tray: 4 UPS states + server connectivity
```

**GUI approach:** `tray-icon` (already on the plan for the client) plus a small always-available window is the lightest path — right-click tray → "Show status" opens a minimal window listing each UPS and its state, rather than building a persistent on-screen dashboard. `egui`/`eframe` is a reasonable choice for the window itself if a tray-only view (icon color/tooltip) isn't enough detail — keep it to a single simple list view, not a themed dashboard, since this is a utility not a product.

## Phase 1 — Server: UPS discovery
- Add `hidapi` crate. Enumerate connected HID devices, filter to USB HID Power Device class (usage page `0x84`, vendor ID `0x051D` for APC).
- Confirm all 4 units enumerate distinctly and can be opened concurrently without conflict.
- Log each device's serial/path so it can be mapped to a human-readable id (e.g. `rack1-a`) in a config file — don't hardcode ids in source.

**Config file** (`config.toml`, loaded at startup):
```toml
[server]
bind_ip = "192.168.1.50"   # the machine-room PC's LAN address
port = 8420                # configurable — avoid clashing with anything else on the box

[[ups]]
id = "rack1-a"
serial = "AS1234567890"

[[ups]]
id = "rack1-b"
serial = "AS1234567891"
# ...repeat for all 4
```

## Phase 2 — Server: HID parsing + polling loop
- Read the `PresentStatus` HID usage (contains `ACPresent`/`Discharging` bits — the on-battery signal), plus battery percentage and runtime-remaining usages if present.
- Background task per UPS, polling every 1–2s, writing into a shared `Arc<Mutex<HashMap<String, UpsStatus>>>` (or `RwLock`).
- `UpsStatus` struct: `{ id, status: Online | OnBattery | Unknown, battery_pct: Option<u8>, runtime_secs: Option<u32>, last_updated: DateTime }`.
- If HID parsing proves unreliable on any unit's firmware, fall back to shelling out to NUT's `upsc` (document this as a known escape hatch, don't build it preemptively).

## Phase 3 — Server: API
- `axum` app, single main endpoint:
  - `GET /status` → JSON array of all `UpsStatus` entries from the shared state (read-only, no blocking I/O — just reads the cache).
  - `GET /healthz` → simple liveness check.
- Bind to the LAN interface and port from `config.toml` (not `0.0.0.0` blindly — pick the actual local IP or make it configurable), no auth needed for a closed local network but keep it easy to add a shared-secret header later if this ever needs to cross a VLAN boundary.

## Phase 3b — Server: status GUI
- Tray icon (via `tray-icon`) reflecting overall health at a glance: normal icon when all 4 are online, changed icon/badge if any UPS is on battery or unreachable.
- Tray tooltip or a "Show status" menu item opening a small window that lists all 4 configured UPS ids with their current status, battery %, and runtime — reads straight from the same shared state the API serves, no separate polling logic needed.
- This is a local-machine convenience view (confirming the box itself sees all 4 units correctly) — it doesn't need to be reachable remotely, that's what the API is for.

## Phase 4 — Client: polling + transition detection
- `reqwest` (blocking or async, either is fine at this scale) polling `GET /status` on a configurable interval, default 5s.
- Maintain last-known status per UPS id in memory; compare each poll against the previous one.
- On transition `Online → OnBattery`: fire a toast naming the specific UPS id.
- On transition `OnBattery → Online`: fire a lower-priority "back to normal" toast (nice-to-have, confirm if wanted).
- **Two-tier severity, not just one alert level:**
  - Tier 1 — on-battery transition (as above): urgent toast, needs attention but not yet an emergency.
  - Tier 2 — battery percentage crosses below a configurable threshold (default 20%) *while on battery*: this is the "shut down now" case — a distinctly more critical alert (see Phase 5), fired once per crossing, not repeated every poll while it stays below threshold.
  - Track "already alerted for this threshold crossing" per UPS so it doesn't spam a toast every 5 seconds while the battery sits at 15% — re-arm only once the UPS goes back online or battery rises back above threshold.
  - Threshold is configurable in the client's `config.toml`, since what counts as "not enough runtime left" may differ per site/UPS.
- Handle server-unreachable gracefully (log, retry, don't crash — and consider a toast if the server itself goes dark for more than N missed polls, since that's also worth knowing about).
- Track connectivity explicitly, not just implicitly via missed polls: last-successful-poll timestamp, and a simple `Connected | Unreachable` state derived from it (e.g. unreachable after 2 consecutive failed polls) — this is what the GUI's connectivity indicator reads from.

## Phase 5 — Client: toast + status GUI
- `notify-rust` (or `winrt-notification` if more control over toast content is needed) for the actual Windows toast.
- **On-battery alert needs to be loud, not a quiet toast that can be missed:**
  - Use a high-priority/urgent toast scenario if the crate supports it (WinRT toasts have a "reminder"/"alarm" scenario that stays on screen and repeats until dismissed, rather than the default toast that fades after a few seconds and vanishes into Action Center).
  - Pair it with an audible sound — WinRT toasts support a specified sound, or fall back to a manual beep/alert sound if the loud/persistent scenario isn't available in the crate.
  - Consider requiring explicit dismissal (a "Dismiss" button on the toast) rather than letting it auto-clear, so it can't be missed if no one's looking at the screen the moment it fires.
  - The "back to normal" toast can stay a normal, quiet, auto-dismissing one — the urgency only needs to apply to the on-battery transition itself.
- **Critical low-battery alert (below threshold, e.g. 20%) needs to read as more severe than the plain on-battery toast, not the same treatment again:**
  - Same "reminder" scenario as Tier 1 but distinguishable at a glance — different title/wording ("SHUT DOWN NOW" vs "On battery power"), and if the toast API allows it, a different icon/accent so it doesn't blend in with the earlier, less urgent alert.
  - Repeat the alarm sound (loop it, or use the loudest/most attention-grabbing sound option available) rather than a single chime — this tier is meant to be genuinely hard to ignore.
  - Content should be actionable, not just alarming — name the specific UPS, its battery %, and estimated runtime remaining so whoever sees it knows exactly what to shut down and how much time they realistically have.
  - Keep Tier 1 and Tier 2 as separate, independently-firing alerts (a UPS can go on-battery, alert once, then separately cross the 20% threshold and alert again more severely) rather than trying to collapse them into one toast.
- Tray icon (via `tray-icon`) as the persistent presence — icon reflects both UPS state (any on battery?) and server connectivity (can't reach it at all?) so a glance at the tray answers "is everything fine" without opening anything.
- "Show status" window/menu listing: each of the 4 UPS ids with their last-known status, and a clear "Connected to server" / "Unreachable — last seen HH:MM:SS" line so the room's PCs can self-diagnose a dead link to the machine-room box rather than just going quiet.
- Run via Task Scheduler "at logon" trigger so it's always present; decide between tray-only vs. also offering a taskbar window based on whether the room's PCs are attended or headless-ish kiosks.

## Phase 6 — Config + deployment
- Both binaries read a local `config.toml` next to the executable.
  - Server's config carries `bind_ip` + `port` (above) plus the UPS id/serial mapping.
  - Client's config carries the server's address and port, e.g.:
    ```toml
    [server]
    address = "192.168.1.50"
    port = 8420

    [polling]
    interval_secs = 5

    [alerts]
    low_battery_threshold_pct = 20
    ```
  - Keeping the port in config rather than hardcoded means it can be changed on either side without a rebuild — just make sure both sides agree after any change.
- No installer needed initially — a copied folder + Task Scheduler entry is enough for a first pass; revisit packaging (MSI/installer) only if this needs to roll out to many PCs.

## Phase 7 — Testing
- Server: unit test HID parsing against captured sample reports if possible (or mock the `UpsStatus` derivation logic separately from the HID I/O so it's testable without hardware).
- Client: unit test the transition-detection logic (given a sequence of statuses, assert which ones should fire a toast) without needing a live server.
- Manual end-to-end test: pull one UPS's mains plug, confirm toast fires within one polling interval; restore power, confirm recovery toast (if built).

## Explicit non-goals (call these out to the agent so it doesn't over-build)
- No cloud/remote access — LAN only.
- No MQTT/broker — plain HTTP polling is sufficient at this scale.
- No auth/TLS on the API for v1 — flag as a possible future addition, don't implement speculatively.
- No graceful shutdown orchestration (that's PowerChute's job for anything actually plugged into these UPS units) — this is monitoring/alerting only.

## Suggested order of work for the agent
1. Scaffold the workspace and both crates with empty `main.rs` files that compile.
2. Server Phase 1–2 (HID discovery + parsing) — validate against real hardware before building the API on top of unverified data.
3. Server Phase 3 (API) — validate with `curl`/browser against real UPS state.
4. Client Phase 4–5 — validate toasts fire correctly against the now-working server.
5. Config + deployment pass last, once both binaries work standalone.
