//! The four independent answers, and why none of them may be folded into
//! another.
//!
//! The obvious shape for application permissions is one boolean per
//! (application, capability) pair. It is wrong in a way that gets somebody
//! hurt: a tick beside "Camera" for a native binary reads as a claim that
//! unticking it would stop the camera, and nothing on this system would.
//!
//! The less obvious shape splits the world into sandboxed and unsandboxed and
//! is also wrong. `io.github.cosmic_utils.camera` is a Flatpak whose manifest
//! says `devices=all`; the host `/dev` is inside its sandbox, so it opens
//! `/dev/video0` directly, never appears in the portal permission store, and
//! its camera access is enforced by exactly what a native binary's is — the
//! POSIX ACL logind writes on the device node. `com.spotify.Client` is a
//! Flatpak with a far tighter context, and its microphone access is still
//! unbrokered, because xdg-desktop-portal has no microphone portal at all: the
//! PulseAudio socket is simply in the sandbox.
//!
//! So the subject kind does not determine enforcement, and this module keeps
//! four things apart that a tick would collapse:
//!
//! * [`State`] — what the answer is right now.
//! * [`Enforcer`] — who refuses if the answer is no.
//! * [`GrantOrigin`] — where the current answer came from.
//! * [`Revocation`] — what taking it away would actually do, including nothing.
//!
//! `apex-agent-core`'s [`Wrap`] reached four values for the same reason:
//! `Option<bool>` could not tell "confined in the definition" from "confined
//! only at launch", and a report built on the smaller type claimed a sandbox a
//! hand-run session did not have. The same care is owed here, and it does not
//! mean four variants — it means as many as the machine actually distinguishes.
//!
//! [`Wrap`]: https://github.com/AndreNijman/apex-os/blob/main/apexd/apex-agent-core/src/mcpconf.rs

use serde::{Deserialize, Serialize};

// ═════════════════════════════════════════════════════════════════════════════
//  who is asking
// ═════════════════════════════════════════════════════════════════════════════

/// The application a row is about.
///
/// Recorded because the owner thinks in applications. **Not** because it
/// decides enforcement — see the module docs for the two counterexamples
/// installed on the development machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Subject {
    /// Identified by its Flatpak application id, which is also the key the
    /// portal permission store files it under.
    Flatpak { app_id: String },
    /// Anything else that runs as the user: an rpm's binary, a system
    /// extension's, something unpacked into `/usr/local`. The id is a desktop
    /// file id or a binary name — a label, not a security boundary, because
    /// there is no security boundary here to name.
    Native { id: String },
}

impl Subject {
    /// The name a store lookup or a `flatpak override` would use, if either
    /// applied. `None` for a native subject, which is the point: there is no
    /// key under which its permissions could be recorded.
    pub fn store_key(&self) -> Option<&str> {
        match self {
            Subject::Flatpak { app_id } => Some(app_id),
            Subject::Native { .. } => None,
        }
    }

