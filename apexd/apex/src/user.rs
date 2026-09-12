//! `apex user` — accounts on a shared machine, as a clap surface over the
//! shipped engine.
//!
//! A separate enum rather than a raw argument passthrough, for the reason
//! `DisposableCmd` is one: `apex user --help` documents the real thing, and a
//! typo is caught before a privileged process is spawned. The engine still
//! owns every decision — which accounts are administrators, which refusals
//! hold, what `useradd` is actually run with; this only builds its argv.
//!
//! That argv is worth pinning by a test, and more so here than anywhere else
//! in this binary. `--admin` is the whole of the standard/administrator
//! distinction P2-016's first criterion is about: it is the difference between
//! an account that can approve a root operation and one that cannot. A dropped
//! flag is not a compile error and not a visible failure — it is an
//! administrator where somebody asked for a standard account, or the reverse,
//! and nobody finds out until it matters.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum UserCmd {
    /// Who has an account on this machine, and which of them are
    /// administrators.
    ///
    /// Needs no root: "who can become root here" is not a privileged
    /// question, and making it one would mean the answer is only ever visible
    /// to somebody who already knows it.
    ///
    /// An administrator is a member of `wheel`, by primary group or by
    /// membership — that is what polkit's admin rule and sudoers both resolve
    /// to on APEX, so it is what `auth_admin` asks for on the agent actions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add an account. STANDARD unless `--admin` is given.
    ///
    /// The default is deliberately the opposite of what the installer does.
    /// `installer/apex-install` puts the one account it creates in `wheel`
    /// unconditionally, which is right for the machine's owner and wrong for
    /// everybody added afterwards: on a shared machine the second account
    /// should not be able to approve a root operation unless somebody says so.
    ///
    /// No password is set. The account cannot be logged into until `passwd
    /// <name>` is run, which is the safe direction and also the only one that
    /// works unattended — a command that reads a password from a terminal
    /// cannot be scripted and cannot be tested.
    Add {
        #[arg(value_name = "NAME")]
        name: String,
        /// Put the account in `wheel`: it can use sudo and answer polkit's
        /// `auth_admin`.
        ///
        /// This is the flag the whole subcommand exists to make explicit.
        #[arg(long)]
        admin: bool,
        /// The GECOS comment — a real name, or what the account is for.
        #[arg(long, value_name = "TEXT")]
        comment: Option<String>,
        /// Print what would be run and change nothing.
        #[arg(long)]
        plan: bool,
    },
    /// Remove an account and, unless told otherwise, its home directory.
    ///
    /// Refuses uid 0, a system account below `UID_MIN`, the account you are
    /// running as, and the last administrator on the machine — that last one
    /// because polkit's `auth_admin` and sudo both resolve to `wheel`, so an
    /// APEX with no member of it left cannot approve anything or become root
    /// again from inside the running system.
    ///
    /// It does NOT remove the account's credentials: P0-002 keeps those in
    /// `/var/lib/apex-secretd/users/<uid>/`, outside the home and root-owned,
    /// so `userdel -r` never sees them. The engine prints the path.
    Rm {
        #[arg(value_name = "NAME")]
        name: String,
        /// Leave the home directory where it is.
        #[arg(long)]
        keep_home: bool,
        /// Print what would be run and change nothing.
        #[arg(long)]
        plan: bool,
    },
    /// Disposable guest accounts (P2-016 criterion 2).
    Guest {
        #[command(subcommand)]
        cmd: GuestCmd,
    },
}

#[derive(Subcommand)]
pub enum GuestCmd {
    /// Which accounts, if any, are configured as disposable guests.
    Status,
    /// Make an account a disposable guest: everything of its own is erased
    /// when it logs out.
    ///
    /// This is destructive by design and there is no undo. At every logout
    /// APEX clears the account's home contents, its credential namespace at
    /// `/var/lib/apex-secretd/users/<uid>/` — which a tmpfs home does not,
    /// and which is the reason this exists — and the greeter's last-user when
    /// it names the guest.
    ///
    /// Refuses an administrator, the account you are running as, uid 0, a
    /// system account, and an account whose home the wipe would not clear.
    Enable {
        #[arg(value_name = "NAME")]
        name: String,
        /// Print what would be run and change nothing.
        #[arg(long)]
        plan: bool,
    },
    /// Stop treating an account as a disposable guest.
    ///
    /// Removes nothing of the account's: the last wipe has already happened.
    Disable {
        #[arg(value_name = "NAME")]
        name: String,
        /// Print what would be run and change nothing.
        #[arg(long)]
        plan: bool,
    },
}

