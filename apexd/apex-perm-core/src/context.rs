//! A Flatpak's sandbox context, which is where most of the real enforcement
//! lives and none of it is negotiable at runtime.
//!
//! `flatpak info --show-permissions <app>` prints a key file. So does an
//! override under `~/.local/share/flatpak/overrides/` or
//! `/var/lib/flatpak/overrides/`, and the override's entries are applied over
//! the manifest's with a `!` prefix meaning "remove". Everything Settings can
//! say about a Flatpak's microphone, network, filesystem or raw-device access
//! is read out of the merged result — there is no portal for any of them, and
//! no store row either.
//!
//! ## Why the merge has to track where each line came from
//!
//! The owner is entitled to know whether a permission is something the
//! application asked for when it was installed or something they themselves
//! turned on later, because those invite different actions: the first is a
//! reason to distrust the application, the second is a reason to check your own
//! memory. [`Merged::origin_of`] answers it per key, which is why the override
//! is not simply folded into the manifest and forgotten.

use std::collections::BTreeMap;

use crate::model::GrantOrigin;

/// The four `[Context]` keys that decide what a sandbox can reach.
///
/// The other keys in the file — bus policies, environment — describe what it
/// may *talk to*, which is a different question this does not try to answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub shared: Vec<String>,
    pub sockets: Vec<String>,
    pub devices: Vec<String>,
    pub filesystems: Vec<String>,
}

impl Context {
    /// Parse the `[Context]` section of a Flatpak key file.
    ///
    /// Tolerant on purpose: `flatpak info --show-permissions` and an override
    /// file are the same format but not the same content, and a key this does
    /// not know about is skipped rather than treated as a parse failure —
    /// Flatpak adds keys, and a permissions page that refused to render
    /// because of one would tell the owner nothing at all.
    pub fn parse(text: &str) -> Context {
        let mut ctx = Context::default();
        let mut in_section = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_section = line == "[Context]";
                continue;
            }
            if !in_section || line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let items: Vec<String> = value
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            match key.trim() {
                "shared" => ctx.shared = items,
                "sockets" => ctx.sockets = items,
                "devices" => ctx.devices = items,
                "filesystems" => ctx.filesystems = items,
                _ => {}
            }
        }
        ctx
    }

    fn field(&self, key: ContextKey) -> &[String] {
        match key {
            ContextKey::Shared => &self.shared,
            ContextKey::Sockets => &self.sockets,
            ContextKey::Devices => &self.devices,
            ContextKey::Filesystems => &self.filesystems,
        }
    }
}

/// Which of the four lists an entry lives on. Named rather than stringly typed
/// because the enforcer a row reports quotes it back to the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextKey {
    Shared,
    Sockets,
    Devices,
    Filesystems,
}

impl ContextKey {
    pub fn name(&self) -> &'static str {
        match self {
            ContextKey::Shared => "shared",
            ContextKey::Sockets => "sockets",
            ContextKey::Devices => "devices",
            ContextKey::Filesystems => "filesystems",
        }
    }
}

/// The manifest and the overrides, merged, with each surviving entry still
/// knowing which file put it there.
#[derive(Debug, Clone, Default)]
pub struct Merged {
    entries: BTreeMap<(&'static str, String), GrantOrigin>,
}

impl Merged {
    /// Start from the application's own manifest.
    pub fn from_manifest(ctx: &Context) -> Merged {
        let mut m = Merged::default();
        m.absorb(ctx, GrantOrigin::Manifest);
        m
    }

    /// Apply an override file over it.
    ///
    /// An entry beginning with `!` removes the matching entry — that is how
    /// `flatpak override --nosocket=pulseaudio` is written — and the removal
    /// is recorded by deleting the key, so a later [`Merged::has`] answers
    /// false rather than "present but negated".
    pub fn apply_override(&mut self, ctx: &Context, origin: GrantOrigin) {
        self.absorb(ctx, origin);
    }

    fn absorb(&mut self, ctx: &Context, origin: GrantOrigin) {
        for key in [
            ContextKey::Shared,
            ContextKey::Sockets,
            ContextKey::Devices,
            ContextKey::Filesystems,
        ] {
            for item in ctx.field(key) {
                if let Some(stripped) = item.strip_prefix('!') {
                    self.entries.remove(&(key.name(), stripped.to_string()));
                } else {
                    self.entries
                        .insert((key.name(), item.clone()), origin.clone());
                }
            }
        }
    }

    /// Whether the merged context carries this entry.
    pub fn has(&self, key: ContextKey, item: &str) -> bool {
        self.entries.contains_key(&(key.name(), item.to_string()))
    }

    /// Whether any entry on this list starts with the given prefix.
    ///
    /// Filesystem entries carry a `:ro` / `:rw` / `:create` suffix, so an
    /// exact match would miss `xdg-run/pipewire-0:ro` — the entry that gives a
    /// Flatpak audio capture without ever naming a microphone.
    pub fn has_prefix(&self, key: ContextKey, prefix: &str) -> bool {
        self.entries
            .keys()
            .any(|(k, v)| *k == key.name() && (v == prefix || v.starts_with(&format!("{prefix}:"))))
    }

