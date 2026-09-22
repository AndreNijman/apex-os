//! Reading the kernel's own BTF to answer one question a status surface could
//! not answer before: **can a sched-ext scheduler bind to this kernel at all?**
//!
//! # Why this exists
//!
//! `apex game status` learned to stop claiming a scheduler the kernel says is
//! not there, and on every APEX image to date the honest answer it produces is
//! `not loaded`. That is correct and it is not useful, because it reads as
//! "something went wrong this time" when the truth is "nothing can ever work
//! here until the kernel changes".
//!
//! Measured on katana, 2026-09-20, on `7.2.6-cachyos1.fc43.x86_64`:
//!
//! ```text
//! libbpf: extern (func ksym) 'scx_bpf_create_dsq': func_proto [1864]
//!         incompatible with vmlinux [60823]
//! libbpf: failed to load BPF skeleton 'bpf_bpf': -EINVAL
//! ```
//!
//! and in that kernel's BTF, type 60823 reads
//!
//! ```text
//! s32 scx_bpf_create_dsq(u64 dsq_id, s32 node, const struct bpf_prog_aux *aux)
//! ```
//!
//! That third parameter is the verifier's **implicit** argument. A kfunc
//! declared `KF_IMPLICIT_ARGS` is supposed to have it stripped from the public
//! BTF prototype by `resolve_btfids`, which finds the kfunc through a
//! `BTF_KIND_DECL_TAG` valued `bpf_kfunc` that `pahole` emits. When the tag is
//! missing the strip never happens, the public prototype keeps the argument,
//! and every BPF program that declares the two-argument form is rejected with
//! `func_proto incompatible with vmlinux`.
//!
//! **Twenty-two sched-ext kfuncs are in that state on the shipped kernel**, so
//! no `scx_*` scheduler can load, and nothing APEX does at runtime can change
//! it. This module turns that from an unexplained `not loaded` into a sentence
//! naming the kfuncs.
//!
//! # What this is NOT
//!
//! It is not a diagnosis of *why* the tags are missing — that is a kernel build
//! question, it is not APEX's to fix (APEX installs a prebuilt
//! `kernel-cachyos` RPM; it does not build a kernel), and a runtime probe has
//! no access to the evidence that would settle it. This reports the one thing
//! it can see for itself: the shape of the prototype in the running kernel's
//! BTF. A cause is written down once, in
//! `ROADMAP/evidence/kernel-btf-scx-20260920.md`, where it can carry its
//! working.
//!
//! # The reader
//!
//! BTF is a small, flat, self-describing format, so this is a reader rather
//! than a dependency: a header, a type section of variable-length records, and
//! a string section. Everything here is bounded — a size cap on the file, a
//! record count cap, and a depth cap on type resolution — because this parses
//! a file the kernel produced but which this code must never trust to be
//! well-formed.

use std::path::Path;

/// Cap on `/sys/kernel/btf/vmlinux`. The real file is ~6.6 MB on a Fedora 43
/// kernel; 64 MiB is four multiples of headroom and still a bound.
const MAX_BTF_BYTES: u64 = 64 * 1024 * 1024;

/// Cap on records walked in the type section. A Fedora 43 vmlinux carries
/// ~150 000 types; a malformed length field must not turn this into a spin.
const MAX_TYPES: usize = 4_000_000;

/// Cap on chasing a type through modifiers/pointers. Legitimate chains are
/// two or three links; a cycle in a corrupt file is unbounded.
const MAX_RESOLVE_DEPTH: u32 = 16;

/// `BTF_MAGIC`, little-endian as the kernel writes it on every architecture
/// APEX ships.
const BTF_MAGIC: u16 = 0xeb9f;

const KIND_PTR: u32 = 2;
const KIND_STRUCT: u32 = 4;
const KIND_UNION: u32 = 5;
const KIND_TYPEDEF: u32 = 8;
const KIND_VOLATILE: u32 = 9;
const KIND_CONST: u32 = 10;
const KIND_RESTRICT: u32 = 11;
const KIND_FUNC: u32 = 12;
const KIND_FUNC_PROTO: u32 = 13;
const KIND_TYPE_TAG: u32 = 18;

/// The struct the verifier passes implicitly and which must NOT appear in a
/// kfunc's public prototype.
const IMPLICIT_ARG_STRUCT: &str = "bpf_prog_aux";

/// The prefix every sched-ext kfunc shares. Deliberately matches the family
/// rather than a hardcoded list of names: which kfuncs a given scheduler
/// references is the scheduler's business, and a list here would go stale the
/// first time `scx-scheds` moved.
const SCX_KFUNC_PREFIX: &str = "scx_bpf_";

