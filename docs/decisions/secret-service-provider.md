# Decision: Metis-owned Secret Service provider

**Status:** accepted  
**Date:** 2026-10-07  
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
- **No DE pin:** `metis-portals.conf` does not force
  `org.freedesktop.impl.portal.Secret=gnome-keyring`. Portal Secret remains a
  residual (sandboxed file-backend encrypt key); session SS is what unsandboxed
  clients use today.

## Residuals

- PAM / login-password unlock for the vault (MVP uses a session-local master key
  file mode `0600` under `$XDG_DATA_HOME/metis/secrets/`)
- `org.freedesktop.impl.portal.Secret` in `metis-portal`
- SSH agent (not required for Metis credentials)
- Full Secret Service feature parity (extra aliases, prompt UX, ACLs)

## Alternatives considered

| Option | Why not |
|--------|---------|
| Prefer gnome-keyring | Couples Metis to a GNOME stack; conflicts with DE-agnostic goal |
| Client-only + warn | Browsers/apps fall back to plaintext without a provider |
| Metis-private vault only | Breaks libsecret / other apps; TODO forbids private-only store |