/// Build the engine argv.
pub fn argv(cmd: UserCmd) -> Vec<String> {
    match cmd {
        UserCmd::List { json } => {
            let mut v = vec!["list".to_string()];
            if json {
                v.push("--json".to_string());
            }
            v
        }
        UserCmd::Add {
            name,
            admin,
            comment,
            plan,
        } => {
            let mut v = vec!["add".to_string(), name];
            // The one flag whose loss is a policy change rather than a
            // missing feature. Pinned by a test below.
            if admin {
                v.push("--admin".to_string());
            }
            if let Some(c) = comment {
                v.push("--comment".to_string());
                v.push(c);
            }
            if plan {
                v.push("--plan".to_string());
            }
            v
        }
        UserCmd::Rm {
            name,
            keep_home,
            plan,
        } => {
            let mut v = vec!["rm".to_string(), name];
            if keep_home {
                v.push("--keep-home".to_string());
            }
            if plan {
                v.push("--plan".to_string());
            }
            v
        }
        UserCmd::Guest { cmd } => match cmd {
            GuestCmd::Status => vec!["guest".to_string(), "status".to_string()],
            GuestCmd::Enable { name, plan } => guest_argv("enable", name, plan),
            GuestCmd::Disable { name, plan } => guest_argv("disable", name, plan),
        },
    }
}

fn guest_argv(verb: &str, name: String, plan: bool) -> Vec<String> {
    let mut v = vec!["guest".to_string(), verb.to_string(), name];
    if plan {
        v.push("--plan".to_string());
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
        cmd: UserCmd,
    }

    fn build(args: &[&str]) -> Vec<String> {
        let mut full = vec!["user"];
        full.extend_from_slice(args);
        argv(Harness::try_parse_from(full).expect("parses").cmd)
    }

    #[test]
    fn an_account_is_standard_unless_admin_is_asked_for() {
        // THE default of this whole subcommand, and the criterion it carries.
        // A `--admin` that leaked into the plain form would make every account
        // APEX creates an administrator, which is the state the installer is
        // in and the reason `apex user` exists.
        let a = build(&["add", "alice"]);
        assert_eq!(a, vec!["add", "alice"]);
        assert!(!a.iter().any(|x| x == "--admin"));
    }

    #[test]
    fn admin_is_the_only_way_to_reach_wheel() {
        // The counterpart. Without this the assertion above would still pass
        // with `--admin` deleted from the builder entirely, and `apex user add
        // --admin` would silently make a standard account — the same defect
        // pointing the other way, and the one that is discovered when somebody
        // cannot approve a request.
        assert_eq!(build(&["add", "alice", "--admin"]), vec!["add", "alice", "--admin"]);
    }

    #[test]
    fn a_comment_reaches_the_engine_as_one_argument() {
        // Two words, one argv slot. Split, the second word becomes the
        // account name of a second positional the engine then refuses — or
        // worse, does not.
        assert_eq!(
            build(&["add", "alice", "--comment", "Alice Smith"]),
            vec!["add", "alice", "--comment", "Alice Smith"]
        );
    }

    #[test]
    fn plan_is_carried_on_every_verb_that_changes_anything() {
        // `--plan` is how a caller finds out what a privileged command will do
        // before it does it. Dropped on the way to the engine, the command
        // runs for real and the caller is told it was a plan.
        for a in [
            build(&["add", "alice", "--plan"]),
            build(&["rm", "alice", "--plan"]),
            build(&["guest", "enable", "alice", "--plan"]),
            build(&["guest", "disable", "alice", "--plan"]),
        ] {
            assert!(a.iter().any(|x| x == "--plan"), "{a:?}");
        }
    }

    #[test]
    fn removing_an_account_takes_its_home_unless_keep_home_says_otherwise() {
        assert_eq!(build(&["rm", "alice"]), vec!["rm", "alice"]);
        assert_eq!(
            build(&["rm", "alice", "--keep-home"]),
            vec!["rm", "alice", "--keep-home"]
        );
    }

    #[test]
    fn the_guest_verbs_are_a_closed_set_and_reach_the_engine_whole() {
        assert_eq!(build(&["guest", "status"]), vec!["guest", "status"]);
        assert_eq!(
            build(&["guest", "enable", "apex-guest"]),
            vec!["guest", "enable", "apex-guest"]
        );
        assert_eq!(
            build(&["guest", "disable", "apex-guest"]),
            vec!["guest", "disable", "apex-guest"]
        );
        // And a verb the engine does not have is refused by clap, here, rather
        // than reaching a privileged program as an unknown word.
        assert!(Harness::try_parse_from(["user", "guest", "wipe", "apex-guest"]).is_err());
    }

    #[test]
    fn list_is_the_only_verb_that_needs_no_name() {
        assert_eq!(build(&["list"]), vec!["list"]);
        assert_eq!(build(&["list", "--json"]), vec!["list", "--json"]);
        for missing in [
            vec!["user", "add"],
            vec!["user", "rm"],
            vec!["user", "guest", "enable"],
            vec!["user", "guest", "disable"],
        ] {
            assert!(
                Harness::try_parse_from(missing.clone()).is_err(),
                "{missing:?} parsed without a name"
            );
        }
    }
}