/// The suffix `resolve_btfids` gives the ORIGINAL definition when it strips a
/// kfunc's implicit argument: `scx_bpf_dsq_insert_impl` keeps the
/// `struct bpf_prog_aux *`, and the public `scx_bpf_dsq_insert` beside it does
/// not. The `_impl` twin carrying the argument is the mechanism working, not
/// failing, and a BPF program never references one — so it is skipped.
///
/// **This was found by running the reader against a real kernel rather than
/// against its own fixtures.** Without the skip the probe reported 47 affected
/// kfuncs on katana where `libbpf` names 22; with it, the 22 match exactly.
const KFUNC_IMPL_SUFFIX: &str = "_impl";

/// Whether this kernel's BTF can accept a sched-ext scheduler.
///
/// Every variant is a distinct answer. In particular [`ScxBtf::Absent`] and
/// [`ScxBtf::Unreadable`] are not folded together, and neither is folded into
/// "fine": a probe that cannot see is not a probe that saw nothing wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScxBtf {
    /// Every `scx_bpf_*` kfunc in this kernel's BTF has a prototype a BPF
    /// scheduler can bind to.
    Usable,
    /// One or more `scx_bpf_*` kfuncs still carry the verifier's implicit
    /// `struct bpf_prog_aux *` argument in their public prototype. No
    /// sched-ext scheduler that references one of them can load, and no
    /// runtime setting changes that.
    ImplicitArgs {
        /// The kfuncs whose prototypes carry it, sorted, so two machines'
        /// readings are comparable.
        affected: Vec<String>,
        /// How many `scx_bpf_*` kfuncs were examined, so `affected.len()`
        /// reads as a proportion rather than a bare number.
        examined: usize,
    },
    /// The BTF parsed and carries no `scx_bpf_*` kfunc at all — a kernel built
    /// without `CONFIG_SCHED_CLASS_EXT`. A definite answer, and a different
    /// one from a broken prototype.
    NoSchedExtKfuncs,
    /// `/sys/kernel/btf/vmlinux` is not there: `CONFIG_DEBUG_INFO_BTF` off, or
    /// sysfs not mounted. Definite, and not an error.
    Absent,
    /// The file is there and could not be read or made sense of. **Not
    /// absence**, and it does not get to borrow absence's meaning — this
    /// program has confused a refused read for a missing thing often enough to
    /// give it its own variant everywhere.
    Unreadable(String),
}

impl ScxBtf {
    /// One word for a status surface: `ok`, `implicit-args`, `no-sched-ext`,
    /// `absent` or `unreadable`.
    pub fn verdict(&self) -> &'static str {
        match self {
            ScxBtf::Usable => "ok",
            ScxBtf::ImplicitArgs { .. } => "implicit-args",
            ScxBtf::NoSchedExtKfuncs => "no-sched-ext",
            ScxBtf::Absent => "absent",
            ScxBtf::Unreadable(_) => "unreadable",
        }
    }

    /// Whether this reading, on its own, means no scheduler can attach.
    ///
    /// Only [`ScxBtf::ImplicitArgs`] and [`ScxBtf::NoSchedExtKfuncs`] say that.
    /// A probe that could not read the BTF says nothing about the kernel.
    pub fn blocks_loading(&self) -> bool {
        matches!(
            self,
            ScxBtf::ImplicitArgs { .. } | ScxBtf::NoSchedExtKfuncs
        )
    }

    /// A sentence saying what was read, and from where.
    ///
    /// The `ImplicitArgs` sentence names a kfunc rather than only counting
    /// them, because a name is the thing a reader can check against
    /// `scx_loader`'s own journal line.
    pub fn describe(&self) -> String {
        match self {
            ScxBtf::Usable => {
                "kernel BTF: sched-ext kfunc prototypes are the shape BPF schedulers expect"
                    .to_string()
            }
            ScxBtf::ImplicitArgs { affected, examined } => {
                let first = affected
                    .first()
                    .map(String::as_str)
                    .unwrap_or("a sched-ext kfunc");
                format!(
                    "kernel BTF: {} of {examined} sched-ext kfuncs still carry the verifier's \
                     implicit 'struct {IMPLICIT_ARG_STRUCT} *' argument (e.g. {first}), so \
                     libbpf rejects every scx scheduler with 'func_proto incompatible with \
                     vmlinux' — NO sched-ext scheduler can load on this kernel, and no APEX \
                     setting changes that",
                    affected.len()
                )
            }
            ScxBtf::NoSchedExtKfuncs => {
                "kernel BTF: this kernel publishes no scx_bpf_* kfuncs — it was built without \
                 sched-ext support"
                    .to_string()
            }
            ScxBtf::Absent => "kernel BTF: /sys/kernel/btf/vmlinux is not present, so the \
                               sched-ext kfunc prototypes could not be checked"
                .to_string(),
            ScxBtf::Unreadable(why) => {
                format!("kernel BTF is present and could not be read: {why}")
            }
        }
    }
}

