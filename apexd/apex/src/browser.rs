//! `apex browser` — P2-012's secure browser automation capsule, as a clap
//! surface over the shipped engine.
//!
//! A typed enum rather than a raw passthrough, for the reason `VmCmd` is one:
//! `apex browser run --help` documents the real thing, and a typo is caught
//! before a capsule directory exists. The engine
//! (`/usr/libexec/apex-browser`) still owns every decision; this builds its
//! argv.
//!
//! ## Why this argv is worth pinning with tests
//!
//! Three of the options below are not conveniences. They are the difference
//! between a capsule that contains something and one that reports that it
//! does:
//!
//!   * `--allow` decides where a browser that may be automating a hostile page
//!     can reach. A capsule with none is refused rather than given the
//!     runtime's whole allowlist, because "no destinations" and "every
//!     destination" are the two answers a dropped flag could turn into each
//!     other.
//!   * `--download` and `--download-to` are the only path by which a file
//!     leaves. A nomination that failed to reach the engine is a file the
//!     caller asked for and did not get, which is loud; a `--download-to` that
//!     failed to reach it would be worse, because everything would be reported
//!     copied.
//!   * `--capability` replaces the destination with a credential's pin. A
//!     dropped one is a capsule that goes where the call site said instead of
//!     where the capability said, which is the whole point of a pin.
//!
//! ## There is no way to open a window from this surface
//!
//! `apex vm` asserts the absence of `--graphics` and a viewer verb. The same
//! rule applies here and for a stronger reason: a browser is the program most
//! likely to be handed a display by accident. There is no `--headed`, no
//! `--display` and no `--profile` pointing at the user's own, and the test at
//! the bottom fails if one appears.

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum BrowserCmd {
    /// What a browser capsule needs, and what is missing.
    ///
    /// Each row observes the capability rather than assuming it: a browser
    /// that exists and cannot be executed is a different answer from no
    /// browser, and an agent runtime that is installed but does not answer is
    /// a different answer from one that refuses.
    Doctor,
    /// Run a browser in a capsule and delete the capsule afterwards.
    ///
    /// Its own profile, its own cookie jar, its own download directory, and no
    /// route onto the network except the destinations named here. Everything
    /// after `--` goes to the browser.
    Run(RunArgs),
}

#[derive(Args)]
pub struct RunArgs {
    /// A destination this capsule may reach, as `HOST` or `HOST:PORT`.
    ///
    /// Repeatable, and required unless `--capability` names one. It must
    /// already be on the runtime's allowlist (`apex agent allow`): the daemon
    /// snapshots that list when the session starts, so a capsule cannot widen
    /// it by asking, and naming something absent is refused with the line to
    /// add rather than becoming a capsule that silently reaches nothing.
    #[arg(long)]
    pub allow: Vec<String>,

    /// Take the destination from a stored credential's pinned host instead.
    ///
    /// The capsule is never given the credential's value — `apex secret list`
    /// does not print one and the engine never asks. What this decides is
    /// where the capsule may go, which is the half of the capability model a
    /// browser can honestly use. `docs/browser-capsule.md` records the half it
    /// cannot: a capsule cannot log in to a site, because agentd's proxy
    /// tunnels CONNECT and a TLS tunnel has nowhere to put a header.
    #[arg(long)]
    pub capability: Option<String>,

    /// A plain filename in the capsule's download directory that may leave it.
    ///
    /// Repeatable. No directory component, no `..`, no wildcard: the copy loop
    /// runs over these names and never over the directory's contents, and a
    /// wildcard would hand the choice of what leaves back to the page.
    #[arg(long)]
    pub download: Vec<String>,

    /// Where nominated downloads land.
    ///
    /// Without it NOTHING leaves the capsule, however much `--download`
    /// nominates. Refused if it resolves inside the capsule root, which
    /// teardown deletes.
    #[arg(long, value_name = "DIR")]
    pub download_to: Option<String>,

    /// Save what the capsule printed before it is deleted.
    #[arg(long, value_name = "FILE")]
    pub console_to: Option<String>,

    /// Let a nominated file replace one already at the destination.
    ///
    /// Off by default because the file was produced by a page the caller did
    /// not trust enough to open in their own browser, so replacing something
    /// of theirs with it is a decision rather than a default.
    #[arg(long)]
    pub force: bool,

    /// Name the capsule instead of generating one.
    #[arg(long)]
    pub name: Option<String>,

    /// How long to let the capsule run, in seconds.
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u32>,

    /// Arguments passed straight to the browser.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub fn argv(cmd: BrowserCmd) -> Vec<String> {
    match cmd {
        BrowserCmd::Doctor => vec!["doctor".to_string()],
        BrowserCmd::Run(a) => run_argv(a),
    }
}

