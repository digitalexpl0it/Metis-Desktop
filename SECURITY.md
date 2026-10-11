# Security

Metis is a same-user desktop environment. A process running as your UID can
drive the session by design; the control plane is not a sandbox against
same-UID malware. Cross-user and capability boundaries are enforced where they
matter.

For the full product description see the [User Guide](docs/USER_GUIDE.md). This
file is the short map for auditors and contributors.

## Trust model (IPC)

| Boundary | Behaviour |
|----------|-----------|
| Runtime dir | `$XDG_RUNTIME_DIR/metis/` — mode `0700`; fails closed if `XDG_RUNTIME_DIR` is unset (no `/tmp/metis`) |
| One-shot secrets / captures | Password files, VPN `passwd-file`s, and portal screenshot PNGs require `XDG_RUNTIME_DIR` (no `/tmp` fallback); pkexec password take only under `/run/user/` |
| Sockets / command files | Mode `0600` |
| Accept path | Linux `SO_PEERCRED` — peer UID must match the compositor euid (`metis_protocol::accept_same_euid`) |
| Widgets process | Spawn-scoped `METIS_IPC_TOKEN` → **widgets** capability only (no EndSession, input inject, capture overlays); cleared when the widgets process exits (no wall-clock TTL) |
| Command files | Verb allowlist + 512-byte cap (`parse_runtime_command`); same-UID poke channel, weaker than socket+token |
| Rate limits | Sliding 1s windows on command IPC, event subscribe, and command-file write/dispatch (Phase 18 B) — bounds same-UID spam, not a sandbox |
| Event bus | Cap on long-lived subscribers (16) |
| Session lock | Rejects focus / launch / clipboard / capture / workspace / session-control and remote-input inject until unlock; PAM `PAM_DISALLOW_NULL_AUTHTOK`; empty submit only with biometric cues |
| SetClipboard images | `image_path` must canonicalize under `$XDG_RUNTIME_DIR` or Metis state/cache roots |

