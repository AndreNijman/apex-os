# APEX-OS

APEX-OS is a two-edition, atomic Linux distribution built on **Fedora bootc**
(OCI-native, image-based, transactional updates with rollback). It ships
[Brain_Shell](https://github.com/AndreNijman/brain-shell-void) as its native
desktop and is managed by **apexd**, a first-party system daemon.

## Editions

| Edition | Accent | Meaning | Target use |
|---------|--------|---------|------------|
| **APEX-OS Gaming** | Gold | Power | Performance-tuned desktop for gaming |
| **APEX-OS Daily**  | Chartreuse | Everyday | General-purpose daily driver |

Both editions share a common base image and diverge in packages, tuning, and
branding. The "spark" logo is the shared mark; gold and chartreuse colorways
distinguish the editions, with mono (black/white) variants for neutral
contexts. See [docs/branding.md](docs/branding.md).

## Repository layout

| Path | Contents |
|------|----------|
| `Containerfile.base` | Shared base image (bootc) |
| `Containerfile.daily` | Daily edition image (chartreuse) |
| `Containerfile.gaming` | Gaming edition image (gold) |
| `kernel/` | Custom kernel config / build inputs |
| `signing/` | MOK / Secure Boot signing tooling (no private keys) |
| `files/branding/` | Logos, Plymouth boot themes, wallpapers |
| `files/system/` | System-level files baked into the image |
| `files/desktop/` | Desktop / Brain_Shell integration files |
| `files/scripts/` | Build and runtime helper scripts |
| `apexd/` | apexd system daemon source |
| `config/sysprofiles/` | System tuning profiles (gaming/daily) |
| `tests/` | Image and integration tests |
| `docs/` | Project documentation |
| `.github/workflows/` | CI (image build, sign, publish) |

## Status

**M1 — production images + CI.** The three images build locally (shared
`Containerfile.base` → `daily` / `gaming`, with the CachyOS kernel, desktop /
greeter stack, scx, and Bazaar), the DIY NVIDIA akmod builds against the shared
kernel, and `.github/workflows/build-image.yml` builds, cosign-signs (keyless),
and pushes all three to GHCR. See [docs/m1-notes.md](docs/m1-notes.md) for build
results, package drift, and open items (greeter render still needs a GL target).
Earlier: [docs/m0-results.md](docs/m0-results.md) (spikes) and
[docs/experiments.md](docs/experiments.md).
