//! Turning the machine's actual state into rows, without inventing an
//! enforcement anywhere along the way.
//!
//! Three inputs: what the session's portal actually exports ([`Session`]), what
//! the permission store holds ([`Store`]), and — for a Flatpak — the merged
//! sandbox context. Everything else is derivation, and every branch that could
//! be tempted to answer "allowed" for something nothing enforces answers
//! something longer instead.
//!
//! ## The session is an input, not a constant
//!
//! `org.freedesktop.portal.Usb` is not exported in APEX's Hyprland session and
//! is exported in its niri session, because `hyprland-portals.conf` resolves to
//! `hyprland;gtk` and neither backend implements it while `gnome.portal` does.
//! The same machine, the same packages, a different login. So "there is no USB
//! permission" is a fact about a session, [`Session::brokers`] is how a row
//! learns it, and a table of constants would have been wrong on two of the
//! three desktops APEX ships.

use std::collections::{BTreeMap, BTreeSet};

use crate::context::{ContextKey, Merged};
use crate::model::{Capability, Enforcer, Grant, GrantOrigin, State, Subject};

// ═════════════════════════════════════════════════════════════════════════════
//  what this session can actually do
// ═════════════════════════════════════════════════════════════════════════════

/// The enforcement primitives that are running right now.
#[derive(Debug, Clone, Default)]
pub struct Session {
    /// `XDG_CURRENT_DESKTOP`, for the report header.
    pub desktop: String,
    /// The `org.freedesktop.portal.*` interfaces exported on
    /// `org.freedesktop.portal.Desktop`. Read by introspection, never assumed
    /// from the installed packages: a backend that is installed but not
    /// preferred by this session's `portals.conf` exports nothing.
    pub portal_interfaces: BTreeSet<String>,
    /// A Wayland screen-copy protocol the compositor offers to any client that
    /// can bind it, if it offers one.
    ///
    /// Hyprland advertises `zwlr_screencopy_manager_v1`,
    /// `ext_image_copy_capture_manager_v1` and
    /// `hyprland_toplevel_export_manager_v1`; niri advertises the first. A
    /// client holding the Wayland socket does not need the ScreenCast portal
    /// at all, which is why screen capture cannot be reported as brokered
    /// merely because a portal exists.
    pub unrestricted_screencopy: Option<String>,
    /// A camera device node, if the machine has one.
    pub camera_node: Option<String>,
    /// An audio capture node, if the machine has one.
    pub audio_capture_node: Option<String>,
    /// Whether the user holds a logind `uaccess` ACL on those nodes — which is
    /// what actually grants a native binary access to them, the group bits
    /// being `root:video` / `root:audio` and the user being in neither.
    pub seat_acl: bool,
}