    /// A stable display id.
    pub fn id(&self) -> &str {
        match self {
            Subject::Flatpak { app_id } => app_id,
            Subject::Native { id } => id,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  what is being asked about
// ═════════════════════════════════════════════════════════════════════════════

/// The capabilities the roadmap names, plus the two the portal adds.
///
/// Deliberately flat. A hierarchy would invite "filesystem implies
/// sensitive-directory", and on this system it does not: a Flatpak may hold
/// `xdg-pictures` and nothing else, while a native binary holds `$HOME`
/// because nothing withheld it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Camera,
    Microphone,
    ScreenCapture,
    Location,
    Notifications,
    /// The user's own files, as a whole.
    HomeFiles,
    /// `~/.ssh`, `~/.gnupg`, `~/.config/apex` and the rest of §13's list — the
    /// directories whose contents are credentials.
    SensitiveDirectories,
    /// Raw device access: `/dev` inside a sandbox, or the USB portal where one
    /// exists.
    UsbDevice,
    Network,
    /// Stored credentials, through the Secret portal or `apex-secretd`.
    Secrets,
}

impl Capability {
    /// Every capability, in the order Settings shows them.
    pub const ALL: [Capability; 10] = [
        Capability::Camera,
        Capability::Microphone,
        Capability::ScreenCapture,
        Capability::Location,
        Capability::Notifications,
        Capability::HomeFiles,
        Capability::SensitiveDirectories,
        Capability::UsbDevice,
        Capability::Network,
        Capability::Secrets,
    ];

    /// A stable name for a JSON consumer, so the shell never matches on prose.
    pub fn tag(&self) -> &'static str {
        match self {
            Capability::Camera => "camera",
            Capability::Microphone => "microphone",
            Capability::ScreenCapture => "screen-capture",
            Capability::Location => "location",
            Capability::Notifications => "notifications",
            Capability::HomeFiles => "home-files",
            Capability::SensitiveDirectories => "sensitive-directories",
            Capability::UsbDevice => "usb-device",
            Capability::Network => "network",
            Capability::Secrets => "secrets",
        }
    }

    /// What the owner is shown.
    pub fn label(&self) -> &'static str {
        match self {
            Capability::Camera => "Camera",
            Capability::Microphone => "Microphone",
            Capability::ScreenCapture => "Screen capture",
            Capability::Location => "Location",
            Capability::Notifications => "Notifications",
            Capability::HomeFiles => "Your files",
            Capability::SensitiveDirectories => "Sensitive directories",
            Capability::UsbDevice => "USB devices",
            Capability::Network => "Network",
            Capability::Secrets => "Stored credentials",
        }
    }

    /// The `org.freedesktop.portal.*` interface that brokers this capability,
    /// if any exists at all.
    ///
    /// `None` for microphone and network is the load-bearing part.
    /// xdg-desktop-portal has never had a microphone portal: a Flatpak that
    /// records you does it through the PulseAudio socket its manifest asked
    /// for, decided once at install time. Network is `shared=network`, the
    /// same. Claiming either is brokered is the lie in the module docs, one
    /// capability further in.
    pub fn portal_interface(&self) -> Option<&'static str> {
        match self {
            Capability::Camera => Some("org.freedesktop.portal.Camera"),
            Capability::ScreenCapture => Some("org.freedesktop.portal.ScreenCast"),
            Capability::Location => Some("org.freedesktop.portal.Location"),
            Capability::Notifications => Some("org.freedesktop.portal.Notification"),
            Capability::HomeFiles => Some("org.freedesktop.portal.FileChooser"),
            Capability::UsbDevice => Some("org.freedesktop.portal.Usb"),
            Capability::Secrets => Some("org.freedesktop.portal.Secret"),
            Capability::Microphone
            | Capability::Network
            | Capability::SensitiveDirectories => None,
        }
    }

    /// Where the portal permission store keeps the answer: `(table, id)`.
    ///
    /// Not every portal persists one. ScreenCast's restore tokens are opt-in
    /// per request, so an application that never asks for one is re-prompted
    /// every session by design and has no store row to read or write; that is
    /// a permanent, correct "never asked" rather than a gap.
    pub fn store_row(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Capability::Camera => Some(("devices", "camera")),
            Capability::Location => Some(("location", "location")),
            Capability::Notifications => Some(("notifications", "notification")),
            _ => None,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  the four answers
// ═════════════════════════════════════════════════════════════════════════════

/// What the answer is right now.
///
/// Four values because the machine distinguishes four. The pair that must not
/// be merged is [`State::Denied`] and [`State::NoPrimitive`]: the first means a
/// mechanism is refusing, the second means there is no mechanism. A Settings
/// page that showed a USB row as "denied" on a session with no USB portal
/// would be inventing an enforcement that is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// An affirmative record exists — a store grant, a manifest line, or an
    /// ACL the seat was given.
    Granted,
    /// A negative record exists. Somebody said no, explicitly.
    Denied,
    /// No record at all. The next request prompts. Distinct from `Denied`
    /// because telling an owner their camera is protected when the first
    /// request will pop a dialog is the same lie in a quieter voice.
    NeverAsked,
    /// Nothing in this session can express an answer to this question.
    NoPrimitive,
}

