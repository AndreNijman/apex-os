//! The capability vocabulary: what a provider offers, in words anyone can read.
//!
//! §13.2 asks for *semantic operations, not one broad "Cloudflare write"
//! permission* — `cloudflare.worker.deploy`, `cloudflare.dns.create`,
//! `cloudflare.r2.object.read`. §14 asks that the architecture not be
//! hard-coded around Cloudflare. This module is both halves: a grammar for
//! those names that has nothing Cloudflare in it, and the declaration a
//! provider makes about the ones it offers.
//!
//! ```text
//! cloudflare . r2.object . read
//! └ provider   └ class     └ verb
//! ```
//!
//! ## Why a declared vocabulary and not a closed enum
//!
//! P0-002 shipped a closed Rust enum, with git's arguments as its fields, and
//! the argument for it was: *"a variant carrying a command line would be a way
//! to run anything with a credential attached, and no reviewer can meaningfully
//! approve that."* That argument is about **arguments**, not about enums, and
//! this module keeps it while dropping the enum:
//!
//! * an operation is a name in a [`ProviderSpec`], so the set is still closed —
//!   closed by the registry at run time rather than by `match` at compile time,
//!   and a name no provider declares is refused before anything is read;
//! * an operation's arguments are a **declared** list of [`ParamSpec`]s with a
//!   syntax each. An undeclared parameter is refused rather than ignored, which
//!   is what keeps "there is no command line here" true once parameters are a
//!   map instead of enum fields;
//! * no [`Syntax`] admits a string that could be read as a URL, a shell word or
//!   an option. That is the rule P0-002 enforced for git remote names,
//!   generalised to every provider: *a session that could name a URL could ask
//!   the broker to send the credential to a host it chose.*
//!
//! What the enum bought and this does not is a compile error when a provider
//! adds an operation its runner does not handle. What it cost was that every
//! new provider had to edit a type in this crate — which is exactly what P1-001
//! exists to remove, and what P1-002 through P1-017 would each have had to do.
//! The registry answers it at startup instead, and `apex-secretd` refuses to
//! serve a registry whose operation ids do not parse.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Longest an operation id may be, in bytes.
pub const MAX_OPERATION_ID: usize = 128;
/// Most segments an operation id may have, `provider` included.
pub const MAX_SEGMENTS: usize = 6;
/// Longest one segment may be.
pub const MAX_SEGMENT: usize = 40;
/// Most parameters one operation may be given.
pub const MAX_PARAMS: usize = 16;
/// Longest a [`Syntax::Text`] parameter value may be.
pub const MAX_TEXT: usize = 4096;

/// Why a name or an argument was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocabularyError {
    /// The string is not a well-formed operation id.
    BadOperationId(String),
    /// Well formed, but no registered provider declares it.
    UnknownOperation(String),
    /// The resource does not match the shape the operation declares.
    BadResource { operation: String, resource: String },
    /// This operation takes no resource, and one was given.
    UnwantedResource { operation: String },
    /// A parameter the operation does not declare.
    UnknownParam { operation: String, param: String },
    /// A required parameter was not given.
    MissingParam { operation: String, param: String },
    /// A declared parameter whose value is not the shape it declares.
    BadParam { param: String, value: String },
    /// More parameters than [`MAX_PARAMS`].
    TooManyParams(usize),
}

impl std::fmt::Display for VocabularyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VocabularyError::BadOperationId(s) => write!(
                f,
                "'{}' is not an operation name. One looks like \
                 `provider.thing.verb` — lower case, dot-separated, e.g. \
                 `git.push` or `cloudflare.worker.deploy`",
                s.escape_debug()
            ),
            VocabularyError::UnknownOperation(s) => write!(
                f,
                "'{}' is not an operation this build offers; run \
                 `apex secret capabilities` for the list",
                s.escape_debug()
            ),
            VocabularyError::BadResource {
                operation,
                resource,
            } => write!(
                f,
                "'{}' is not a resource {operation} can act on. A resource is a \
                 NAME the provider resolves for itself, never a URL — a URL \
                 would let a session choose where your credential gets sent",
                resource.escape_debug()
            ),
            VocabularyError::UnwantedResource { operation } => {
                write!(f, "{operation} does not act on a named resource")
            }
            VocabularyError::UnknownParam { operation, param } => write!(
                f,
                "{operation} has no '{}' option; this build refuses an option it \
                 does not declare rather than ignoring it",
                param.escape_debug()
            ),
            VocabularyError::MissingParam { operation, param } => {
                write!(f, "{operation} needs a '{}' option", param.escape_debug())
            }
            VocabularyError::BadParam { param, value } => write!(
                f,
                "'{}' is not a valid {}",
                value.escape_debug(),
                param.escape_debug()
            ),
            VocabularyError::TooManyParams(n) => {
                write!(f, "{n} options is more than the {MAX_PARAMS} allowed")
            }
        }
    }
}