/// Read [`ScxBtf`] out of a sysfs root (`/sys`, or a fixture).
///
/// Rooted rather than hardcoded, for the same reason [`crate::syswriter::read_scx_state`]
/// is: the round before this one found that function reading `/sys` verbatim
/// even when built with an explicit fixture root, which left the developer's
/// own machine as the only thing the tests could exercise. Every answer this
/// returns is reachable from a temp directory.
pub fn scx_btf_support(sys_root: &Path) -> ScxBtf {
    let path = sys_root.join("kernel/btf/vmlinux");
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ScxBtf::Absent,
        Err(e) => return ScxBtf::Unreadable(format!("{}: {e}", path.display())),
    };
    // `/sys/kernel/btf/vmlinux` reports its true size, so this is a real bound
    // and not a formality.
    if meta.len() > MAX_BTF_BYTES {
        return ScxBtf::Unreadable(format!(
            "{}: {} bytes exceeds the {MAX_BTF_BYTES}-byte cap this reader will load",
            path.display(),
            meta.len()
        ));
    }
    let raw = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return ScxBtf::Unreadable(format!("{}: {e}", path.display())),
    };
    scx_btf_from_bytes(&raw)
}

/// The reader proper, split out so a test can hand it a blob without a
/// filesystem and so a person can point it at a saved `vmlinux` BTF.
pub fn scx_btf_from_bytes(raw: &[u8]) -> ScxBtf {
    let btf = match Btf::parse(raw) {
        Ok(b) => b,
        Err(e) => return ScxBtf::Unreadable(e),
    };

    let mut examined = 0usize;
    let mut affected: Vec<String> = Vec::new();
    for t in &btf.types {
        if t.kind != KIND_FUNC {
            continue;
        }
        let Some(name) = btf.name(t.name_off) else {
            continue;
        };
        if !name.starts_with(SCX_KFUNC_PREFIX) || name.ends_with(KFUNC_IMPL_SUFFIX) {
            continue;
        }
        examined += 1;
        // `size_or_type` on a FUNC is the id of its FUNC_PROTO.
        if btf.proto_takes_implicit_arg(t.size_or_type) {
            affected.push(name.to_string());
        }
    }

    if examined == 0 {
        return ScxBtf::NoSchedExtKfuncs;
    }
    if affected.is_empty() {
        return ScxBtf::Usable;
    }
    affected.sort();
    affected.dedup();
    ScxBtf::ImplicitArgs { affected, examined }
}

/// One decoded BTF type record. Only the fields this probe needs are kept.
struct BtfType {
    kind: u32,
    name_off: u32,
    size_or_type: u32,
    /// Byte offset of the record's variable-length tail, used for FUNC_PROTO
    /// parameters.
    body: usize,
    vlen: u32,
}

struct Btf<'a> {
    raw: &'a [u8],
    strings: &'a [u8],
    /// Index 0 is a placeholder: BTF type ids start at 1 and id 0 means `void`.
    types: Vec<BtfType>,
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

