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

**Pre-M0 planning.** Branding is complete (logos, Plymouth themes for both
editions, default wallpaper). Container images, kernel, apexd, and CI are
scaffolded but **not yet implemented** — image work lands in M1. See
[docs/experiments.md](docs/experiments.md) for the log of exploratory branches.