impl std::error::Error for VocabularyError {}

/// A §13.2 operation name, parsed.
///
/// Borrowing rather than owning, because the framework parses one out of a
/// request and hands it straight to a lookup. [`OperationId::to_owned_id`]
/// exists for the places that need to keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationId<'a> {
    text: &'a str,
    /// Byte offset of the first `.`.
    first_dot: usize,
    /// Byte offset of the last `.`.
    last_dot: usize,
}

impl<'a> OperationId<'a> {
    /// Parse `provider.class….verb`.
    ///
    /// The grammar is deliberately narrow: lower case, digits and `-` inside a
    /// segment, `.` between segments, nothing else. It has to survive being a
    /// grant-table key, a JSON field an administrator greps, and a word in a
    /// prompt somebody approves, and the shapes that break any of those —
    /// whitespace, quotes, newlines, `/`, `:` — are the same shapes that make a
    /// name look like a path or a URL.
    pub fn parse(text: &'a str) -> Result<OperationId<'a>, VocabularyError> {
        let bad = || VocabularyError::BadOperationId(text.to_string());
        if text.is_empty() || text.len() > MAX_OPERATION_ID {
            return Err(bad());
        }
        let mut segments = 0;
        let mut first_dot = None;
        let mut last_dot = 0;
        for (offset, segment) in segment_offsets(text) {
            segments += 1;
            if segments > MAX_SEGMENTS || !valid_segment(segment) {
                return Err(bad());
            }
            if offset > 0 {
                if first_dot.is_none() {
                    first_dot = Some(offset - 1);
                }
                last_dot = offset - 1;
            }
        }
        // Two segments minimum: an operation always names the provider it
        // belongs to, so `read` on its own can never be ambiguous between two
        // providers that both offer one.
        let Some(first_dot) = first_dot.filter(|_| segments >= 2) else {
            return Err(bad());
        };
        Ok(OperationId {
            text,
            first_dot,
            last_dot,
        })
    }

    pub fn as_str(&self) -> &'a str {
        self.text
    }

    /// The provider that owns it: `cloudflare` in `cloudflare.worker.deploy`.
    ///
    /// This is what makes a registry lookup possible without a table mapping
    /// operations to providers — the name carries its own routing.
    pub fn provider(&self) -> &'a str {
        &self.text[..self.first_dot]
    }

    /// The verb: `deploy` in `cloudflare.worker.deploy`.
    pub fn verb(&self) -> &'a str {
        &self.text[self.last_dot + 1..]
    }

    /// What the verb acts on: `r2.object` in `cloudflare.r2.object.read`.
    ///
    /// Empty for a two-segment id like `git.push`, where the provider is the
    /// only noun there is.
    pub fn class(&self) -> &'a str {
        if self.first_dot == self.last_dot {
            ""
        } else {
            &self.text[self.first_dot + 1..self.last_dot]
        }
    }

    pub fn to_owned_id(&self) -> String {
        self.text.to_string()
    }
}

impl std::fmt::Display for OperationId<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.text)
    }
}

/// `(byte offset of the segment, the segment)` for each `.`-separated piece.
fn segment_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split('.').map(move |segment| {
        let at = offset;
        offset += segment.len() + 1;
        (at, segment)
    })
}