fn run_argv(a: RunArgs) -> Vec<String> {
    let mut v = vec!["run".to_string()];
    if let Some(n) = a.name {
        v.push("--name".to_string());
        v.push(n);
    }
    for d in a.allow {
        v.push("--allow".to_string());
        v.push(d);
    }
    if let Some(c) = a.capability {
        v.push("--capability".to_string());
        v.push(c);
    }
    for d in a.download {
        v.push("--download".to_string());
        v.push(d);
    }
    if let Some(d) = a.download_to {
        v.push("--download-to".to_string());
        v.push(d);
    }
    if let Some(f) = a.console_to {
        v.push("--console-to".to_string());
        v.push(f);
    }
    if let Some(t) = a.timeout {
        v.push("--timeout".to_string());
        v.push(t.to_string());
    }
    if a.force {
        v.push("--force".to_string());
    }
    // Always emitted, even for an empty tail. The engine reads everything
    // after the separator as the browser's own words, and without it a
    // browser argument that starts with a dash would be read as one of the
    // engine's flags.
    v.push("--".to_string());
    v.extend(a.args);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: BrowserCmd,
    }

    fn build(args: &[&str]) -> Vec<String> {
        let mut full = vec!["apex-browser"];
        full.extend_from_slice(args);
        argv(Harness::try_parse_from(full).expect("parses").cmd)
    }

    fn refused(args: &[&str]) -> bool {
        let mut full = vec!["apex-browser"];
        full.extend_from_slice(args);
        Harness::try_parse_from(full).is_err()
    }

    #[test]
    fn doctor_is_one_word() {
        assert_eq!(build(&["doctor"]), vec!["doctor"]);
    }

    #[test]
    fn every_allowed_destination_reaches_the_engine() {
        // Each --allow is a separate flag-and-value pair. Joining them, or
        // dropping the second, would be a capsule that reaches less than the
        // caller asked for while reporting success.
        let a = build(&[
            "run", "--allow", "a.example:443", "--allow", "b.example:443", "--", "https://a.example/",
        ]);
        let pairs: Vec<_> = a.windows(2).filter(|w| w[0] == "--allow").map(|w| w[1].clone()).collect();
        assert_eq!(pairs, vec!["a.example:443", "b.example:443"]);
    }

    #[test]
    fn a_capsule_with_no_destination_is_refused_by_the_engine_not_here() {
        // The refusal belongs to the engine, which is the only place that
        // knows whether --capability supplied one. What this asserts is that
        // the surface does not quietly invent a destination: an argv with no
        // --allow and no --capability reaches the engine as exactly that.
        let a = build(&["run", "--", "https://example.com/"]);
        assert!(!a.iter().any(|x| x == "--allow"));
        assert!(!a.iter().any(|x| x == "--capability"));
    }

    #[test]
    fn a_nomination_and_its_destination_travel_together() {
        let a = build(&[
            "run", "--allow", "e.example", "--download", "report.csv", "--download-to", "/home/u/r",
            "--", "https://e.example/",
        ]);
        assert!(a.windows(2).any(|w| w[0] == "--download" && w[1] == "report.csv"));
        assert!(a.windows(2).any(|w| w[0] == "--download-to" && w[1] == "/home/u/r"));
    }

    #[test]
    fn a_capability_reaches_the_engine_so_the_pin_can_replace_the_destination() {
        let a = build(&["run", "--capability", "intranet", "--", "https://x/"]);
        assert!(a.windows(2).any(|w| w[0] == "--capability" && w[1] == "intranet"));
    }

    #[test]
    fn the_browser_arguments_keep_their_separator() {
        // Without the separator the engine reads `--screenshot` as one of its
        // own flags and refuses a capsule that was perfectly well formed.
        let a = build(&["run", "--allow", "e.example", "--", "--screenshot", "s.png", "https://e/"]);
        assert_eq!(&a[a.len() - 3..], &["--screenshot", "s.png", "https://e/"]);
        let sep = a.iter().position(|x| x == "--").expect("a separator");
        assert!(sep < a.len() - 3);
    }

    #[test]
    fn the_separator_is_emitted_even_with_no_browser_arguments() {
        let a = build(&["run", "--allow", "e.example"]);
        assert_eq!(a.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn there_is_no_way_to_open_a_window_or_name_a_real_profile() {
        // The headless rule and the fresh-profile rule, asserted rather than
        // trusted to review. A `--headed`, a `--display` or a `--profile`
        // arriving in a later change would point a browser at somebody's
        // screen, or at the cookie jar this verb exists to stay away from.
        //
        // Measured, not assumed, and the measurement changed the engine.
        // `--profile` typed before the separator is NOT rejected here: the
        // trailing var-arg absorbs it into the browser's own arguments, the
        // same way `apex vm run` absorbs a `--network`. On a Firefox command
        // line a second `--profile` then WINS over the one the engine passed,
        // so the capsule would run against a directory it neither created nor
        // deletes. What this test pins is that the word still reaches the
        // engine — where it is refused by name — rather than being silently
        // eaten here.
        let a = build(&["run", "--allow", "e", "--profile", "/home/u/.mozilla"]);
        let sep = a.iter().position(|x| x == "--").expect("a separator");
        assert!(
            a[sep..].iter().any(|x| x == "--profile"),
            "the engine never sees the --profile it has to refuse: {a:?}"
        );
        assert!(
            !a[..sep].iter().any(|x| x == "--profile"),
            "a --profile reached the engine as one of its own flags: {a:?}"
        );
        assert!(refused(&["viewer"]));
        let help = BrowserCmd::augment_subcommands(clap::Command::new("t"))
            .find_subcommand("run")
            .expect("run exists")
            .clone()
            .render_long_help()
            .to_string();
        for forbidden in ["--headed", "--display", "--profile", "--window", "--gui"] {
            assert!(!help.contains(forbidden), "run grew a {forbidden} option");
        }
    }

    #[test]
    fn help_names_the_capsule_and_not_a_browser_window() {
        let help = Harness::command().render_long_help().to_string();
        assert!(help.contains("capsule"), "{help}");
    }
}