    /// Who put this entry there.
    pub fn origin_of(&self, key: ContextKey, item: &str) -> Option<&GrantOrigin> {
        self.entries.get(&(key.name(), item.to_string()))
    }

    /// The first matching entry's origin, for the prefix form.
    pub fn origin_of_prefix(&self, key: ContextKey, prefix: &str) -> Option<&GrantOrigin> {
        self.entries
            .iter()
            .find(|((k, v), _)| {
                *k == key.name() && (v == prefix || v.starts_with(&format!("{prefix}:")))
            })
            .map(|(_, o)| o)
    }

    /// Every entry on one list, for display.
    pub fn list(&self, key: ContextKey) -> Vec<&str> {
        self.entries
            .keys()
            .filter(|(k, _)| *k == key.name())
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Whether the host `/dev` is inside the sandbox.
    ///
    /// The single most consequential line a manifest can carry, and the reason
    /// this crate never infers enforcement from "it is a Flatpak": `devices=all`
    /// puts `/dev/video0` and `/dev/snd` in front of the application, so the
    /// camera portal is decorative and the ACL is the only thing left.
    pub fn has_raw_devices(&self) -> bool {
        self.has(ContextKey::Devices, "all")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read from `flatpak info --show-permissions io.github.cosmic_utils.camera`
    /// on the development machine, 2026-09-12.
    const COSMIC_CAMERA: &str = "\
[Context]
shared=ipc;
sockets=wayland;pulseaudio;fallback-x11;
devices=all;
filesystems=xdg-run/pipewire-0:ro;xdg-pictures;xdg-videos;xdg-config/cosmic;/run/udev:ro;

[Session Bus Policy]
org.freedesktop.FileManager1=talk
";

    /// Read from `flatpak info --show-permissions com.spotify.Client`, same day.
    const SPOTIFY: &str = "\
[Context]
shared=network;ipc;
sockets=wayland;pulseaudio;fallback-x11;
devices=dri;
filesystems=xdg-run/pipewire-0:ro;xdg-pictures:ro;xdg-music:ro;

[Environment]
TMPDIR=/tmp
";

    #[test]
    fn the_context_section_is_read_and_the_rest_is_not() {
        let c = Context::parse(SPOTIFY);
        assert_eq!(c.shared, ["network", "ipc"]);
        assert_eq!(c.devices, ["dri"]);
        assert_eq!(c.filesystems.len(), 3);
        // TMPDIR lives under [Environment] and must not have leaked in.
        assert!(!c.filesystems.iter().any(|f| f.contains("TMPDIR")));
    }

    #[test]
    fn devices_all_is_recognised_and_devices_dri_is_not() {
        assert!(Merged::from_manifest(&Context::parse(COSMIC_CAMERA)).has_raw_devices());
        assert!(!Merged::from_manifest(&Context::parse(SPOTIFY)).has_raw_devices());
    }

    #[test]
    fn a_filesystem_entry_is_matched_through_its_mode_suffix() {
        let m = Merged::from_manifest(&Context::parse(SPOTIFY));
        assert!(m.has_prefix(ContextKey::Filesystems, "xdg-run/pipewire-0"));
        assert!(!m.has(ContextKey::Filesystems, "xdg-run/pipewire-0"));
        assert!(!m.has_prefix(ContextKey::Filesystems, "home"));
    }

    #[test]
    fn an_override_removes_an_entry_and_is_not_merely_recorded_over_it() {
        let mut m = Merged::from_manifest(&Context::parse(SPOTIFY));
        assert!(m.has(ContextKey::Sockets, "pulseaudio"));
        m.apply_override(
            &Context::parse("[Context]\nsockets=!pulseaudio;\n"),
            GrantOrigin::UserOverride {
                path: "/x/overrides/com.spotify.Client".into(),
            },
        );
        assert!(
            !m.has(ContextKey::Sockets, "pulseaudio"),
            "a negated entry must be gone, not present-and-negated"
        );
        assert!(m.has(ContextKey::Sockets, "wayland"), "and nothing else went");
    }

    #[test]
    fn an_override_that_adds_is_attributed_to_the_owner_not_the_app() {
        let mut m = Merged::from_manifest(&Context::parse(SPOTIFY));
        m.apply_override(
            &Context::parse("[Context]\nfilesystems=home;\n"),
            GrantOrigin::UserOverride {
                path: "/x/overrides/com.spotify.Client".into(),
            },
        );
        assert!(matches!(
            m.origin_of(ContextKey::Filesystems, "home"),
            Some(GrantOrigin::UserOverride { .. })
        ));
        assert!(matches!(
            m.origin_of(ContextKey::Shared, "network"),
            Some(GrantOrigin::Manifest)
        ));
    }

    #[test]
    fn an_unknown_key_does_not_lose_the_keys_beside_it() {
        let c = Context::parse("[Context]\npersistent=.mozilla;\nshared=network;\n");
        assert_eq!(c.shared, ["network"]);
    }
}