impl Session {
    /// Whether this session's portal brokers a capability at all.
    pub fn brokers(&self, cap: Capability) -> bool {
        cap.portal_interface()
            .map(|i| self.portal_interfaces.contains(i))
            .unwrap_or(false)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  what the permission store holds
// ═════════════════════════════════════════════════════════════════════════════

/// The portal permission store, as `flatpak permission-list` prints it.
#[derive(Debug, Clone, Default)]
pub struct Store {
    rows: BTreeMap<(String, String, String), Vec<String>>,
    /// Applications with at least one `documents` grant, which is a list of
    /// individual files rather than a capability.
    documents: BTreeSet<String>,
}

impl Store {
    /// Parse `flatpak permission-list` output: tab-separated
    /// `table  id  app  permissions  data`, permissions comma-separated.
    ///
    /// A row this cannot parse is skipped rather than guessed at. The
    /// difference matters: a guessed row could turn a `no` into a `yes`, and
    /// the whole point of this crate is that it does not do that.
    pub fn parse(text: &str) -> Store {
        let mut store = Store::default();
        for line in text.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            let (table, id, app) = (cols[0].trim(), cols[1].trim(), cols[2].trim());
            if table.is_empty() || app.is_empty() {
                continue;
            }
            let perms: Vec<String> = cols
                .get(3)
                .map(|p| {
                    p.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if table == "documents" {
                store.documents.insert(app.to_string());
            }
            store.rows.insert(
                (table.to_string(), id.to_string(), app.to_string()),
                perms,
            );
        }
        store
    }

    /// The stored answer, if there is one.
    ///
    /// `None` is "never asked" and is emphatically not "denied" — the
    /// distinction this whole model exists to keep.
    pub fn lookup(&self, table: &str, id: &str, app: &str) -> Option<&[String]> {
        self.rows
            .get(&(table.to_string(), id.to_string(), app.to_string()))
            .map(Vec::as_slice)
    }

    /// Whether the app has any per-file document grants.
    pub fn has_documents(&self, app: &str) -> bool {
        self.documents.contains(app)
    }

    /// Every application the store has heard of.
    pub fn apps(&self) -> BTreeSet<String> {
        self.rows.keys().map(|(_, _, a)| a.clone()).collect()
    }

    fn state_of(&self, table: &str, id: &str, app: &str) -> State {
        match self.lookup(table, id, app) {
            None => State::NeverAsked,
            Some(p) if p.iter().any(|v| v == "no") => State::Denied,
            Some([]) => State::Denied,
            Some(_) => State::Granted,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  rows
// ═════════════════════════════════════════════════════════════════════════════

/// A store-backed capability, resolved against this session.
fn store_row(
    subject: &Subject,
    cap: Capability,
    session: &Session,
    store: &Store,
) -> Option<Grant> {
    let (table, id) = cap.store_row()?;
    let app = subject.store_key()?;
    if !session.brokers(cap) {
        return Some(Grant::new(
            subject.clone(),
            cap,
            State::NoPrimitive,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        ));
    }
    let state = store.state_of(table, id, app);
    let origin = match state {
        State::NeverAsked => GrantOrigin::Unmediated,
        _ => GrantOrigin::StoreGrant {
            table: table.to_string(),
        },
    };
    Some(Grant::new(
        subject.clone(),
        cap,
        state,
        Enforcer::PortalStore { table, id },
        origin,
    ))
}

/// Screen capture, which is the capability where a portal's existence proves
/// the least.
fn screen_capture_row(subject: &Subject, session: &Session) -> Grant {
    if let Some(proto) = &session.unrestricted_screencopy {
        let g = Grant::new(
            subject.clone(),
            Capability::ScreenCapture,
            State::Granted,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        );
        return match subject {
            // A Flatpak's Wayland socket is tagged with
            // `wp_security_context_v1` by flatpak 1.16, and a compositor may
            // refuse the capture protocols to a tagged client. Whether this
            // one does was NOT measured, so the row says the weaker thing and
            // names the uncertainty instead of resolving it in the direction
            // that flatters the page.
            Subject::Flatpak { .. } => g.with_caveat(format!(
                "The compositor offers {proto} to clients directly. Flatpak tags its Wayland \
                 socket with wp_security_context_v1 and the compositor may refuse capture to a \
                 tagged client — that was not measured here, so treat the portal as the only \
                 checked path and this row as the worst case."
            )),
            Subject::Native { .. } => g.with_caveat(format!(
                "The compositor offers {proto} to any client holding the Wayland socket, so a \
                 native program can capture the screen without going through the portal at all."
            )),
        };
    }
    if session.brokers(Capability::ScreenCapture) {
        return Grant::new(
            subject.clone(),
            Capability::ScreenCapture,
            State::NeverAsked,
            Enforcer::PortalPrompt {
                interface: "org.freedesktop.portal.ScreenCast",
            },
            GrantOrigin::Unmediated,
        );
    }
    Grant::new(
        subject.clone(),
        Capability::ScreenCapture,
        State::NoPrimitive,
        Enforcer::Nothing,
        GrantOrigin::Unmediated,
    )
}

/// Every row for one Flatpak.
pub fn flatpak_grants(
    app_id: &str,
    merged: &Merged,
    store: &Store,
    session: &Session,
) -> Vec<Grant> {
    let subject = Subject::Flatpak {
        app_id: app_id.to_string(),
    };
    let raw_dev = merged.has_raw_devices();
    let dev_origin = merged
        .origin_of(ContextKey::Devices, "all")
        .cloned()
        .unwrap_or(GrantOrigin::Manifest);
    let mut out = Vec::new();

    // Camera. `devices=all` is checked FIRST, because a sandbox holding the
    // host /dev never reaches the portal and a store lookup would report
    // "never asked" about an application that has had the camera all along.
    out.push(if raw_dev {
        Grant::new(
            subject.clone(),
            Capability::Camera,
            State::Granted,
            Enforcer::LogindAcl {
                node: session
                    .camera_node
                    .clone()
                    .unwrap_or_else(|| "/dev/video*".into()),
            },
            dev_origin.clone(),
        )
        .with_caveat(
            "devices=all puts the host /dev inside this sandbox, so it opens the camera \
             directly and the portal never sees the request.",
        )
    } else {
        store_row(&subject, Capability::Camera, session, store)
            .expect("camera has a store row and a flatpak has a store key")
    });

    // Microphone. There is no microphone portal, in any version of
    // xdg-desktop-portal. Either the audio socket is in the sandbox or it is
    // not, and that was decided at install time.
    out.push(if raw_dev {
        Grant::new(
            subject.clone(),
            Capability::Microphone,
            State::Granted,
            Enforcer::LogindAcl {
                node: session
                    .audio_capture_node
                    .clone()
                    .unwrap_or_else(|| "/dev/snd/*".into()),
            },
            dev_origin.clone(),
        )
    } else if merged.has(ContextKey::Sockets, "pulseaudio") {
        Grant::new(
            subject.clone(),
            Capability::Microphone,
            State::Granted,
            Enforcer::SandboxContext {
                key: "sockets=pulseaudio".into(),
            },
            merged
                .origin_of(ContextKey::Sockets, "pulseaudio")
                .cloned()
                .unwrap_or(GrantOrigin::Manifest),
        )
        .with_caveat(
            "There is no microphone portal. The audio socket carries capture as well as \
             playback, so this is the same permission as \"can play sound\".",
        )
    } else if merged.has_prefix(ContextKey::Filesystems, "xdg-run/pipewire-0") {
        Grant::new(
            subject.clone(),
            Capability::Microphone,
            State::Granted,
            Enforcer::SandboxContext {
                key: "filesystems=xdg-run/pipewire-0".into(),
            },
            merged
                .origin_of_prefix(ContextKey::Filesystems, "xdg-run/pipewire-0")
                .cloned()
                .unwrap_or(GrantOrigin::Manifest),
        )
    } else {
        Grant::new(
            subject.clone(),
            Capability::Microphone,
            State::Denied,
            Enforcer::SandboxContext {
                key: "sockets=pulseaudio".into(),
            },
            GrantOrigin::NotRequested,
        )
    });

    out.push(screen_capture_row(&subject, session));
    for cap in [Capability::Location, Capability::Notifications] {
        out.push(
            store_row(&subject, cap, session, store).expect("both have store rows"),
        );
    }

    // Files. `home` and `host` are whole-tree grants; anything else is either
    // a narrow xdg-* grant or nothing, and "nothing" still leaves the document
    // portal, which is a per-file mechanism rather than a capability.
    let broad = ["home", "host", "host-os", "host-etc"]
        .iter()
        .find(|f| merged.has_prefix(ContextKey::Filesystems, f))
        .copied();
    out.push(match broad {
        Some(f) => Grant::new(
            subject.clone(),
            Capability::HomeFiles,
            State::Granted,
            Enforcer::SandboxContext {
                key: format!("filesystems={f}"),
            },
            merged
                .origin_of_prefix(ContextKey::Filesystems, f)
                .cloned()
                .unwrap_or(GrantOrigin::Manifest),
        ),
        None if store.has_documents(app_id) => Grant::new(
            subject.clone(),
            Capability::HomeFiles,
            State::Granted,
            Enforcer::DocumentPortal,
            GrantOrigin::StoreGrant {
                table: "documents".into(),
            },
        ),
        None => Grant::new(
            subject.clone(),
            Capability::HomeFiles,
            State::NeverAsked,
            Enforcer::DocumentPortal,
            GrantOrigin::Unmediated,
        ),
    });

    // Sensitive directories. `filesystems=home` includes ~/.ssh and ~/.gnupg —
    // Flatpak does not carve them out — so a broad grant is a credential grant
    // and the row says so rather than letting "Your files" cover it.
    out.push(match broad {
        Some(f) => Grant::new(
            subject.clone(),
            Capability::SensitiveDirectories,
            State::Granted,
            Enforcer::SandboxContext {
                key: format!("filesystems={f}"),
            },
            merged
                .origin_of_prefix(ContextKey::Filesystems, f)
                .cloned()
                .unwrap_or(GrantOrigin::Manifest),
        )
        .with_caveat(
            "filesystems=home carries ~/.ssh, ~/.gnupg and ~/.config with it. Flatpak does not \
             exclude them.",
        ),
        None => Grant::new(
            subject.clone(),
            Capability::SensitiveDirectories,
            State::Denied,
            Enforcer::SandboxContext {
                key: "filesystems=home".into(),
            },
            GrantOrigin::NotRequested,
        ),
    });

    // USB. `devices=all` again, then the portal if this session has one.
    out.push(if raw_dev {
        Grant::new(
            subject.clone(),
            Capability::UsbDevice,
            State::Granted,
            Enforcer::LogindAcl {
                node: "/dev".into(),
            },
            dev_origin,
        )
    } else if session.brokers(Capability::UsbDevice) {
        Grant::new(
            subject.clone(),
            Capability::UsbDevice,
            State::NeverAsked,
            Enforcer::PortalPrompt {
                interface: "org.freedesktop.portal.Usb",
            },
            GrantOrigin::Unmediated,
        )
    } else {
        Grant::new(
            subject.clone(),
            Capability::UsbDevice,
            State::NoPrimitive,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        )
        .with_caveat(
            "This session's portal configuration exports no Usb interface, so no application is \
             being brokered for device access — not this one, and not any other.",
        )
    });

    out.push(if merged.has(ContextKey::Shared, "network") {
        Grant::new(
            subject.clone(),
            Capability::Network,
            State::Granted,
            Enforcer::SandboxContext {
                key: "shared=network".into(),
            },
            merged
                .origin_of(ContextKey::Shared, "network")
                .cloned()
                .unwrap_or(GrantOrigin::Manifest),
        )
    } else {
        Grant::new(
            subject.clone(),
            Capability::Network,
            State::Denied,
            Enforcer::SandboxContext {
                key: "shared=network".into(),
            },
            GrantOrigin::NotRequested,
        )
    });

    out.push(if session.brokers(Capability::Secrets) {
        Grant::new(
            subject.clone(),
            Capability::Secrets,
            State::NeverAsked,
            Enforcer::PortalPrompt {
                interface: "org.freedesktop.portal.Secret",
            },
            GrantOrigin::Unmediated,
        )
    } else {
        Grant::new(
            subject.clone(),
            Capability::Secrets,
            State::NoPrimitive,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        )
        .with_caveat(
            "No Secret portal in this session. APEX pins gnome-keyring for it on labwc and niri \
             and not on Hyprland — see docs/app-permissions.md §7.",
        )
    });

    out
}

/// Every row for one native application.
///
/// Most of them are the same row, and that is the finding rather than a
/// shortcut: a native program runs as the user, and the user's capabilities
/// are its capabilities.
pub fn native_grants(id: &str, session: &Session) -> Vec<Grant> {
    let subject = Subject::Native { id: id.to_string() };
    let mut out = Vec::new();

    let node_row = |cap: Capability, node: &Option<String>| match node {
        Some(n) if session.seat_acl => Grant::new(
            subject.clone(),
            cap,
            State::Granted,
            Enforcer::LogindAcl { node: n.clone() },
            GrantOrigin::SeatAcl,
        ),
        Some(n) => Grant::new(
            subject.clone(),
            cap,
            State::Denied,
            Enforcer::LogindAcl { node: n.clone() },
            GrantOrigin::SeatAcl,
        ),
        None => Grant::new(
            subject.clone(),
            cap,
            State::NoPrimitive,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        ),
    };

    out.push(
        node_row(Capability::Camera, &session.camera_node).with_caveat(
            "The permission is a logind ACL on the device node and belongs to your login \
             session. Every program you run is on the same side of it.",
        ),
    );
    out.push(node_row(
        Capability::Microphone,
        &session.audio_capture_node,
    ));
    out.push(screen_capture_row(&subject, session));

    // Everything below is the same answer for the same reason, and it is
    // written out per capability rather than collapsed so that a reader of the
    // page sees how far the absence goes.
    for (cap, why) in [
        (
            Capability::Location,
            "A native program talks to geoclue directly; the Location portal is one route, not \
             the only one.",
        ),
        (
            Capability::Notifications,
            "A native program talks to the notification daemon directly.",
        ),
        (
            Capability::HomeFiles,
            "It runs as you, so it opens your files the way you do.",
        ),
        (
            Capability::SensitiveDirectories,
            "~/.ssh and ~/.gnupg are readable by you, and it is you.",
        ),
        (
            Capability::UsbDevice,
            "Device nodes are gated per device by group and ACL, never per application.",
        ),
        (
            Capability::Network,
            "Nothing mediates a socket a native program opens.",
        ),
        (
            Capability::Secrets,
            "The keyring authorises by what the caller is, and apex-secretd brokers agent \
             sessions — neither answers for an ordinary desktop program.",
        ),
    ] {
        out.push(
            Grant::new(
                subject.clone(),
                cap,
                State::Granted,
                Enforcer::Nothing,
                GrantOrigin::Unmediated,
            )
            .with_caveat(why),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;

    /// The live Hyprland session on the development machine, 2026-09-12.
    fn hyprland() -> Session {
        Session {
            desktop: "Hyprland".into(),
            portal_interfaces: [
                "org.freedesktop.portal.Camera",
                "org.freedesktop.portal.ScreenCast",
                "org.freedesktop.portal.Location",
                "org.freedesktop.portal.Notification",
                "org.freedesktop.portal.FileChooser",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            unrestricted_screencopy: Some("zwlr_screencopy_manager_v1".into()),
            camera_node: Some("/dev/video0".into()),
            audio_capture_node: Some("/dev/snd/pcmC2D0c".into()),
            seat_acl: true,
        }
    }

    /// The same machine's niri session, which reaches gnome.portal and so does
    /// export Usb and Secret.
    fn niri() -> Session {
        let mut s = hyprland();
        s.desktop = "niri".into();
        s.portal_interfaces.insert("org.freedesktop.portal.Usb".into());
        s.portal_interfaces
            .insert("org.freedesktop.portal.Secret".into());
        s
    }

    const COSMIC_CAMERA: &str = "\
[Context]
shared=ipc;
sockets=wayland;pulseaudio;fallback-x11;
devices=all;
filesystems=xdg-run/pipewire-0:ro;xdg-pictures;xdg-videos;
";

    const SPOTIFY: &str = "\
[Context]
shared=network;ipc;
sockets=wayland;pulseaudio;fallback-x11;
devices=dri;
filesystems=xdg-run/pipewire-0:ro;xdg-pictures:ro;xdg-music:ro;
";

    /// Read from `flatpak permission-list` on the development machine.
    const STORE: &str = "\
notifications\tnotification\tapp.zen_browser.zen\tyes\t0x00
documents\tf2268abf\tapp.zen_browser.zen\tread,write,grant-permissions\t(...)
location\tlocation\tapp.zen_browser.zen\tEXACT,2215716777\t0x00";

    fn grant_for(rows: &[Grant], cap: Capability) -> &Grant {
        rows.iter().find(|g| g.capability == cap).expect("row")
    }

    /// The item's headline finding, as a test over two REAL manifests.
    #[test]
    fn devices_all_moves_a_flatpak_onto_the_native_enforcer() {
        let session = hyprland();
        let store = Store::parse(STORE);
        let cosmic = flatpak_grants(
            "io.github.cosmic_utils.camera",
            &Merged::from_manifest(&Context::parse(COSMIC_CAMERA)),
            &store,
            &session,
        );
        let spotify = flatpak_grants(
            "com.spotify.Client",
            &Merged::from_manifest(&Context::parse(SPOTIFY)),
            &store,
            &session,
        );
        let native = native_grants("zed", &session);

        assert!(matches!(
            grant_for(&cosmic, Capability::Camera).enforcer,
            Enforcer::LogindAcl { .. }
        ));
        assert!(matches!(
            grant_for(&native, Capability::Camera).enforcer,
            Enforcer::LogindAcl { .. }
        ));
        assert_eq!(
            grant_for(&cosmic, Capability::Camera).headline(),
            grant_for(&native, Capability::Camera).headline()
        );
        // And the control case: a Flatpak without devices=all IS brokered, or
        // the assertion above would hold for a reason that has nothing to do
        // with devices=all.
        assert!(matches!(
            grant_for(&spotify, Capability::Camera).enforcer,
            Enforcer::PortalStore { .. }
        ));
        assert!(grant_for(&spotify, Capability::Camera)
            .revocation
            .offers_a_control());
        assert!(!grant_for(&cosmic, Capability::Camera)
            .revocation
            .offers_a_control());
    }

    /// A refusal's origin is an absence, and labelling it "the app asked for
    /// it" beside the word "blocked" is a sentence that argues with itself.
    #[test]
    fn a_denial_is_not_attributed_to_the_manifest_that_did_not_ask() {
        let rows = flatpak_grants(
            "com.spotify.Client",
            &Merged::from_manifest(&Context::parse(SPOTIFY)),
            &Store::parse(STORE),
            &hyprland(),
        );
        let d = grant_for(&rows, Capability::SensitiveDirectories);
        assert_eq!(d.state, State::Denied);
        assert!(matches!(d.origin, GrantOrigin::NotRequested));
        // And the granted one still names the manifest, or this asserts
        // nothing about telling them apart.
        let n = grant_for(&rows, Capability::Network);
        assert_eq!(n.state, State::Granted);
        assert!(matches!(n.origin, GrantOrigin::Manifest));
    }

    #[test]
    fn a_flatpak_microphone_is_never_reported_as_brokered() {
        let rows = flatpak_grants(
            "com.spotify.Client",
            &Merged::from_manifest(&Context::parse(SPOTIFY)),
            &Store::parse(STORE),
            &hyprland(),
        );
        let mic = grant_for(&rows, Capability::Microphone);
        assert_eq!(mic.state, State::Granted);
        assert!(matches!(mic.enforcer, Enforcer::SandboxContext { .. }));
        assert!(!matches!(mic.enforcer, Enforcer::PortalStore { .. }));
    }

    /// The same application, the same machine, two logins — and a different
    /// set of capabilities that can be answered at all.
    #[test]
    fn the_session_decides_whether_usb_and_secrets_exist() {
        let ctx = Merged::from_manifest(&Context::parse(SPOTIFY));
        let store = Store::parse(STORE);
        let h = flatpak_grants("com.spotify.Client", &ctx, &store, &hyprland());
        let n = flatpak_grants("com.spotify.Client", &ctx, &store, &niri());
        for cap in [Capability::UsbDevice, Capability::Secrets] {
            assert_eq!(
                grant_for(&h, cap).state,
                State::NoPrimitive,
                "{cap:?} should be unanswerable on Hyprland"
            );
            assert_ne!(
                grant_for(&n, cap).state,
                State::NoPrimitive,
                "{cap:?} should be answerable on niri, or this test proves nothing"
            );
        }
    }

    /// A store row that says `yes`, one that says `no`, and one that is not
    /// there — three states, from one parser.
    #[test]
    fn the_store_tells_granted_denied_and_never_asked_apart() {
        let store = Store::parse(
            "notifications\tnotification\ta.yes\tyes\t0x00\n\
             notifications\tnotification\ta.no\tno\t0x00",
        );
        assert_eq!(
            store.state_of("notifications", "notification", "a.yes"),
            State::Granted
        );
        assert_eq!(
            store.state_of("notifications", "notification", "a.no"),
            State::Denied
        );
        assert_eq!(
            store.state_of("notifications", "notification", "a.absent"),
            State::NeverAsked
        );
    }

    /// Screen capture is the case where a portal existing proves nothing, and
    /// the row must not be quietly promoted because ScreenCast is exported.
    #[test]
    fn screen_capture_names_the_protocol_that_bypasses_the_portal() {
        let rows = native_grants("zed", &hyprland());
        let sc = grant_for(&rows, Capability::ScreenCapture);
        assert!(matches!(sc.enforcer, Enforcer::Nothing));
        assert!(sc
            .caveat
            .as_ref()
            .expect("a caveat")
            .contains("zwlr_screencopy_manager_v1"));

        // On a compositor that offers no such protocol, the portal IS the only
        // route and the row changes. Without this half, the assertion above
        // would pass on a build that always answered `Nothing`.
        let mut locked = hyprland();
        locked.unrestricted_screencopy = None;
        let locked_rows = native_grants("zed", &locked);
        let sc = grant_for(&locked_rows, Capability::ScreenCapture);
        assert!(matches!(sc.enforcer, Enforcer::PortalPrompt { .. }));
    }

    /// Nothing a native subject produces may offer a per-application control.
    #[test]
    fn no_native_row_offers_a_revocation() {
        for g in native_grants("zed", &hyprland()) {
            assert!(
                !g.revocation.offers_a_control(),
                "{:?} offered {:?} for a native program",
                g.capability,
                g.revocation
            );
        }
    }

    /// Every capability appears exactly once, for both subject kinds, or the
    /// page silently drops a row.
    #[test]
    fn every_capability_is_answered_once() {
        let session = hyprland();
        for rows in [
            flatpak_grants(
                "com.spotify.Client",
                &Merged::from_manifest(&Context::parse(SPOTIFY)),
                &Store::parse(STORE),
                &session,
            ),
            native_grants("zed", &session),
        ] {
            let mut seen: Vec<Capability> = rows.iter().map(|g| g.capability).collect();
            seen.sort_unstable();
            let mut want = Capability::ALL.to_vec();
            want.sort_unstable();
            assert_eq!(seen, want);
        }
    }
}