fn valid_segment(segment: &str) -> bool {
    if segment.is_empty() || segment.len() > MAX_SEGMENT {
        return false;
    }
    if segment.starts_with('-') || segment.ends_with('-') {
        return false;
    }
    segment
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether a string is *safe to forward* as an operation name.
///
/// Deliberately looser than [`OperationId::parse`], and the difference is the
/// layering. A client — the `apex` CLI, `apex-agentd` — has no business knowing
/// which operations exist: that is the registry's, in `apex-secretd`, and a
/// client that had the list would be a client every new provider had to edit.
/// What a client owes is that a string cannot become a grant-table key, a field
/// in a line-delimited audit trail, or a word in a prompt somebody approves,
/// while carrying whitespace, quotes, a newline, a path or a URL.
///
/// So this admits `git.push` and also `git-fetch` — an older spelling only the
/// registry knows is an alias — and refuses `exec me`, `../x` and
/// `https://attacker.example`. `exec` passes, and is then refused by the daemon
/// as an operation no provider offers, which is the right layer to say so.
pub fn valid_operation_ref(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_OPERATION_ID {
        return false;
    }
    let first = name.as_bytes()[0];
    let last = name.as_bytes()[name.len() - 1];
    for edge in [first, last] {
        if !edge.is_ascii_lowercase() && !edge.is_ascii_digit() {
            return false;
        }
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
}

/// Whether an operation can change the provider's state.
///
/// Two values and not three. `cloudflare.dns.delete` is obviously worse than
/// `cloudflare.dns.update`, and the temptation is a `Destroy` in between — but
/// the operation id already says `delete`, and a policy that wants to treat
/// deletion differently should key on the id it can read rather than on a
/// bucket somebody assigned it. What the framework itself needs this for is
/// narrow: git resolves `remote.<name>.pushurl` for a write and the fetch URL
/// for a read, and pinning the wrong one checks the wrong host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Read,
    Write,
}

impl Effect {
    pub fn as_str(&self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Write => "write",
        }
    }

    pub fn is_write(&self) -> bool {
        matches!(self, Effect::Write)
    }
}

impl std::fmt::Display for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The shape of a scalar a caller may put in a request.
///
/// **No variant admits a URL, an option or a shell word**, and that is the
/// point of having variants at all rather than a length limit. The rule is
/// P0-002's, generalised from git remotes to every provider: a caller that
/// could write `https://attacker.example/` into a field the provider resolves
/// is a caller that can choose where the credential is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// One segment: a git remote, a worker name, a bucket, a zone. Letters,
    /// digits, `_`, `.`, `-`; never leading `-`, so it cannot be read as an
    /// option.
    Name,
    /// Segments joined by `/`: an R2 object key, a KV key, a DNS record under
    /// a zone. No `:`, no `//`, no leading or trailing `/`, no `..` — so it is
    /// a path *within* something the provider already chose, never an authority.
    Path,
    /// A git-style ref: a branch, a tag, a version label. `valid_branch_name`'s
    /// rules.
    Ref,
    /// Bounded free text: a DNS record's content, a commit message, a Worker
    /// version's annotation.
    ///
    /// Printable, no control characters, no newline, [`MAX_TEXT`] bytes. This
    /// one cannot be made safe as a *word*, so the contract is on the provider
    /// instead and the framework states it here: **a `Text` value must never
    /// reach a command line as an argument.** It goes in a request body, on
    /// stdin, or in a file. A provider that puts one in `argv` has reintroduced
    /// the hole the closed vocabulary exists to close.
    Text,
}

impl Syntax {
    pub fn as_str(&self) -> &'static str {
        match self {
            Syntax::Name => "name",
            Syntax::Path => "path",
            Syntax::Ref => "ref",
            Syntax::Text => "text",
        }
    }

    /// Whether `value` is this shape.
    pub fn accepts(&self, value: &str) -> bool {
        match self {
            Syntax::Name => valid_name(value),
            Syntax::Path => valid_path(value),
            Syntax::Ref => valid_ref(value),
            Syntax::Text => valid_text(value),
        }
    }
}

/// What an operation may be given besides its resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub syntax: Syntax,
    pub required: bool,
    /// One line, for `apex secret capabilities`.
    pub summary: &'static str,
}

/// What an operation's `resource` field must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// The operation acts on nothing named — `cloudflare.account.read`.
    None,
    /// One name — a git remote, a worker, a bucket.
    Name,
    /// A path within something — `bucket/key`, `zone/record`.
    Path,
}

impl ResourceKind {
    /// Whether `resource` is the shape this operation takes.
    pub fn accepts(&self, resource: &str) -> bool {
        match self {
            ResourceKind::None => resource.is_empty(),
            ResourceKind::Name => valid_name(resource),
            ResourceKind::Path => valid_path(resource),
        }
    }
}

