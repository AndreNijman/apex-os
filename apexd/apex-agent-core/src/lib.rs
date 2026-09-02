//! APEX agent runtime, shared library.
//!
//! Everything the runtime knows that is not I/O: the control protocol, the
//! sandbox policy, the adapters, project detection and checkpoints. The daemon
//! (`apex-agentd`) and the CLI (`apex agent`) both build on this, so the two
//! can never disagree about what a session is or what a policy means.
//!
//! The split is also what makes the security-relevant parts testable. The
//! sandbox argv, the state machine and the checkpoint plumbing are pure
//! functions over their inputs, asserted directly rather than through a live
//! PTY — and they are the shipped functions, not a reimplementation of them.
//!
//! ## Architecture
//!
//! ```text
//! claude / opencode / codex / gemini / any binary
//!         │  (real upstream process, unmodified, in a real PTY)
//!         ▼
//! apex-agentd  — unprivileged, per-user, systemd --user
//!         ├─ PTY + session lifecycle       session.rs
//!         ├─ sandbox policy                sandbox.rs
//!         ├─ adapters                      adapter.rs
//!         ├─ projects + worktrees          project.rs
//!         └─ checkpoints                   checkpoint.rs
//!         ▲
//!         │  newline-delimited JSON on a Unix socket    protocol.rs
//! apex agent … / APEX Shell
//! ```
//!
//! Nothing here is privileged, and nothing here talks to `apexd`. Agent
//! orchestration in the privileged daemon is exactly what the roadmap forbids;
//! when a session eventually needs a system change it will be a narrow,
//! audited request to `org.apexos.Apexd1`, not this process gaining rights.

pub mod adapter;
pub mod checkpoint;
pub mod client;
pub mod config;
pub mod git;
pub mod paths;
pub mod project;
pub mod protocol;
pub mod sandbox;
pub mod session;
pub mod term;

pub use protocol::{AgentState, SandboxPolicy, SessionInfo};