impl<'a> Btf<'a> {
    fn parse(raw: &'a [u8]) -> Result<Btf<'a>, String> {
        // struct btf_header: magic u16, version u8, flags u8, hdr_len u32,
        // then type_off, type_len, str_off, str_len as u32.
        let magic = u16_at(raw, 0).ok_or_else(|| "shorter than a BTF header".to_string())?;
        if magic != BTF_MAGIC {
            // 0x9feb is the same header read the other way round. Saying so is
            // more use than "bad magic" on the day somebody runs this on a
            // big-endian machine.
            let why = if magic == BTF_MAGIC.swap_bytes() {
                "byte-swapped BTF (big-endian); this reader only decodes native little-endian"
            } else {
                "not BTF"
            };
            return Err(format!("bad magic 0x{magic:04x} — {why}"));
        }
        let hdr_len = u32_at(raw, 4).ok_or_else(|| "truncated header".to_string())? as usize;
        let type_off = u32_at(raw, 8).ok_or_else(|| "truncated header".to_string())? as usize;
        let type_len = u32_at(raw, 12).ok_or_else(|| "truncated header".to_string())? as usize;
        let str_off = u32_at(raw, 16).ok_or_else(|| "truncated header".to_string())? as usize;
        let str_len = u32_at(raw, 20).ok_or_else(|| "truncated header".to_string())? as usize;

        let tstart = hdr_len
            .checked_add(type_off)
            .ok_or_else(|| "type section offset overflows".to_string())?;
        let tend = tstart
            .checked_add(type_len)
            .ok_or_else(|| "type section length overflows".to_string())?;
        let sstart = hdr_len
            .checked_add(str_off)
            .ok_or_else(|| "string section offset overflows".to_string())?;
        let send = sstart
            .checked_add(str_len)
            .ok_or_else(|| "string section length overflows".to_string())?;
        if tend > raw.len() || send > raw.len() {
            return Err(format!(
                "sections run past the end of the file: types {tstart}..{tend}, \
                 strings {sstart}..{send}, file {} bytes",
                raw.len()
            ));
        }
        let strings = &raw[sstart..send];

        let mut types: Vec<BtfType> = Vec::new();
        // The void placeholder, so `types[id]` is the type with that id.
        types.push(BtfType {
            kind: 0,
            name_off: 0,
            size_or_type: 0,
            body: 0,
            vlen: 0,
        });
        let mut off = tstart;
        while off < tend {
            if types.len() > MAX_TYPES {
                return Err(format!("more than {MAX_TYPES} type records"));
            }
            let name_off = u32_at(raw, off).ok_or_else(|| "truncated type record".to_string())?;
            let info = u32_at(raw, off + 4).ok_or_else(|| "truncated type record".to_string())?;
            let size_or_type =
                u32_at(raw, off + 8).ok_or_else(|| "truncated type record".to_string())?;
            let vlen = info & 0xffff;
            let kind = (info >> 24) & 0x1f;
            let body = off + 12;
            let extra = record_tail_len(kind, vlen)
                .ok_or_else(|| format!("unknown BTF kind {kind} at offset {off}"))?;
            let next = body
                .checked_add(extra)
                .ok_or_else(|| "type record length overflows".to_string())?;
            if next > tend {
                return Err(format!(
                    "type record at {off} (kind {kind}, vlen {vlen}) runs past the type section"
                ));
            }
            types.push(BtfType {
                kind,
                name_off,
                size_or_type,
                body,
                vlen,
            });
            off = next;
        }
        Ok(Btf {
            raw,
            strings,
            types,
        })
    }

    fn name(&self, off: u32) -> Option<&'a str> {
        if off == 0 {
            return None;
        }
        let start = off as usize;
        let rest = self.strings.get(start..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        std::str::from_utf8(&rest[..end]).ok()
    }

    fn get(&self, id: u32) -> Option<&BtfType> {
        self.types.get(id as usize)
    }

    /// Whether `proto_id` names a FUNC_PROTO whose LAST parameter resolves to
    /// `struct bpf_prog_aux *`.
    ///
    /// Last, not any: the implicit argument the verifier appends is always the
    /// trailing one, and a kfunc that legitimately *takes* a `bpf_prog_aux`
    /// somewhere else is not this defect. A prototype with no parameters
    /// cannot be carrying one.
    fn proto_takes_implicit_arg(&self, proto_id: u32) -> bool {
        let Some(proto) = self.get(proto_id) else {
            return false;
        };
        if proto.kind != KIND_FUNC_PROTO || proto.vlen == 0 {
            return false;
        }
        // struct btf_param is { name_off: u32, type: u32 }.
        let last = proto.body + (proto.vlen as usize - 1) * 8;
        let Some(param_type) = u32_at(self.raw, last + 4) else {
            return false;
        };
        self.resolves_to_implicit_arg_ptr(param_type)
    }

    /// Follow modifiers to a pointer, then follow the pointee's modifiers to a
    /// struct, and say whether that struct is `bpf_prog_aux`.
    ///
    /// The real prototype reads `const struct bpf_prog_aux *aux`, so both the
    /// `const` and the `struct` have to be seen through. `typedef` and
    /// `type_tag` are followed too, because a kernel is free to introduce
    /// either and a probe that stopped at one would silently start answering
    /// "fine".
    fn resolves_to_implicit_arg_ptr(&self, mut id: u32) -> bool {
        let mut depth = 0;
        // Strip modifiers down to the pointer itself.
        loop {
            depth += 1;
            if depth > MAX_RESOLVE_DEPTH {
                return false;
            }
            let Some(t) = self.get(id) else {
                return false;
            };
            match t.kind {
                KIND_CONST | KIND_VOLATILE | KIND_RESTRICT | KIND_TYPEDEF | KIND_TYPE_TAG => {
                    id = t.size_or_type;
                }
                KIND_PTR => {
                    id = t.size_or_type;
                    break;
                }
                _ => return false,
            }
        }
        // And now the pointee.
        loop {
            depth += 1;
            if depth > MAX_RESOLVE_DEPTH {
                return false;
            }
            let Some(t) = self.get(id) else {
                return false;
            };
            match t.kind {
                KIND_CONST | KIND_VOLATILE | KIND_RESTRICT | KIND_TYPEDEF | KIND_TYPE_TAG => {
                    id = t.size_or_type;
                }
                KIND_STRUCT | KIND_UNION => {
                    return self.name(t.name_off) == Some(IMPLICIT_ARG_STRUCT);
                }
                _ => return false,
            }
        }
    }
}

/// Bytes of variable-length tail after the 12-byte common header, per kind.
///
/// `None` for a kind this reader does not know, which is treated as a parse
/// failure rather than skipped: guessing a record length wrong desynchronises
/// every record after it, and the result would still look like a valid parse.
fn record_tail_len(kind: u32, vlen: u32) -> Option<usize> {
    let v = vlen as usize;
    Some(match kind {
        0 => 0,           // VOID
        1 => 4,           // INT
        KIND_PTR => 0,    // PTR
        3 => 12,          // ARRAY
        KIND_STRUCT | KIND_UNION => v * 12,
        6 => v * 8,       // ENUM
        7 => 0,           // FWD
        KIND_TYPEDEF | KIND_VOLATILE | KIND_CONST | KIND_RESTRICT => 0,
        KIND_FUNC => 0,
        KIND_FUNC_PROTO => v * 8,
        14 => 4,          // VAR
        15 => v * 12,     // DATASEC
        16 => 0,          // FLOAT
        17 => 4,          // DECL_TAG
        KIND_TYPE_TAG => 0,
        19 => v * 12,     // ENUM64
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    //! Every case runs against a BTF blob this module builds itself.
    //!
    //! The real `/sys/kernel/btf/vmlinux` is 6.6 MB and carries 149 301 types;
    //! checking it in as a fixture would be a 6.6 MB binary in the repository
    //! that nobody could read or modify, and a test that only ever proved one
    //! machine's answer. A builder gives both readings — the broken prototype
    //! and the fixed one — from the same code path, which is what makes the
    //! assertion fail in both directions.

    use super::*;
    use std::path::PathBuf;

    /// A scratch sysfs root that cleans itself up. `apexd-core` has no
    /// `tempfile` dev-dependency and the rest of the crate's fixtures do it
    /// this way; a new dependency for four tests is not worth the divergence.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "apexd-kernelbtf-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&p).ok();
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        /// Write `bytes` where `scx_btf_support` will look for them.
        fn put_btf(&self, bytes: &[u8]) {
            let d = self.0.join("kernel/btf");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("vmlinux"), bytes).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A minimal BTF writer. Enough kinds to express the prototypes this probe
    /// looks at, and nothing else.
    #[derive(Default)]
    struct BtfBuilder {
        types: Vec<u8>,
        strings: Vec<u8>,
        count: u32,
    }

    impl BtfBuilder {
        fn new() -> Self {
            // The string section must start with a NUL: offset 0 is "no name".
            BtfBuilder {
                strings: vec![0],
                ..Default::default()
            }
        }

        fn intern(&mut self, s: &str) -> u32 {
            let off = self.strings.len() as u32;
            self.strings.extend_from_slice(s.as_bytes());
            self.strings.push(0);
            off
        }

        fn push(&mut self, name_off: u32, kind: u32, vlen: u32, size_or_type: u32) -> u32 {
            let info = (kind << 24) | (vlen & 0xffff);
            self.types.extend_from_slice(&name_off.to_le_bytes());
            self.types.extend_from_slice(&info.to_le_bytes());
            self.types.extend_from_slice(&size_or_type.to_le_bytes());
            self.count += 1;
            self.count
        }

        fn int(&mut self, name: &str) -> u32 {
            let n = self.intern(name);
            let id = self.push(n, 1, 0, 4);
            self.types.extend_from_slice(&0u32.to_le_bytes()); // btf_int encoding
            id
        }

        fn strukt(&mut self, name: &str) -> u32 {
            let n = self.intern(name);
            self.push(n, KIND_STRUCT, 0, 0)
        }

        fn konst(&mut self, inner: u32) -> u32 {
            self.push(0, KIND_CONST, 0, inner)
        }

        fn typedef(&mut self, name: &str, inner: u32) -> u32 {
            let n = self.intern(name);
            self.push(n, KIND_TYPEDEF, 0, inner)
        }

        fn ptr(&mut self, inner: u32) -> u32 {
            self.push(0, KIND_PTR, 0, inner)
        }

        fn proto(&mut self, ret: u32, params: &[(&str, u32)]) -> u32 {
            let named: Vec<(u32, u32)> = params
                .iter()
                .map(|(n, t)| (self.intern(n), *t))
                .collect();
            let id = self.push(0, KIND_FUNC_PROTO, named.len() as u32, ret);
            for (n, t) in named {
                self.types.extend_from_slice(&n.to_le_bytes());
                self.types.extend_from_slice(&t.to_le_bytes());
            }
            id
        }

        fn func(&mut self, name: &str, proto: u32) -> u32 {
            let n = self.intern(name);
            // vlen on a FUNC is its linkage; 1 == global, as vmlinux writes it.
            self.push(n, KIND_FUNC, 1, proto)
        }

        /// A kind this reader does not know, to prove an unknown record is a
        /// parse failure rather than something silently skipped.
        fn unknown_kind(&mut self) -> u32 {
            self.push(0, 25, 0, 0)
        }

        fn finish(&self) -> Vec<u8> {
            let hdr_len: u32 = 24;
            let type_len = self.types.len() as u32;
            let str_len = self.strings.len() as u32;
            let mut out = Vec::new();
            out.extend_from_slice(&BTF_MAGIC.to_le_bytes());
            out.push(1); // version
            out.push(0); // flags
            out.extend_from_slice(&hdr_len.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // type_off
            out.extend_from_slice(&type_len.to_le_bytes());
            out.extend_from_slice(&type_len.to_le_bytes()); // str_off
            out.extend_from_slice(&str_len.to_le_bytes());
            out.extend_from_slice(&self.types);
            out.extend_from_slice(&self.strings);
            out
        }
    }

    /// The shape katana's kernel actually has: `scx_bpf_create_dsq` with the
    /// implicit argument, and one kfunc beside it that is fine.
    fn broken_kernel() -> Vec<u8> {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let s32t = b.int("s32");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let caux = b.konst(aux);
        let pcaux = b.ptr(caux);
        let p_bad = b.proto(s32t, &[("dsq_id", u64t), ("node", s32t), ("aux", pcaux)]);
        b.func("scx_bpf_create_dsq", p_bad);
        let p_ok = b.proto(s32t, &[("cpu", s32t), ("flags", u64t)]);
        b.func("scx_bpf_kick_cpu", p_ok);
        b.finish()
    }

    /// The same kernel with the argument stripped, which is what a correctly
    /// built kernel publishes.
    fn fixed_kernel() -> Vec<u8> {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let s32t = b.int("s32");
        // `bpf_prog_aux` still exists as a type — only the prototype changes.
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let _ = b.ptr(aux);
        let p1 = b.proto(s32t, &[("dsq_id", u64t), ("node", s32t)]);
        b.func("scx_bpf_create_dsq", p1);
        let p2 = b.proto(s32t, &[("cpu", s32t), ("flags", u64t)]);
        b.func("scx_bpf_kick_cpu", p2);
        b.finish()
    }

    #[test]
    fn implicit_arg_is_found_and_named() {
        match scx_btf_from_bytes(&broken_kernel()) {
            ScxBtf::ImplicitArgs { affected, examined } => {
                assert_eq!(affected, vec!["scx_bpf_create_dsq".to_string()]);
                assert_eq!(examined, 2, "both scx_bpf_* kfuncs should be examined");
            }
            other => panic!("expected ImplicitArgs, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_kernel_reads_usable() {
        assert_eq!(scx_btf_from_bytes(&fixed_kernel()), ScxBtf::Usable);
    }

    /// The pair above is the whole point: the SAME reader has to give two
    /// different answers, or it is asserting nothing.
    #[test]
    fn the_two_kernels_do_not_read_alike() {
        let broken = scx_btf_from_bytes(&broken_kernel());
        let fixed = scx_btf_from_bytes(&fixed_kernel());
        assert_ne!(broken, fixed);
        assert!(broken.blocks_loading());
        assert!(!fixed.blocks_loading());
        assert_eq!(broken.verdict(), "implicit-args");
        assert_eq!(fixed.verdict(), "ok");
    }

    #[test]
    fn a_const_free_pointer_is_still_the_implicit_argument() {
        // katana's reads `const struct bpf_prog_aux *`; nothing guarantees the
        // next kernel's does, so the bare pointer has to be caught too.
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let paux = b.ptr(aux);
        let p = b.proto(u64t, &[("dsq_id", u64t), ("aux", paux)]);
        b.func("scx_bpf_dsq_nr_queued", p);
        match scx_btf_from_bytes(&b.finish()) {
            ScxBtf::ImplicitArgs { affected, .. } => {
                assert_eq!(affected, vec!["scx_bpf_dsq_nr_queued".to_string()]);
            }
            other => panic!("expected ImplicitArgs, got {other:?}"),
        }
    }

    #[test]
    fn the_argument_is_followed_through_a_typedef() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let td = b.typedef("prog_aux_t", aux);
        let ptd = b.ptr(td);
        let p = b.proto(u64t, &[("aux", ptd)]);
        b.func("scx_bpf_locked_rq", p);
        assert!(scx_btf_from_bytes(&b.finish()).blocks_loading());
    }

    /// The trailing position is load-bearing. A kfunc that takes a
    /// `bpf_prog_aux *` as a real, non-final argument is not this defect, and
    /// reporting it as one would put a false "no scheduler can load" on a
    /// machine that works.
    #[test]
    fn an_implicit_arg_that_is_not_last_is_not_the_defect() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let paux = b.ptr(aux);
        let p = b.proto(u64t, &[("aux", paux), ("dsq_id", u64t)]);
        b.func("scx_bpf_something", p);
        assert_eq!(scx_btf_from_bytes(&b.finish()), ScxBtf::Usable);
    }

    /// A kfunc that takes NO arguments at all.
    ///
    /// Not hypothetical: `scx_bpf_locked_rq()` reads exactly like this on a
    /// correctly built kernel, and once the implicit argument is stripped
    /// several more join it. Without the `vlen == 0` guard the reader computes
    /// `body + (0 - 1) * 8` and panics on the subtraction — a probe that
    /// brings down `apex game status` on the machines where it has nothing to
    /// report. Nothing else in the suite had a zero-parameter prototype, so
    /// removing the guard passed everything.
    #[test]
    fn a_kfunc_with_no_parameters_is_read_rather_than_panicked_on() {
        let mut b = BtfBuilder::new();
        let s32t = b.int("s32");
        let p = b.proto(s32t, &[]);
        b.func("scx_bpf_locked_rq", p);
        assert_eq!(scx_btf_from_bytes(&b.finish()), ScxBtf::Usable);
    }

    /// A kfunc taking a pointer to some OTHER struct is ordinary.
    #[test]
    fn another_struct_pointer_is_not_the_implicit_argument() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let task = b.strukt("task_struct");
        let ptask = b.ptr(task);
        let p = b.proto(u64t, &[("p", ptask)]);
        b.func("scx_bpf_task_cgroup", p);
        assert_eq!(scx_btf_from_bytes(&b.finish()), ScxBtf::Usable);
    }

    /// The `_impl` twin is SUPPOSED to carry the argument — it is the
    /// pre-strip original that `resolve_btfids` renames, and no BPF program
    /// references it. Counting it made the reader report 47 affected kfuncs on
    /// katana where libbpf names 22, which is how this was found.
    #[test]
    fn the_impl_twin_is_the_mechanism_working_not_failing() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let s32t = b.int("s32");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let caux = b.konst(aux);
        let pcaux = b.ptr(caux);
        // Exactly the pair a correctly built kernel publishes.
        let with_aux = b.proto(
            s32t,
            &[("p", u64t), ("dsq_id", u64t), ("aux", pcaux)],
        );
        b.func("scx_bpf_dsq_insert_impl", with_aux);
        let public = b.proto(s32t, &[("p", u64t), ("dsq_id", u64t)]);
        b.func("scx_bpf_dsq_insert", public);
        let got = scx_btf_from_bytes(&b.finish());
        assert_eq!(
            got,
            ScxBtf::Usable,
            "the _impl twin must not be counted: {got:?}"
        );
    }

    /// And the twin must not hide a real one either: the public name beside it
    /// still gets read.
    #[test]
    fn a_broken_public_kfunc_beside_an_impl_twin_is_still_found() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let s32t = b.int("s32");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let paux = b.ptr(aux);
        let with_aux = b.proto(s32t, &[("dsq_id", u64t), ("aux", paux)]);
        b.func("scx_bpf_dsq_insert_impl", with_aux);
        let also_broken = b.proto(s32t, &[("dsq_id", u64t), ("aux", paux)]);
        b.func("scx_bpf_create_dsq", also_broken);
        match scx_btf_from_bytes(&b.finish()) {
            ScxBtf::ImplicitArgs { affected, examined } => {
                assert_eq!(affected, vec!["scx_bpf_create_dsq".to_string()]);
                assert_eq!(examined, 1, "the _impl twin is not part of the population");
            }
            other => panic!("expected ImplicitArgs, got {other:?}"),
        }
    }

    /// Only `scx_bpf_*` is this probe's business. A broken prototype on an
    /// unrelated kfunc must not be reported as a sched-ext problem.
    #[test]
    fn non_sched_ext_kfuncs_are_not_this_probes_business() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let paux = b.ptr(aux);
        let bad = b.proto(u64t, &[("x", u64t), ("aux", paux)]);
        b.func("bpf_get_current_task", bad);
        let good = b.proto(u64t, &[("cpu", u64t)]);
        b.func("scx_bpf_cpu_node", good);
        assert_eq!(scx_btf_from_bytes(&b.finish()), ScxBtf::Usable);
    }

