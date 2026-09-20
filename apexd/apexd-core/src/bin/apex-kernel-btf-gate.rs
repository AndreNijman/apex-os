//! Build gate: refuse a kernel whose BTF carries the sched-ext kfunc defect.
//!
//! APEX builds its own kernel *because* every kernel it shipped before could
//! not load a sched-ext scheduler:
//! `ROADMAP/evidence/kernel-btf-scx-20260920.md` measured 22 of 68 `scx_bpf_*`
//! kfuncs on katana still carrying `resolve_btfids`' implicit
//! `struct bpf_prog_aux *`. A kernel build that can ship that exact bug is the
//! "gate that inspects nothing" — this repository's dominant defect family —
//! so the build runs this over the BTF it just produced and fails if it is
//! there.
//!
//! **It reuses [`apexd_core::kernelbtf`] deliberately.** That reader is the one
//! `apex game status` answers from. A second reader written here could disagree
//! with it, and then a kernel could pass the build and still tell the user its
//! BTF is broken — or the reverse. One reader, two callers.
//!
//! Usage: `apex-kernel-btf-gate <btf-blob>`
//!
//! The argument is a raw BTF blob — `objcopy --dump-section .BTF=… vmlinux`, or
//! `/sys/kernel/btf/vmlinux` on a running machine.
//!
//! **Exit 0 on [`ScxBtf::Usable`] and on nothing else.** The three readings that
//! are not "broken prototypes" fail too, and each for its own reason:
//!
//! * `NoSchedExtKfuncs` — a kernel built without `CONFIG_SCHED_CLASS_EXT`. It
//!   has no defect, and it is also not the kernel APEX means to ship: Gaming
//!   Mode's scheduler tier needs those kfuncs to exist. Passing it would let a
//!   config regression that deletes sched-ext entirely sail through the one
//!   gate whose subject is sched-ext.
//! * `Absent` / `Unreadable` — the gate could not see. A gate that passes when
//!   it cannot see is the thing this file exists to prevent. `blocks_loading()`
//!   is correct to say these two do not convict the *kernel*; it is not the
//!   right test for whether a *build* may proceed, and the difference is the
//!   whole point.

use apexd_core::kernelbtf::{scx_btf_from_bytes, ScxBtf};
use std::process::ExitCode;

/// Same cap the sysfs reader uses, for the same reason: a build artefact that
/// is implausibly large is a wrong file, not a big kernel.
const MAX_BTF_BYTES: u64 = 64 * 1024 * 1024;