impl State {
    pub fn tag(&self) -> &'static str {
        match self {
            State::Granted => "granted",
            State::Denied => "denied",
            State::NeverAsked => "never_asked",
            State::NoPrimitive => "no_primitive",
        }
    }

    /// Whether the application can currently do the thing.
    ///
    /// `NeverAsked` answers **true**, and that is the uncomfortable part: an
    /// unmediated capability nobody has been asked about is a capability the
    /// application has. Only a capability whose enforcer would refuse makes
    /// this false, so the caller must read [`Enforcer`] beside it.
    pub fn effective(&self, enforcer: &Enforcer) -> bool {
        match self {
            State::Granted | State::NoPrimitive => true,
            State::Denied => false,
            State::NeverAsked => !enforcer.refuses_by_default(),
        }
    }
}

/// Who refuses if the answer is no.
///
/// Computed from the manifest and the overrides, never assumed from the
/// subject kind. This is the field a tick hides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Enforcer {
    /// xdg-desktop-portal refuses the request before it reaches a dialog, and
    /// the permission store holds the answer. The only enforcer here that is
    /// both per-application and revocable while the application runs.
    PortalStore {
        table: &'static str,
        id: &'static str,
    },
    /// xdg-desktop-portal brokers every request and **keeps no answer**. The
    /// owner is asked each time, which is enforcement — and there is nothing
    /// stored to withdraw, so it offers no control either.
    ///
    /// ScreenCast is this: restore tokens are opt-in per request, so an
    /// application that never asks for one is re-prompted every session by
    /// design.
    PortalPrompt { interface: &'static str },
    /// A FUSE mount exports exactly the files granted, one at a time. Per-file
    /// rather than per-capability, so it is listed rather than toggled.
    DocumentPortal,
    /// bubblewrap, at application start, from the manifest plus any overrides.
    /// Real kernel enforcement — and fixed for the life of the process.
    SandboxContext { key: String },
    /// A POSIX ACL on a device node, written by logind for whoever holds the
    /// active seat. **Per user, not per application**: every process running
    /// as that user is on the same side of it.
    LogindAcl { node: String },
    /// Nothing stands in the way.
    Nothing,
}

impl Enforcer {
    pub fn tag(&self) -> &'static str {
        match self {
            Enforcer::PortalStore { .. } => "portal_store",
            Enforcer::PortalPrompt { .. } => "portal_prompt",
            Enforcer::DocumentPortal => "document_portal",
            Enforcer::SandboxContext { .. } => "sandbox_context",
            Enforcer::LogindAcl { .. } => "logind_acl",
            Enforcer::Nothing => "nothing",
        }
    }

    /// Whether an application that has never been asked is refused until it is.
    ///
    /// True only for the portal, which is the whole difference between a
    /// brokered capability and one that was merely never mentioned.
    pub fn refuses_by_default(&self) -> bool {
        matches!(
            self,
            Enforcer::PortalStore { .. } | Enforcer::PortalPrompt { .. } | Enforcer::DocumentPortal
        )
    }

    /// Whether the owner can take this away from **this application alone**.
    ///
    /// False for [`Enforcer::LogindAcl`] even though the ACL is real
    /// enforcement, because the only control it offers removes the capability
    /// from the whole login session. A switch that did that, labelled with one
    /// application's name, would be worse than no switch.
    pub fn per_application(&self) -> bool {
        matches!(
            self,
            Enforcer::PortalStore { .. }
                | Enforcer::PortalPrompt { .. }
                | Enforcer::DocumentPortal
                | Enforcer::SandboxContext { .. }
        )
    }

    /// One sentence for the owner, in the page, where a disabled switch would
    /// otherwise invite them to wonder what is broken.
    pub fn why(&self) -> String {
        match self {
            Enforcer::PortalStore { .. } => {
                "The desktop portal asks before this app gets it, and refuses if you say no."
                    .into()
            }
            Enforcer::PortalPrompt { interface } => format!(
                "{interface} asks every time. Nothing is stored, so there is no standing \
                 permission here to take away."
            ),
            Enforcer::DocumentPortal => {
                "This app sees only the individual files you have opened for it.".into()
            }
            Enforcer::SandboxContext { key } => format!(
                "The sandbox was built with {key}. Changing it applies the next time the app \
                 starts, not to a window that is already open."
            ),
            Enforcer::LogindAcl { node } => format!(
                "Access comes from a permission on {node} that belongs to your login session, \
                 not to this app. Nothing on this system can take it from one app and leave it \
                 for the others."
            ),
            Enforcer::Nothing => {
                "Nothing on this system stands between this app and this. It is not restricted, \
                 and it cannot be."
                    .into()
            }
        }
    }
}