/// One operation a provider offers.
///
/// `&'static` throughout: a provider's vocabulary is a constant in its own
/// module, so the set cannot be edited at run time by anything, and the
/// registry can hand out `&'static OperationSpec` without lifetimes leaking
/// into the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationSpec {
    /// The §13.2 name — `git.push`, `cloudflare.worker.deploy`.
    pub id: &'static str,
    /// One line, in the second person, for `apex secret capabilities` and for
    /// the prompt an owner approves.
    pub summary: &'static str,
    pub effect: Effect,
    pub resource: ResourceKind,
    pub params: &'static [ParamSpec],
    /// Older spellings that still resolve to [`OperationSpec::id`].
    ///
    /// Exists so a grant already on disk keeps working across a rename. The
    /// canonical id is what gets stored and logged; an alias is only ever an
    /// input.
    pub aliases: &'static [&'static str],
}

impl OperationSpec {
    /// Whether `name` is this operation, under its own name or an old one.
    pub fn answers_to(&self, name: &str) -> bool {
        self.id == name || self.aliases.contains(&name)
    }

    /// Whether the caller names nothing at all: no resource, no parameters.
    ///
    /// The one property that makes a grant safe to hold in every project. An
    /// operation like this can only reach the endpoint pinned when its
    /// credential was stored, so where it is asked for changes nothing about
    /// what it reaches. `git.push` fails this — it acts on a remote resolved
    /// out of whatever repository the caller is standing in, so the same grant
    /// would be a different permission in every directory.
    ///
    /// Every parameter counts, not only the required ones: an optional
    /// `branch` is still something the caller names.
    pub fn names_nothing(&self) -> bool {
        matches!(self.resource, ResourceKind::None) && self.params.is_empty()
    }