    #[test]
    fn a_kernel_without_sched_ext_says_so() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        let p = b.proto(u64t, &[("x", u64t)]);
        b.func("bpf_ktime_get_ns", p);
        assert_eq!(scx_btf_from_bytes(&b.finish()), ScxBtf::NoSchedExtKfuncs);
        assert!(scx_btf_from_bytes(&b.finish()).blocks_loading());
    }

    #[test]
    fn a_missing_file_is_absent_and_a_directory_is_not() {
        let t = Tmp::new("absent");
        assert_eq!(scx_btf_support(t.path()), ScxBtf::Absent);

        // A directory where the file should be: present, and unreadable. The
        // point is that it is NOT rounded to Absent.
        std::fs::create_dir_all(t.path().join("kernel/btf/vmlinux")).expect("mkdir");
        let got = scx_btf_support(t.path());
        assert!(
            matches!(got, ScxBtf::Unreadable(_)),
            "a directory in the file's place must read Unreadable, got {got:?}"
        );
        assert!(!got.blocks_loading(), "an unreadable probe blocks nothing");
    }

    /// The `stat` that fails for a reason that is NOT "no such file".
    ///
    /// `metadata()` on a directory SUCCEEDS, so the case above never reaches
    /// this arm — it fails later, in the `read`. Folding this arm into
    /// `Absent` therefore survived the whole suite, which is the
    /// permission-denied-is-not-absence class reappearing inside the very
    /// module written to stop reporting one thing as another. A file where
    /// `kernel/btf` should be a directory makes the `stat` itself fail with
    /// `ENOTDIR`, needs no privilege, and reaches nothing else.
    #[test]
    fn a_stat_that_fails_for_any_other_reason_is_not_absence() {
        let t = Tmp::new("enotdir");
        std::fs::create_dir_all(t.path().join("kernel")).unwrap();
        // A regular FILE named `btf`, so `kernel/btf/vmlinux` cannot be stat'd.
        std::fs::write(t.path().join("kernel/btf"), b"not a directory").unwrap();
        let got = scx_btf_support(t.path());
        assert!(
            matches!(got, ScxBtf::Unreadable(_)),
            "a stat that failed with something other than NotFound must not read \
             as Absent, got {got:?}"
        );
        assert_ne!(got, ScxBtf::Absent);
        assert!(!got.blocks_loading());
    }

    #[test]
    fn a_real_blob_through_the_filesystem_reads_the_same_as_in_memory() {
        let t = Tmp::new("roundtrip");
        t.put_btf(&broken_kernel());
        assert_eq!(
            scx_btf_support(t.path()),
            scx_btf_from_bytes(&broken_kernel())
        );
    }

    #[test]
    fn rubbish_is_unreadable_rather_than_fine() {
        assert!(matches!(
            scx_btf_from_bytes(b"not btf at all, not even close"),
            ScxBtf::Unreadable(_)
        ));
        assert!(matches!(scx_btf_from_bytes(&[]), ScxBtf::Unreadable(_)));
        // Header, then a type section that claims to run past the file.
        let mut short = BtfBuilder::new().finish();
        short[12] = 0xff;
        short[13] = 0xff;
        assert!(matches!(scx_btf_from_bytes(&short), ScxBtf::Unreadable(_)));
    }

    #[test]
    fn a_byte_swapped_header_says_which_problem_it_is() {
        let mut swapped = broken_kernel();
        swapped[0..2].copy_from_slice(&BTF_MAGIC.swap_bytes().to_le_bytes());
        match scx_btf_from_bytes(&swapped) {
            ScxBtf::Unreadable(why) => assert!(
                why.contains("big-endian"),
                "expected the endianness to be named, got {why:?}"
            ),
            other => panic!("expected Unreadable, got {other:?}"),
        }
    }

    /// An unknown record kind has to stop the parse. Skipping it with a
    /// guessed length desynchronises every record after it and the reader
    /// would then report a confident, wrong answer.
    #[test]
    fn an_unknown_record_kind_stops_the_parse() {
        let mut b = BtfBuilder::new();
        let u64t = b.int("u64");
        b.unknown_kind();
        let aux = b.strukt(IMPLICIT_ARG_STRUCT);
        let paux = b.ptr(aux);
        let p = b.proto(u64t, &[("aux", paux)]);
        b.func("scx_bpf_create_dsq", p);
        match scx_btf_from_bytes(&b.finish()) {
            ScxBtf::Unreadable(why) => {
                assert!(why.contains("unknown BTF kind 25"), "got {why:?}")
            }
            other => panic!("expected Unreadable, got {other:?}"),
        }
    }

    #[test]
    fn describe_names_a_kfunc_and_the_proportion() {
        let d = scx_btf_from_bytes(&broken_kernel()).describe();
        assert!(d.contains("scx_bpf_create_dsq"), "{d}");
        assert!(d.contains("1 of 2"), "{d}");
        assert!(d.contains("bpf_prog_aux"), "{d}");
        // The sentence has to say the thing a user needs: it is not transient.
        assert!(d.contains("NO sched-ext scheduler can load"), "{d}");
    }

    #[test]
    fn the_cap_is_a_real_bound() {
        let t = Tmp::new("cap");
        let d = t.path().join("kernel/btf");
        std::fs::create_dir_all(&d).expect("mkdir");
        let f = std::fs::File::create(d.join("vmlinux")).expect("create");
        // Sparse: nothing is written, so this costs no disk and still makes
        // `metadata().len()` report a file over the cap.
        f.set_len(MAX_BTF_BYTES + 1).expect("sparse grow");
        drop(f);
        match scx_btf_support(t.path()) {
            ScxBtf::Unreadable(why) => assert!(why.contains("cap"), "got {why:?}"),
            other => panic!("expected Unreadable, got {other:?}"),
        }
    }
}
