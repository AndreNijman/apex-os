//! `apex disposable` — §19's disposable execution, as a clap surface over the
//! shipped engine.
//!
//! A separate enum rather than a raw argument passthrough, for exactly the
//! reason `EnvCmd` and `PluginCmd` are: `apex disposable --help` documents the
//! real thing, and a typo is caught before a process is spawned. The engine
//! still owns every decision; this only builds its argv.
//!
//! That argv is worth pinning by a test. `--copy-out` decides whether anything
//! at all leaves a disposable environment, and `--force` decides whether it may
//! overwrite a file on the host — a dropped flag here is not a compile error,
//! it is a silent policy change.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum DisposableCmd {
    /// Print exactly what a run would create, mount, copy and delete —
    /// and change nothing.
    ///
    /// This is §19's "clear copy-in/copy-out boundaries" surface: it names
    /// every host path the environment can read, every one it can write, what
    /// is copied in, what may leave, and what teardown removes. Run it before
    /// a `run` you have not run before.
    Plan(RunArgs),
    /// Create a disposable capsule, run in it, and delete the whole thing when
    /// it closes.
    ///
    /// Without a trailing command it opens an interactive shell. Teardown
    /// happens when that shell exits, including on Ctrl-C.
    Run(RunArgs),
    /// Disposable environments that exist right now.
    ///
    /// Normally none: a run deletes its own. An entry here is one whose
    /// teardown did not complete — a machine that lost power mid-run — and
    /// `apex disposable purge` removes them.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove one disposable environment by name.
    Rm {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Remove every disposable environment.
    Purge,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// The container image, instead of the capsule engine's default.
    #[arg(long, value_name = "REF")]
    pub image: Option<String>,
    /// A host path to COPY in, readable at ~/in/<name> inside.
    ///
    /// A copy, not a bind: whatever happens inside cannot change the host
    /// original. Repeatable. Nothing is copied in by default.
    #[arg(long, value_name = "PATH")]
    pub copy_in: Vec<String>,
    /// Where the contents of ~/out inside the capsule are copied when it
    /// closes.
    ///
    /// Without this, NOTHING leaves the environment. An existing file at the
    /// destination is never overwritten unless --force says so.
    #[arg(long, value_name = "DIR")]
    pub copy_out: Option<String>,
    /// Allow --copy-out to overwrite files that already exist.
    #[arg(long)]
    pub force: bool,
    /// Clone a git repository into ~/in inside the capsule and start there.
    ///
    /// The clone runs INSIDE the disposable environment, so the host's git
    /// configuration and credential helpers are never used for it. This is
    /// §19's "run untrusted GitHub projects in disposable capsules".
    #[arg(long, value_name = "URL")]
    pub git: Option<String>,
    /// Name the environment instead of generating one. Prefixed `disp-`
    /// either way.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// The command to run inside. Omit for an interactive shell.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// Build the engine argv.
///
/// The `--` before a trailing command is not cosmetic: without it the engine
/// cannot tell `apex disposable run -- ls -l` from a flag of its own, and clap
/// has already stripped the separator the user typed.
pub fn argv(cmd: DisposableCmd) -> Vec<String> {
    match cmd {
        DisposableCmd::Plan(a) => run_argv("plan", a),
        DisposableCmd::Run(a) => run_argv("run", a),
        DisposableCmd::List { json } => {
            let mut v = vec!["list".to_string()];
            if json {
                v.push("--json".to_string());
            }
            v
        }
        DisposableCmd::Rm { name } => vec!["rm".to_string(), name],
        DisposableCmd::Purge => vec!["purge".to_string()],
    }
}

fn run_argv(verb: &str, a: RunArgs) -> Vec<String> {
    let mut v = vec![verb.to_string()];
    if let Some(i) = a.image {
        v.push(format!("--image={i}"));
    }
    for p in a.copy_in {
        v.push(format!("--copy-in={p}"));
    }
    if let Some(o) = a.copy_out {
        v.push(format!("--copy-out={o}"));
    }
    if a.force {
        v.push("--force".to_string());
    }
    if let Some(g) = a.git {
        v.push(format!("--git={g}"));
    }
    if let Some(n) = a.name {
        v.push(format!("--name={n}"));
    }
    if !a.command.is_empty() {
        v.push("--".to_string());
        v.extend(a.command);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: DisposableCmd,
    }

    fn build(args: &[&str]) -> Vec<String> {
        let mut full = vec!["disposable"];
        full.extend_from_slice(args);
        argv(Harness::try_parse_from(full).expect("parses").cmd)
    }

    #[test]
    fn nothing_leaves_unless_copy_out_is_asked_for() {
        // THE default that matters. A disposable environment whose contents
        // reached the host by accident would not be disposable.
        let a = build(&["run"]);
        assert_eq!(a, vec!["run"]);
        assert!(!a.iter().any(|x| x.starts_with("--copy-out")));
        assert!(!a.iter().any(|x| x == "--force"));
    }

    #[test]
    fn every_flag_that_widens_the_boundary_reaches_the_engine() {
        // Each of these is a policy decision the engine enforces, and a
        // dropped one here is a silent widening rather than a compile error.
        let a = build(&[
            "run",
            "--copy-in",
            "/home/u/src",
            "--copy-in",
            "/home/u/data",
            "--copy-out",
            "/home/u/results",
            "--force",
            "--image",
            "docker.io/library/ubuntu:24.04",
            "--name",
            "review",
        ]);
        assert!(a.contains(&"--copy-in=/home/u/src".to_string()));
        assert!(a.contains(&"--copy-in=/home/u/data".to_string()));
        assert!(a.contains(&"--copy-out=/home/u/results".to_string()));
        assert!(a.contains(&"--force".to_string()));
        assert!(a.contains(&"--image=docker.io/library/ubuntu:24.04".to_string()));
        assert!(a.contains(&"--name=review".to_string()));
    }

    #[test]
    fn a_trailing_command_keeps_its_separator() {
        // Without the `--`, the engine reads `-l` as one of its own flags.
        let a = build(&["run", "--", "ls", "-l"]);
        assert_eq!(a, vec!["run", "--", "ls", "-l"]);
    }

    #[test]
    fn plan_and_run_build_the_same_arguments() {
        // The plan must describe the run, not a different run. Identical argv
        // apart from the verb is how that is guaranteed rather than intended.
        let plan = build(&["plan", "--copy-in", "/x", "--copy-out", "/y", "--", "make"]);
        let run = build(&["run", "--copy-in", "/x", "--copy-out", "/y", "--", "make"]);
        assert_eq!(plan[0], "plan");
        assert_eq!(run[0], "run");
        assert_eq!(plan[1..], run[1..]);
    }

    #[test]
    fn the_git_url_is_passed_through_for_the_engine_to_validate() {
        let a = build(&["run", "--git", "https://github.com/o/r"]);
        assert!(a.contains(&"--git=https://github.com/o/r".to_string()));
    }
}