    fn param(&self, name: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Check a request's resource and parameters against this declaration.
    ///
    /// Refuses an undeclared parameter rather than dropping it. A framework
    /// that ignored one would let a caller believe it had constrained an
    /// operation that in fact ran unconstrained — and would let a typo in
    /// `--branch` push the wrong branch silently.
    pub fn check(&self, resource: &str, params: &Params) -> Result<(), VocabularyError> {
        if matches!(self.resource, ResourceKind::None) && !resource.is_empty() {
            return Err(VocabularyError::UnwantedResource {
                operation: self.id.to_string(),
            });
        }
        if !self.resource.accepts(resource) {
            return Err(VocabularyError::BadResource {
                operation: self.id.to_string(),
                resource: resource.to_string(),
            });
        }
        if params.len() > MAX_PARAMS {
            return Err(VocabularyError::TooManyParams(params.len()));
        }
        for (name, value) in params {
            let Some(spec) = self.param(name) else {
                return Err(VocabularyError::UnknownParam {
                    operation: self.id.to_string(),
                    param: name.clone(),
                });
            };
            if !spec.syntax.accepts(value) {
                return Err(VocabularyError::BadParam {
                    param: name.clone(),
                    value: value.clone(),
                });
            }
        }
        for spec in self.params.iter().filter(|p| p.required) {
            if !params.contains_key(spec.name) {
                return Err(VocabularyError::MissingParam {
                    operation: self.id.to_string(),
                    param: spec.name.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// What a provider declares about itself.
///
/// The whole of the *declarative* half of a provider. It has no methods that
/// touch a credential, a socket or a process, so every crate can hold one —
/// which is what lets the `apex` CLI and `apex-agentd` validate a request's
/// shape without either of them being able to perform it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    /// The first segment of every operation it offers: `git`, `cloudflare`.
    pub id: &'static str,
    /// One line for `apex secret capabilities`.
    pub summary: &'static str,
    pub operations: &'static [OperationSpec],
}

impl ProviderSpec {
    pub fn operation(&self, name: &str) -> Option<&'static OperationSpec> {
        self.operations.iter().find(|op| op.answers_to(name))
    }

    /// Whether every operation it declares is well formed and its own.
    ///
    /// Called at startup by the registry rather than trusted. A provider whose
    /// id does not match the operations it offers would answer for names it
    /// does not implement, and one whose id does not parse would be
    /// unreachable — both are programming errors, and both are cheaper to
    /// find when the daemon starts than when a credential is in flight.
    pub fn validate(&self) -> Result<(), String> {
        for op in self.operations {
            let id = OperationId::parse(op.id)
                .map_err(|e| format!("provider '{}': {e}", self.id))?;
            if id.provider() != self.id {
                return Err(format!(
                    "provider '{}' declares '{}', which belongs to '{}'",
                    self.id,
                    op.id,
                    id.provider()
                ));
            }
            if op.summary.is_empty() {
                return Err(format!("'{}' has no summary", op.id));
            }
            for param in op.params {
                if param.name.is_empty() || param.summary.is_empty() {
                    return Err(format!("'{}' has an undescribed option", op.id));
                }
            }
            for alias in op.aliases {
                if alias.is_empty() || alias.contains(char::is_whitespace) {
                    return Err(format!("'{}' has a malformed alias", op.id));
                }
            }
        }
        Ok(())
    }
}

/// An operation's arguments, as they travel.
///
/// A map rather than typed fields, because typed fields are what forced every
/// provider through this crate. Ordered so a serialised record is stable and
/// two runs of the same request produce the same audit line.
pub type Params = BTreeMap<String, String>;

// ── the scalar grammars ─────────────────────────────────────────────────────

/// One name segment: what git accepts for a remote, minus anything that could
/// be read as a URL or an option — generalised from there to every provider.
///
/// The leading-character rule is the one that matters. `-` first would be read
/// as an option; `:` or `/` anywhere is how a URL looks.
pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 100 {
        return false;
    }
    let first = name.as_bytes()[0];
    if !first.is_ascii_alphanumeric() && first != b'_' {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A path *within* something the provider already chose.
///
/// `/`-joined [`valid_name`] segments and nothing else. No leading `/`, so it
/// cannot be an absolute path; no `//`, so it cannot be the start of an
/// authority; no `..`, so it cannot climb; no `:`, so it cannot carry a scheme
/// or a port.
pub fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 1024 {
        return false;
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains("//") {
        return false;
    }
    path.split('/').all(|segment| valid_name(segment) && segment != ".." && segment != ".")
}

/// A git-style ref: a branch, a tag, a version label.
///
/// git's own branch rules, tightened. Notably refused: a leading `-` (an
/// option), `..` (a revision range), and every control character. A ref reaches
/// a command line, and this is the check that keeps it from being read as
/// something else.
pub fn valid_ref(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') {
        return false;
    }
    if name.contains("..") || name.contains("//") {
        return false;
    }
    if name.ends_with(".lock") || name == "@" {
        return false;
    }
    name.chars().all(|c| {
        (c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '+')) && !c.is_control()
    })
}

/// Bounded free text.
///
/// Printable and one line. See [`Syntax::Text`] for the contract this does
/// *not* discharge: a value of this shape must never reach a command line.
pub fn valid_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TEXT
        && !text.chars().any(|c| c.is_control())
}

/// One operation, in a form that crosses the wire.
///
/// [`OperationSpec`] is `&'static` and cannot be sent; this is what
/// `Response::Hello` carries so `apex secret capabilities` prints the vocabulary
/// the daemon actually serves rather than one the CLI hardcoded and would have
/// to keep in step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationInfo {
    pub id: String,
    pub summary: String,
    pub effect: String,
    /// `none`, `name` or `path` — what the caller must give as the resource.
    pub resource: String,
    #[serde(default)]
    pub params: Vec<ParamInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub name: String,
    pub summary: String,
    pub required: bool,
}

impl OperationInfo {
    pub fn of(op: &OperationSpec) -> OperationInfo {
        OperationInfo {
            id: op.id.to_string(),
            summary: op.summary.to_string(),
            effect: op.effect.as_str().to_string(),
            resource: match op.resource {
                ResourceKind::None => "none",
                ResourceKind::Name => "name",
                ResourceKind::Path => "path",
            }
            .to_string(),
            params: op
                .params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.to_string(),
                    summary: p.summary.to_string(),
                    required: p.required,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// §13.2's list, verbatim from ROADMAP.md.
    ///
    /// Not a Cloudflare provider — P1-002 builds that, and it needs an account.
    /// This is the claim P1-001 has to make good on: that the *vocabulary* is
    /// expressible without anything in this crate knowing what Cloudflare is.
    const SECTION_13_2: &[&str] = &[
        "cloudflare.account.read",
        "cloudflare.worker.read",
        "cloudflare.worker.upload-version",
        "cloudflare.worker.deploy",
        "cloudflare.worker.rollback",
        "cloudflare.worker.tail",
        "cloudflare.dns.read",
        "cloudflare.dns.create",
        "cloudflare.dns.update",
        "cloudflare.dns.delete",
        "cloudflare.r2.object.read",
        "cloudflare.r2.object.write",
        "cloudflare.r2.bucket.create",
        "cloudflare.d1.read",
        "cloudflare.d1.query",
        "cloudflare.d1.migrate",
        "cloudflare.kv.read",
        "cloudflare.kv.write",
        "cloudflare.queue.publish",
        "cloudflare.queue.manage",
        "cloudflare.hyperdrive.read",
        "cloudflare.hyperdrive.edit",
        "cloudflare.secret.create",
        "cloudflare.secret.bind",
        "cloudflare.secret.rotate",
        "cloudflare.access.read",
        "cloudflare.access.edit",
        "cloudflare.tunnel.read",
        "cloudflare.tunnel.edit",
        "cloudflare.workers-ai.run",
        "cloudflare.ai-gateway.run",
        "cloudflare.ai-gateway.edit",
    ];

    #[test]
    fn every_name_section_thirteen_two_lists_parses() {
        // If one of these ever stops parsing, P1-002 cannot name its own
        // operations and the vocabulary is Cloudflare-shaped in the wrong
        // direction — too narrow rather than too wide.
        for name in SECTION_13_2 {
            let id = OperationId::parse(name)
                .unwrap_or_else(|e| panic!("§13.2 lists '{name}' and it does not parse: {e}"));
            assert_eq!(id.provider(), "cloudflare", "{name}");
            assert!(!id.verb().is_empty(), "{name}");
            assert_eq!(id.as_str(), *name);
        }
    }

    #[test]
    fn a_name_splits_into_provider_class_and_verb() {
        let deploy = OperationId::parse("cloudflare.worker.deploy").unwrap();
        assert_eq!(deploy.provider(), "cloudflare");
        assert_eq!(deploy.class(), "worker");
        assert_eq!(deploy.verb(), "deploy");

        // A nested class keeps its dots — `r2.object` is one noun, not two.
        let object = OperationId::parse("cloudflare.r2.object.read").unwrap();
        assert_eq!(object.provider(), "cloudflare");
        assert_eq!(object.class(), "r2.object");
        assert_eq!(object.verb(), "read");

        // Two segments: the provider is the only noun there is.
        let push = OperationId::parse("git.push").unwrap();
        assert_eq!(push.provider(), "git");
        assert_eq!(push.class(), "");
        assert_eq!(push.verb(), "push");
    }

    #[test]
    fn an_operation_name_cannot_carry_framing_a_path_or_a_url() {
        // The trail is line-delimited JSON somebody greps and the grant table
        // is keyed on this string. Every shape below either breaks one of
        // those or makes a name look like somewhere to send a credential.
        for evil in [
            "",
            "push",                       // no provider
            "git.",                       // empty verb
            ".push",                      // empty provider
            "git..push",                  // empty class
            "git.PUSH",                   // case
            "git push",
            "git.push arg",
            "git.push\n",
            "git.push;rm -rf /",
            "https://attacker.example",
            "git/push",
            "git:push",
            "-git.push",
            "git.-push",
            "git.push-",
            "git.push\"",
            "../git.push",
            "a.b.c.d.e.f.g",              // more than MAX_SEGMENTS
        ] {
            assert!(
                OperationId::parse(evil).is_err(),
                "'{}' was accepted as an operation name",
                evil.escape_debug()
            );
        }
        assert!(OperationId::parse(&format!("git.{}", "x".repeat(MAX_SEGMENT))).is_ok());
        assert!(OperationId::parse(&format!("git.{}", "x".repeat(MAX_SEGMENT + 1))).is_err());
        assert!(OperationId::parse(&"a.".repeat(MAX_OPERATION_ID)).is_err());
    }

    #[test]
    fn a_resource_may_not_be_a_url_an_option_or_an_absolute_path() {
        // The hole P0-002 closed for git remotes, asserted for every shape a
        // provider can ask for. A caller that can write an authority into a
        // resource can choose where the credential goes.
        for evil in [
            "https://attacker.example/r",
            "git@github.com:a/b",
            "-f",
            "--force",
            "../x",
            "a b",
            "",
            "ori\ngin",
            "a:b",
        ] {
            assert!(!valid_name(evil), "'{}' passed Name", evil.escape_debug());
            assert!(!valid_path(evil), "'{}' passed Path", evil.escape_debug());
        }
        // A path is segments of a name, and nothing that could be an authority.
        assert!(valid_path("my-bucket/logs/2026-09-06.json"));
        assert!(valid_path("single"));
        for evil in ["/abs", "trailing/", "a//b", "a/../b", "a/./b", "//host/x"] {
            assert!(!valid_path(evil), "'{}' passed Path", evil.escape_debug());
        }
    }

    #[test]
    fn text_is_one_printable_line_and_nothing_longer() {
        assert!(valid_text("v1.2.3 — deployed by apex"));
        for evil in ["", "two\nlines", "bell\x07", &"x".repeat(MAX_TEXT + 1)] {
            assert!(!valid_text(evil), "'{}' passed Text", evil.escape_debug());
        }
    }

    // A provider declared purely to test the framework, with one operation of
    // each resource shape. Deliberately not git and not Cloudflare: a check
    // that only passes for the provider it was written against is not a check.
    const DEMO: ProviderSpec = ProviderSpec {
        id: "demo",
        summary: "a provider that exists to test the framework",
        operations: &[
            OperationSpec {
                id: "demo.account.read",
                summary: "read the account",
                effect: Effect::Read,
                resource: ResourceKind::None,
                params: &[],
                aliases: &[],
            },
            OperationSpec {
                id: "demo.thing.write",
                summary: "write a thing",
                effect: Effect::Write,
                resource: ResourceKind::Name,
                params: &[
                    ParamSpec {
                        name: "version",
                        syntax: Syntax::Ref,
                        required: true,
                        summary: "which version",
                    },
                    ParamSpec {
                        name: "note",
                        syntax: Syntax::Text,
                        required: false,
                        summary: "an annotation",
                    },
                ],
                aliases: &["demo-write"],
            },
            OperationSpec {
                id: "demo.blob.object.read",
                summary: "read an object",
                effect: Effect::Read,
                resource: ResourceKind::Path,
                params: &[],
                aliases: &[],
            },
        ],
    };

    #[test]
    fn a_provider_that_declares_an_operation_it_does_not_own_is_refused() {
        assert_eq!(DEMO.validate(), Ok(()));

        const IMPOSTOR: ProviderSpec = ProviderSpec {
            id: "demo",
            summary: "s",
            operations: &[OperationSpec {
                id: "cloudflare.worker.deploy",
                summary: "s",
                effect: Effect::Write,
                resource: ResourceKind::Name,
                params: &[],
                aliases: &[],
            }],
        };
        let err = IMPOSTOR.validate().unwrap_err();
        assert!(err.contains("belongs to 'cloudflare'"), "{err}");

        const UNPARSEABLE: ProviderSpec = ProviderSpec {
            id: "demo",
            summary: "s",
            operations: &[OperationSpec {
                id: "not an operation",
                summary: "s",
                effect: Effect::Read,
                resource: ResourceKind::None,
                params: &[],
                aliases: &[],
            }],
        };
        assert!(UNPARSEABLE.validate().is_err());
    }

    #[test]
    fn an_undeclared_option_is_refused_and_never_ignored() {
        // The property that keeps "there is no command line here" true once
        // arguments are a map. A framework that dropped an unknown key would
        // let a typo in `--branch` push the wrong branch and say nothing.
        let write = DEMO.operation("demo.thing.write").unwrap();
        assert!(matches!(
            write.check("thing", &params(&[("version", "v1"), ("force", "yes")])),
            Err(VocabularyError::UnknownParam { .. })
        ));
        assert_eq!(write.check("thing", &params(&[("version", "v1")])), Ok(()));
    }

    #[test]
    fn a_required_option_is_required_and_a_declared_one_is_checked() {
        let write = DEMO.operation("demo.thing.write").unwrap();
        assert!(matches!(
            write.check("thing", &params(&[])),
            Err(VocabularyError::MissingParam { .. })
        ));
        // A ref that is an option, a revision range or a lock file is refused
        // by the same rules that already guard a git branch.
        for evil in ["-f", "a..b", "a.lock", "@"] {
            assert!(
                matches!(
                    write.check("thing", &params(&[("version", evil)])),
                    Err(VocabularyError::BadParam { .. })
                ),
                "'{evil}' was accepted as a ref"
            );
        }
    }

    #[test]
    fn a_resource_must_be_the_shape_its_operation_declares() {
        let read_account = DEMO.operation("demo.account.read").unwrap();
        assert_eq!(read_account.check("", &params(&[])), Ok(()));
        assert!(matches!(
            read_account.check("something", &params(&[])),
            Err(VocabularyError::UnwantedResource { .. })
        ));

        let read_object = DEMO.operation("demo.blob.object.read").unwrap();
        assert_eq!(read_object.check("bucket/key", &params(&[])), Ok(()));
        // A Name operation does not take a path, and a Path operation does not
        // take an empty one. Both would otherwise reach the provider as
        // something it has to guess about.
        let write = DEMO.operation("demo.thing.write").unwrap();
        assert!(write.check("a/b", &params(&[("version", "v1")])).is_err());
        assert!(read_object.check("", &params(&[])).is_err());
    }

    #[test]
    fn an_alias_resolves_to_the_canonical_operation() {
        // What keeps a grant already on disk working across a rename. The
        // canonical id is what gets stored; an alias is only ever an input.
        let by_alias = DEMO.operation("demo-write").unwrap();
        assert_eq!(by_alias.id, "demo.thing.write");
        assert!(DEMO.operation("demo.thing.delete").is_none());
    }

    #[test]
    fn too_many_options_is_refused_before_any_are_read() {
        let write = DEMO.operation("demo.thing.write").unwrap();
        let many: Params = (0..MAX_PARAMS + 1)
            .map(|i| (format!("p{i}"), "v".to_string()))
            .collect();
        assert!(matches!(
            write.check("thing", &many),
            Err(VocabularyError::TooManyParams(_))
        ));
    }

    #[test]
    fn a_wire_form_carries_what_the_cli_needs_to_print() {
        // The CLI prints the daemon's vocabulary rather than one of its own, so
        // a provider added to the daemon shows up in `apex secret capabilities`
        // without the CLI being rebuilt around it.
        let info = OperationInfo::of(DEMO.operation("demo.thing.write").unwrap());
        assert_eq!(info.id, "demo.thing.write");
        assert_eq!(info.effect, "write");
        assert_eq!(info.resource, "name");
        assert_eq!(info.params.len(), 2);
        assert!(info.params.iter().any(|p| p.name == "version" && p.required));
        let text = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<OperationInfo>(&text).unwrap(), info);
    }

    #[test]
    fn a_forwardable_name_admits_an_alias_and_refuses_framing() {
        // The client-side check. It must let an older spelling through — the
        // registry is the only thing that knows `git-fetch` means `git.push`'s
        // sibling — while refusing anything that could reshape a trail or look
        // like somewhere to send a credential.
        for good in ["git.push", "git-fetch", "cloudflare.r2.object.read", "exec"] {
            assert!(valid_operation_ref(good), "{good}");
        }
        for bad in [
            "",
            "git push",
            "git.push;rm -rf /",
            "https://attacker.example",
            "../x",
            "git/push",
            "GIT.PUSH",
            "-git",
            "git-",
            ".git",
            "git.",
            "git\npush",
            &"x".repeat(MAX_OPERATION_ID + 1),
        ] {
            assert!(!valid_operation_ref(bad), "'{}' was accepted", bad.escape_debug());
        }
        // And everything the strict parser accepts, this accepts too — a client
        // must never refuse a name the daemon would have honoured.
        for name in SECTION_13_2 {
            assert!(valid_operation_ref(name), "{name}");
        }
    }

    #[test]
    fn a_ref_refuses_an_option_and_a_revision_range() {
        for good in ["main", "feat/x-1.2", "v1.0.0+build"] {
            assert!(valid_ref(good), "{good}");
        }
        for evil in ["-f", "a..b", "x/", "/x", "a//b", "a.lock", "@", "", "a\nb"] {
            assert!(!valid_ref(evil), "'{}' was accepted as a ref", evil.escape_debug());
        }
    }

    #[test]
    fn an_effect_says_only_whether_state_changes() {
        assert!(Effect::Write.is_write());
        assert!(!Effect::Read.is_write());
        assert_eq!(Effect::Read.as_str(), "read");
    }
}
