//! Application permissions: what a program may touch, who enforces it, and
//! what a revocation would actually do.
//!
//! Roadmap P1-061. The acceptance criteria carry their own honesty clauses —
//! "where Linux/portal primitives permit", "without pretending they have
//! identical enforcement mechanisms" — and they are the whole difficulty. A
//! Flatpak's camera access may be enforced by the portal and the sandbox; a
//! native binary's is enforced by nothing at all; and a Flatpak whose manifest
//! says `devices=all` is in the native case, not the sandboxed one. A page that
//! showed all three as "allowed" with the same tick would be a lie about the
//! machine.
//!
//! This crate is the truth-teller, not a new enforcement layer. It writes no
//! database of its own: every answer it reports is read from something that is
//! actually consulted — xdg-desktop-portal's permission store, a Flatpak's
//! merged sandbox context, a device node's ACL — and every write it offers goes
//! back through the same place. An APEX-owned shadow store of intentions the
//! portal never reads would be the original lie, one layer deeper.
//!
//! The design, and the measurements it was built from, are in
//! `docs/app-permissions.md`.
//!
//! ## Layout
//!
//! * [`model`] — the four answers that must not be folded into one another.
//! * [`context`] — a Flatpak's `[Context]`, merged with its overrides.
//! * [`report`] — the session's live primitives, the permission store, and the
//!   assembly of rows from both.

pub mod context;
pub mod model;
pub mod report;

pub use context::{Context, ContextKey, Merged};
pub use model::{Capability, Enforcer, Grant, GrantOrigin, Revocation, State, Subject, Timing};
pub use report::{flatpak_grants, native_grants, Session, Store};