Details: [User Guide — Session IPC trust model](docs/USER_GUIDE.md#session-ipc-trust-model),
[Ubuntu/dev notes](docs/UBUNTU_DEV.md#ipc-trust-model).

## X11 / XWayland

- Native Wayland clients are not keylogged or buffer-scraped by X11 clients.
- Default: one shared XWayland (`config.json` → `"xwayland_mode": "shared"`). Classic X11↔X11 risks remain among X11 apps.
- Opt-in: `"xwayland_mode": "isolated"` (Settings → Gaming) — **soft** two-bucket
  policy (Phase 18 D): Metis-spawned gaming-class launches get a separate
  `DISPLAY` (lazy-spawned). Routing is independent of GPU/battery offload.
  Residual risk: X11↔X11 **inside** each bucket; same-UID processes can still
  open either X socket; clipboard may bridge via the Wayland seat. Flatpak X11
  is not a separate enforceable domain.
- Abstract X11 socket default-off (`"xwayland_abstract_socket": false`).
- Metis does **not** claim XSECURITY or true per-app / per-sandbox XWayland
  isolation.

Details: [User Guide — Window management](docs/USER_GUIDE.md#5-window-management).

## Gaming / Flatpak overrides

- `metis-gamingd` / Flatpak optimize uses `Command::new("flatpak").args(...)` with a fixed app/flag allowlist — no shell interpolation of untrusted strings.
- Optimize requires explicit consent (`optimize-gaming yes` / Settings dialog).
- `gaming-flatpak.json` is a ledger of applied overrides, not a command script source.
- **`extra_steam_paths`** (in `gaming.json`) are fail-closed: `~` expands via `$HOME` only,
  paths are `canonicalize`d, must be directories, and may only resolve under `$HOME`,
  `/mnt`, `/media`, or `/run/media`. Invalid entries are dropped with a warning.
- Flatpak `--env` and `launch-steam` exports allowlist GPU offload keys only
  (`DRI_PRIME`, `__NV_PRIME_RENDER_OFFLOAD`, …); values reject NUL/newlines; shell
  exports use POSIX single-quoting.

Details: [User Guide — Steam & Proton](docs/USER_GUIDE.md#steam-proton--steamos-class-gaming),
[Ubuntu/dev — Gaming](docs/UBUNTU_DEV.md#steam--proton-gaming).

## Colour management (upstream)

`wp_color_management_v1` stays **default-off**. Enable only with
`METIS_COLOR_MGMT=1` for testing. Advertising the global to Chromium/Ozone can
abort the session due to an upstream wayland-rs **server/sys** `ObjectData` UAF.
Hardware ICC / LUT / HDR do **not** need that env var.

Tracking: [docs/upstream/README.md](docs/upstream/README.md),
[wayland-rs ObjectData UAF](docs/upstream/wayland-rs-server-objectdata-uaf.md).

## Metis Remote (RUDP)

Quinn / TLS host in the compositor (`rudp_host`); clients use TOFU
(`known_hosts`) + PAM allowlist (`rudp.json` `allowed_users`). Passwords are
never stored in config. Host behaviour:

| Control | Behaviour |
|---------|-----------|
| Auth rate limit | Per peer IP: 5 failures / 60s → 60s lockout; emits `rate_limited`; exponential backoff on fails; caps on in-flight auths and authenticated sessions |
| Unlock on connect | `unlock_session_on_auth` (default on) — session owner only; never clears `ext-session-lock` |
| Stream while locked | Video/audio paused while Metis PAM or protocol lock is active; input/clipboard already gated |
| LAN fail-closed | With `lan_only`, host does not bind until `firewall_applied`; Settings applies firewall before enabling |
| Empty allowlist | Host refuses to start |

Client contract: [docs/RUDP_CLIENT.md](docs/RUDP_CLIENT.md). Classic RDP/GRD
pause-on-lock remains separate (`metis-remote pause`).

## Native libraries

GTK, lcms2, OpenSSL/system TLS, libinput, PipeWire, and similar C dependencies
are outside Rust’s safety model — rely on distro security updates
([PACKAGING.md](docs/PACKAGING.md)).

## Reporting

Prefer a private report to the maintainers (GitHub Security Advisories on this
repository when available) for issues that could affect session integrity or
cross-user isolation. Please include Metis version / commit, distro, and
whether the session is nested (winit) or DRM.

## Residual hardening (Phase 18)

Phase 15 (product security) and Phase 16 (engineering hardening: CI, deny,
trust-boundary tests, panic triage, command-file allowlist) are shipped — see
[`TODO.md`](metis-os-workspace/TODO.md) and [`CHANGELOG.md`](CHANGELOG.md).

Tracked as **[Phase 18](metis-os-workspace/TODO.md#phase-18--security-polish-ipc-dos-isolation-stretch)**:

1. ~~Sanitize gaming config path/env edges (`extra_steam_paths`, launcher exports).~~ **Done** (Phase 18 A).
2. ~~IPC sliding-window rate limits (same-UID spam / DoS); optional token TTL docs.~~ **Done** (Phase 18 B — spawn-scoped tokens documented; no wall-clock TTL).
3. ~~Widget pack JSON schema validation at startup (fail closed).~~ **Done** (Phase 18 C — serde-strict + layout/helper gate; author doc `docs/WIDGET_PACK_SCHEMA.md`).
4. ~~True per-app / per-sandbox rootless XWayland (beyond the two-bucket prototype).~~ **Done as soft policy** (Phase 18 D — class-based lazy gaming bucket + Settings; not an enforceable sandbox).
5. Default-on colour management after a wayland-rs **server/sys** fix (no local
   ObjectData UAF workaround in-tree).
6. ~~GLES `MultiRenderer` compositor stretch~~ **Done** (Wave A/B/C 2026-10-03 —
   hybrid MultiRenderer with SSD/blur/HDR/LUT; see Urgent #1).