/// Exit status for a reading. **The whole gate is this function**, so it is a
/// function rather than a `match` buried in `main`: the property that matters
/// — `Usable` and nothing else passes — is then something a test can state
/// directly instead of something a reader has to trust.
///
/// 0 pass · 1 the kernel is not shippable · 2 usage · 3 the input is not a
/// readable file.
fn exit_code_for(verdict: &ScxBtf) -> u8 {
    match verdict {
        ScxBtf::Usable => 0,
        // Every other reading fails, including the two that `blocks_loading()`
        // correctly declines to blame the kernel for. See the module docs: the
        // question here is "may this build proceed", not "is this kernel's
        // sched-ext broken", and they have different right answers.
        ScxBtf::ImplicitArgs { .. }
        | ScxBtf::NoSchedExtKfuncs
        | ScxBtf::Absent
        | ScxBtf::Unreadable(_) => 1,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: {} <btf-blob>", args[0]);
        eprintln!();
        eprintln!("Reads a raw BTF blob and exits 0 only if every scx_bpf_* kfunc");
        eprintln!("prototype is the shape a BPF scheduler can bind to.");
        return ExitCode::from(2);
    }
    let path = std::path::Path::new(&args[1]);

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("FATAL: {}: {e}", path.display());
            return ExitCode::from(3);
        }
    };
    if !meta.is_file() {
        eprintln!("FATAL: {} is not a regular file", path.display());
        return ExitCode::from(3);
    }
    if meta.len() > MAX_BTF_BYTES {
        eprintln!(
            "FATAL: {}: {} bytes exceeds the {MAX_BTF_BYTES}-byte cap",
            path.display(),
            meta.len()
        );
        return ExitCode::from(3);
    }
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("FATAL: {}: {e}", path.display());
            return ExitCode::from(3);
        }
    };

    let verdict = scx_btf_from_bytes(&raw);
    println!("kernel BTF gate: {} ({} bytes)", path.display(), raw.len());
    println!("  verdict : {}", verdict.verdict());
    println!("  reading : {}", verdict.describe());

    let code = exit_code_for(&verdict);
    match &verdict {
        ScxBtf::Usable => {
            println!("PASS: this kernel can load a sched-ext scheduler.");
        }
        ScxBtf::ImplicitArgs { affected, examined } => {
            eprintln!();
            eprintln!(
                "FATAL: {} of {examined} scx_bpf_* kfuncs carry the implicit \
                 'struct bpf_prog_aux *' argument.",
                affected.len()
            );
            eprintln!("Affected kfuncs:");
            for name in affected {
                eprintln!("  {name}");
            }
            eprintln!();
            eprintln!(
                "This is the defect APEX builds its own kernel to avoid. The cause is \
                 pahole failing to emit a `bpf_kfunc` DECL_TAG for these functions, so \
                 resolve_btfids never strips the argument. Check that the buildroot's \
                 dwarves is the pinned version (kernel/kernel.pin) and that \
                 CONFIG_PAHOLE_VERSION in the built config matches it."
            );
        }
        ScxBtf::NoSchedExtKfuncs => {
            eprintln!();
            eprintln!(
                "FATAL: this kernel publishes no scx_bpf_* kfuncs at all — it was built \
                 without CONFIG_SCHED_CLASS_EXT. APEX's Gaming Mode scheduler tier needs \
                 them, so this is a config regression, not a clean kernel."
            );
        }
        ScxBtf::Absent | ScxBtf::Unreadable(_) => {
            eprintln!();
            eprintln!(
                "FATAL: the gate could not read this BTF, so it is refusing rather than \
                 passing. Could-not-check is not checked."
            );
        }
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    //! The gate's whole contract is "exit 0 on `Usable` and on nothing else",
    //! so these assert it variant by variant rather than in the aggregate. A
    //! row per variant is what makes a mutation name itself: widening the pass
    //! arm to include `Absent` turns exactly one row red and says which.

    use super::exit_code_for;
    use apexd_core::kernelbtf::ScxBtf;

    #[test]
    fn a_usable_kernel_is_the_only_thing_that_passes() {
        assert_eq!(exit_code_for(&ScxBtf::Usable), 0);
    }

    #[test]
    fn the_kfunc_defect_this_tier_exists_to_prevent_fails_the_build() {
        let v = ScxBtf::ImplicitArgs {
            affected: vec!["scx_bpf_get_idle_cpumask".to_string()],
            examined: 68,
        };
        assert_ne!(
            exit_code_for(&v),
            0,
            "a kernel carrying the implicit-args defect must not be shippable — \
             that is the entire reason this gate exists"
        );
    }

    #[test]
    fn a_kernel_with_no_sched_ext_at_all_fails_rather_than_passing_vacuously() {
        // It has no *defect*, so it is tempting to let it through. But Gaming
        // Mode's scheduler tier needs these kfuncs to exist, and a config
        // regression that deleted sched_ext would otherwise sail through the
        // one gate whose subject is sched_ext -- while every kfunc-level
        // assertion passed, because there would be nothing left to inspect.
        assert_ne!(exit_code_for(&ScxBtf::NoSchedExtKfuncs), 0);
    }

    #[test]
    fn a_gate_that_could_not_see_refuses_instead_of_passing() {
        // `blocks_loading()` is right that these say nothing about the kernel.
        // It is the wrong question for a build gate, and folding the two
        // questions together is how "could not check" becomes "checked".
        assert_ne!(exit_code_for(&ScxBtf::Absent), 0);
        assert_ne!(
            exit_code_for(&ScxBtf::Unreadable("truncated header".into())),
            0
        );
    }

    #[test]
    fn the_gate_does_not_simply_mirror_blocks_loading() {
        // The failing-but-not-blocking readings are the ones a second reader
        // would most plausibly get wrong, so this states the divergence as a
        // fact rather than leaving it in a comment: `blocks_loading()` is
        // false for both, and both still fail the build.
        for v in [ScxBtf::Absent, ScxBtf::Unreadable("x".into())] {
            assert!(!v.blocks_loading(), "precondition: {v:?} does not blame the kernel");
            assert_ne!(exit_code_for(&v), 0, "{v:?} must still fail the build");
        }
    }
}