/// Where the current answer came from.
///
/// Criterion 3 asks a page to show origin, and this is not
/// `apex-agent-core`'s `RequestOrigin` — that answers "who is asking". This
/// answers "who decided", which for a permission the owner is being invited to
/// change is the more useful of the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantOrigin {
    /// The application shipped with it. The owner never chose this.
    Manifest,
    /// `~/.local/share/flatpak/overrides/<app-id>` — the owner, or something
    /// acting as them, changed the sandbox.
    UserOverride { path: String },
    /// `/var/lib/flatpak/overrides/` — an administrator changed it for
    /// everyone.
    SystemOverride { path: String },
    /// The owner answered a portal dialog and the answer was kept.
    StoreGrant { table: String },
    /// logind gave it to the login session, for holding the active seat.
    SeatAcl,
    /// Nobody granted it. It was never withheld.
    Unmediated,
}

impl GrantOrigin {
    pub fn tag(&self) -> &'static str {
        match self {
            GrantOrigin::Manifest => "manifest",
            GrantOrigin::UserOverride { .. } => "user_override",
            GrantOrigin::SystemOverride { .. } => "system_override",
            GrantOrigin::StoreGrant { .. } => "store_grant",
            GrantOrigin::SeatAcl => "seat_acl",
            GrantOrigin::Unmediated => "unmediated",
        }
    }

    /// What the owner reads under the capability name.
    pub fn label(&self) -> String {
        match self {
            GrantOrigin::Manifest => "the app asked for it when it was installed".into(),
            GrantOrigin::UserOverride { path } => format!("you changed it — {path}"),
            GrantOrigin::SystemOverride { path } => {
                format!("set for every user on this machine — {path}")
            }
            GrantOrigin::StoreGrant { table } => {
                format!("you answered a permission prompt (stored in {table})")
            }
            GrantOrigin::SeatAcl => "your login session has it, and so does everything in it".into(),
            GrantOrigin::Unmediated => "nobody granted it; nothing withheld it".into(),
        }
    }
}

/// When a revocation takes effect.
///
/// Nothing in this implementation produces [`Timing::Immediate`], and the
/// reason is written down rather than left to be rediscovered: whether a
/// permission-store change reaches an *already running* PipeWire client — and
/// how fast — was not measured, because measuring it would have meant taking a
/// camera grant away on a live machine. `NextRequest` is the conservative true
/// statement. If somebody measures it, this is the variant to start using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    /// A running application loses it now. Unused — see above.
    Immediate,
    /// The next time the application asks the portal.
    NextRequest,
    /// The next time the application starts. A window that is already open
    /// keeps what it has.
    NextLaunch,
    /// Never, because there is nothing to revoke.
    Never,
}

impl Timing {
    pub fn tag(&self) -> &'static str {
        match self {
            Timing::Immediate => "immediate",
            Timing::NextRequest => "next_request",
            Timing::NextLaunch => "next_launch",
            Timing::Never => "never",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Timing::Immediate => "takes effect now",
            Timing::NextRequest => "takes effect the next time the app asks",
            Timing::NextLaunch => "takes effect the next time the app starts",
            Timing::Never => "there is nothing to take effect",
        }
    }
}

