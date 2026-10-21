# Decision: Metis-owned Secret Service provider

**Status:** accepted  
**Date:** 2026-10-07  
**Updated:** 2026-10-21  
**Related:** [`metis-secrets`](../../metis-os-workspace/metis-secrets/),
[`metis-secretsd`](../../metis-os-workspace/metis-secretsd/),
[UBUNTU_DEV.md](../UBUNTU_DEV.md) (Keyring section)

## Question

Should Metis depend on a desktop-environment keyring (GNOME Keyring, KWallet,
…), remain a bare client that warns when none is installed, or own a
session-default Secret Service provider?

## Decision

**Metis owns the default session provider** via `metis-secretsd`, which
implements `org.freedesktop.secrets` on the user bus.

- **Contract:** freedesktop Secret Service API. Apps (including Metis via
  `metis-secrets` / `oo7`, and third-party libsecret clients) talk D-Bus — not a
  Metis-private store.
- **Default:** session start launches `metis-secretsd` when the bus name is free.
- **Compatible:** if another provider already owns `org.freedesktop.secrets`
  (KeePassXC, KWallet, pass-secret-service, gnome-keyring, …), leave it alone.
- **Packaging:** D-Bus activation lives at
  `/usr/share/metis/dbus-1/services/org.freedesktop.secrets.service` (and
  `metis-session` prepends `/usr/share/metis` to `XDG_DATA_DIRS`). Do **not**
  install into `/usr/share/dbus-1/services/` — that path is owned by
  `gnome-keyring` on Ubuntu/Debian and breaks package upgrades.
- **At rest:** DEK wraps with Argon2id(login password) in `vault.wrap`; unlock via
  `org.metis.Secrets1.UnlockWithPassword` (PAM). Legacy plaintext `vault.key` is
  migrated on first unlock. Shell prompts at startup when locked.
- **Portal Secret:** `metis-portal` implements
  `org.freedesktop.impl.portal.Secret` and is pinned in `metis-portals.conf`;
  per-app keys live in Session SS (`portal_secret` kind).

## Residuals

- SSH agent (not required for Metis credentials; optional gnome-keyring
  `--components=ssh` fallback remains)
- Full Secret Service feature parity (extra aliases, Prompt UX, ACLs)
- Re-wrap after login password change (user must unlock with old wrap password
  until a rewrap path lands)

## Alternatives considered

| Option | Why not |
|--------|---------|
| Prefer gnome-keyring | Couples Metis to a GNOME stack; conflicts with DE-agnostic goal |
| Client-only + warn | Browsers/apps fall back to plaintext without a provider |
| Metis-private vault only | Breaks libsecret / other apps; TODO forbids private-only store |