/// What `apex permissions revoke` would actually do.
///
/// Not a boolean, and three of the five are "less than you hoped".
/// [`Revocation::StoreDeny`] and [`Revocation::StoreForget`] are two different
/// intentions — "never" and "ask me again" — and a page with one button for
/// both answers a question the owner did not ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Revocation {
    /// Write `no` into the permission store. An explicit refusal, which also
    /// stops the next prompt.
    StoreDeny {
        table: &'static str,
        id: &'static str,
    },
    /// Delete the entry. Back to [`State::NeverAsked`], so the owner is asked
    /// again next time.
    StoreForget {
        table: &'static str,
        id: &'static str,
    },
    /// Write a `flatpak override`. Real, and it applies at next launch.
    ContextEdit { key: String },
    /// The capability is real, the access is real, and nothing can take it
    /// from this application alone.
    Unsupported { why: String },
    /// There is no permission here to revoke.
    NoPrimitive,
}

impl Revocation {
    pub fn tag(&self) -> &'static str {
        match self {
            Revocation::StoreDeny { .. } => "store_deny",
            Revocation::StoreForget { .. } => "store_forget",
            Revocation::ContextEdit { .. } => "context_edit",
            Revocation::Unsupported { .. } => "unsupported",
            Revocation::NoPrimitive => "no_primitive",
        }
    }

    /// Whether Settings should draw a control at all.
    ///
    /// A disabled switch invites the owner to wonder what is wrong with their
    /// machine. A sentence tells them the truth, which is that nothing on this
    /// system can do what they are asking for.
    pub fn offers_a_control(&self) -> bool {
        matches!(
            self,
            Revocation::StoreDeny { .. }
                | Revocation::StoreForget { .. }
                | Revocation::ContextEdit { .. }
        )
    }

    /// When it would take effect.
    pub fn timing(&self) -> Timing {
        match self {
            Revocation::StoreDeny { .. } | Revocation::StoreForget { .. } => Timing::NextRequest,
            Revocation::ContextEdit { .. } => Timing::NextLaunch,
            Revocation::Unsupported { .. } | Revocation::NoPrimitive => Timing::Never,
        }
    }

    /// The one derivation in this module, and the invariant the tests below
    /// exist for: a revocation is never stronger than its enforcer.
    ///
    /// A store operation is offered only where the store is what enforces.
    /// [`Enforcer::LogindAcl`] and [`Enforcer::Nothing`] can only ever produce
    /// [`Revocation::Unsupported`] — there is no argument, no flag and no
    /// configuration that changes that, because the reason is the shape of the
    /// primitive and not a decision this code made.
    pub fn derive(state: State, enforcer: &Enforcer) -> Revocation {
        if state == State::NoPrimitive {
            return Revocation::NoPrimitive;
        }
        match enforcer {
            Enforcer::PortalStore { table, id } => Revocation::StoreDeny { table, id },
            Enforcer::PortalPrompt { interface } => Revocation::Unsupported {
                why: format!(
                    "{interface} asks every time and keeps no answer, so there is no standing \
                     permission to withdraw. Saying no at the prompt is the revocation."
                ),
            },
            Enforcer::DocumentPortal => Revocation::Unsupported {
                why: "Each file was granted one at a time. Revoke them individually.".into(),
            },
            Enforcer::SandboxContext { key } => Revocation::ContextEdit { key: key.clone() },
            Enforcer::LogindAcl { node } => Revocation::Unsupported {
                why: format!(
                    "{node} is opened directly, under a permission that belongs to your login \
                     session rather than to this app. Taking it would take it from every app at \
                     once."
                ),
            },
            Enforcer::Nothing => Revocation::Unsupported {
                why: "Nothing mediates this, so there is nothing to withdraw.".into(),
            },
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  a row
// ═════════════════════════════════════════════════════════════════════════════

/// One (subject, capability) pair, with all four answers and nothing collapsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Grant {
    pub subject: Subject,
    pub capability: Capability,
    pub state: State,
    pub enforcer: Enforcer,
    pub origin: GrantOrigin,
    pub revocation: Revocation,
    /// Something measured to be uncertain, rather than left out because it was
    /// inconvenient. Rendered under the row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caveat: Option<String>,
}

impl Grant {
    /// Build a row, deriving the revocation so it cannot disagree with the
    /// enforcer.
    pub fn new(
        subject: Subject,
        capability: Capability,
        state: State,
        enforcer: Enforcer,
        origin: GrantOrigin,
    ) -> Grant {
        let revocation = Revocation::derive(state, &enforcer);
        Grant {
            subject,
            capability,
            state,
            enforcer,
            origin,
            revocation,
            caveat: None,
        }
    }

    /// Attach a measured uncertainty.
    pub fn with_caveat(mut self, caveat: impl Into<String>) -> Grant {
        self.caveat = Some(caveat.into());
        self
    }

    /// Whether the application can do it right now.
    pub fn effective(&self) -> bool {
        self.state.effective(&self.enforcer)
    }

    /// The sentence a page shows instead of a tick.
    ///
    /// "Allowed" on its own is never returned for an unenforceable capability,
    /// which is the single output requirement of this whole item.
    pub fn headline(&self) -> &'static str {
        match (self.state, self.enforcer.per_application()) {
            (State::NoPrimitive, _) => "not available on this session",
            (State::Denied, _) => "blocked",
            (State::NeverAsked, true) => "will ask the first time",
            (State::NeverAsked, false) => "not used yet — nothing would stop it",
            (State::Granted, true) => "allowed",
            (State::Granted, false) => "allowed, and cannot be withdrawn",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acl() -> Enforcer {
        Enforcer::LogindAcl {
            node: "/dev/video0".into(),
        }
    }

    fn store() -> Enforcer {
        Enforcer::PortalStore {
            table: "devices",
            id: "camera",
        }
    }

    /// The item's whole reason for existing, as an assertion over the product
    /// rather than over the pairs somebody thought of.
    #[test]
    fn a_revocation_is_never_stronger_than_its_enforcer() {
        let enforcers = [
            store(),
            Enforcer::PortalPrompt {
                interface: "org.freedesktop.portal.ScreenCast",
            },
            Enforcer::DocumentPortal,
            Enforcer::SandboxContext {
                key: "sockets=pulseaudio".into(),
            },
            acl(),
            Enforcer::Nothing,
        ];
        let states = [
            State::Granted,
            State::Denied,
            State::NeverAsked,
            State::NoPrimitive,
        ];
        for e in &enforcers {
            for s in &states {
                let r = Revocation::derive(*s, e);
                if !e.per_application() {
                    assert!(
                        !r.offers_a_control(),
                        "{e:?} + {s:?} offered {r:?}, but nothing can revoke it per application"
                    );
                }
                if matches!(r, Revocation::StoreDeny { .. } | Revocation::StoreForget { .. }) {
                    assert!(
                        matches!(e, Enforcer::PortalStore { .. }),
                        "{e:?} produced a store operation and the store does not enforce it"
                    );
                }
            }
        }
    }

    /// The `devices=all` case. Same capability, same answer, opposite subject
    /// kinds — and the page has to say the same thing about both.
    #[test]
    fn a_flatpak_with_dev_all_reads_the_same_as_a_native_binary() {
        let fp = Grant::new(
            Subject::Flatpak {
                app_id: "io.github.cosmic_utils.camera".into(),
            },
            Capability::Camera,
            State::Granted,
            acl(),
            GrantOrigin::Manifest,
        );
        let native = Grant::new(
            Subject::Native { id: "zed".into() },
            Capability::Camera,
            State::Granted,
            acl(),
            GrantOrigin::SeatAcl,
        );
        assert_eq!(fp.headline(), native.headline());
        assert_eq!(fp.headline(), "allowed, and cannot be withdrawn");
        assert!(!fp.revocation.offers_a_control());
        assert!(!native.revocation.offers_a_control());
    }

    /// A Flatpak that goes through the portal is not the same row, and the
    /// test would be worthless if it were — this is the case that must differ.
    #[test]
    fn a_brokered_flatpak_does_offer_a_control() {
        let g = Grant::new(
            Subject::Flatpak {
                app_id: "app.zen_browser.zen".into(),
            },
            Capability::Camera,
            State::Granted,
            store(),
            GrantOrigin::StoreGrant {
                table: "devices".into(),
            },
        );
        assert_eq!(g.headline(), "allowed");
        assert!(g.revocation.offers_a_control());
        assert_eq!(g.revocation.timing(), Timing::NextRequest);
    }

    /// Never-asked is not denied, and it is not allowed either. Which of the
    /// two it behaves like depends entirely on the enforcer.
    #[test]
    fn never_asked_means_allowed_unless_something_brokers_it() {
        assert!(!State::NeverAsked.effective(&store()));
        assert!(State::NeverAsked.effective(&acl()));
        assert!(State::NeverAsked.effective(&Enforcer::Nothing));
        assert!(State::NeverAsked.effective(&Enforcer::SandboxContext {
            key: "shared=network".into()
        }));
    }

    /// `Denied` and `NoPrimitive` both mean the owner sees no grant, and they
    /// must never render the same, because one of them is a mechanism
    /// refusing and the other is no mechanism at all.
    #[test]
    fn denied_and_no_primitive_are_different_sentences() {
        let denied = Grant::new(
            Subject::Flatpak {
                app_id: "x".into(),
            },
            Capability::Camera,
            State::Denied,
            store(),
            GrantOrigin::StoreGrant {
                table: "devices".into(),
            },
        );
        let absent = Grant::new(
            Subject::Flatpak {
                app_id: "x".into(),
            },
            Capability::UsbDevice,
            State::NoPrimitive,
            Enforcer::Nothing,
            GrantOrigin::Unmediated,
        );
        assert_ne!(denied.headline(), absent.headline());
        assert!(!denied.effective());
        assert!(absent.effective(), "nothing is stopping it; say so");
        assert_eq!(absent.revocation, Revocation::NoPrimitive);
    }

    /// The measurement in docs/app-permissions.md §2.4, held as a test so that
    /// a later edit has to change the claim deliberately.
    #[test]
    fn nothing_claims_a_revocation_is_immediate() {
        for e in [
            store(),
            Enforcer::PortalPrompt {
                interface: "org.freedesktop.portal.ScreenCast",
            },
            Enforcer::DocumentPortal,
            Enforcer::SandboxContext { key: "k".into() },
            acl(),
            Enforcer::Nothing,
        ] {
            for s in [State::Granted, State::Denied, State::NeverAsked] {
                assert_ne!(
                    Revocation::derive(s, &e).timing(),
                    Timing::Immediate,
                    "{e:?} claims an immediate revocation, and none was measured"
                );
            }
        }
    }

    /// Microphone and network have no portal, and a capability table that
    /// quietly invented one for them would put a broker in the page that does
    /// not exist on the machine.
    #[test]
    fn the_unbrokered_capabilities_name_no_portal() {
        assert_eq!(Capability::Microphone.portal_interface(), None);
        assert_eq!(Capability::Network.portal_interface(), None);
        assert_eq!(Capability::Microphone.store_row(), None);
        assert_eq!(Capability::ScreenCapture.store_row(), None);
        assert!(Capability::Camera.store_row().is_some());
    }

    #[test]
    fn tags_are_unique_so_a_json_consumer_can_switch_on_them() {
        let mut tags: Vec<&str> = Capability::ALL.iter().map(|c| c.tag()).collect();
        tags.sort_unstable();
        let n = tags.len();
        tags.dedup();
        assert_eq!(tags.len(), n);
    }
}
