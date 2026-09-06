//! Agent adapters.
//!
//! An adapter is a small description of one upstream CLI: what binary to run,
//! how it takes a prompt, which of its own files it needs when the sandbox has
//! masked the home directory, and which environment variables carry its
//! credential. It is deliberately *not* a wrapper — the runtime launches the
//! real `claude`, `opencode`, `codex` or `gemini` binary and gets out of the
//! way, exactly as the roadmap's non-negotiable rules require.
//!
//! ## Why the home allowlist is per-adapter
//!
//! The sandbox masks `$HOME`. Everything an agent needs from it therefore has
//! to be named. That includes things it is easy to forget: `opencode` and
//! `codex` install into `~/.local/bin`, so without that entry the sandbox would
//! mask the binary the session is trying to run.
//!
//! ## State detection
//!
//! No adapter parses terminal output for meaning. Detection is the generic
//! path in [`crate::session`] — bell, OSC notifications, prompt markers, idle
//! and exit status — plus whatever the agent publishes through
//! `apex agent event`. Recognising a permission prompt by pattern-matching a
//! TUI's output would break the first time upstream changed a string, and
//! would report the wrong thing rather than nothing.
//!
//! ## The agent's own permission mode
//!
//! Dimension 1 of §3.1, and the only dimension that is a property of the
//! upstream CLI rather than of APEX. Each adapter declares the arguments that
//! select it, because every one of these tools spells it differently, and an
//! adapter that cannot express a mode says so instead of silently ignoring it:
//! a `--agent-bypass` that quietly did nothing would leave the user believing
//! confirmations were off while the agent kept asking, or the reverse.
//!
//! The arguments below were read from the installed binaries' `--help`, not
//! from memory. `codex` deliberately gets `-a never` rather than
//! `--dangerously-bypass-approvals-and-sandbox`: that flag also removes
//! *codex's own* sandbox, and dimension 1 is the approval policy. Removing a
//! confinement layer nobody asked about is the exact collapse §3.1 forbids.

use std::path::PathBuf;

use crate::policy::NativeMode;
use crate::sandbox::SandboxSpec;

/// Toolchain state shared by every adapter, relative to `$HOME`.
///
/// Writable, because a build that cannot populate its cache is a build that
/// fails: `cargo` writes to `~/.cargo/registry`, `npm` to `~/.npm`, `go` to
/// its module cache.
const TOOLCHAIN_RW: &[&str] = &[
    ".cargo",
    ".rustup",
    ".npm",
    ".cache/pip",
    ".cache/uv",
    ".cache/go-build",
    ".local/share/pnpm",
    ".local/share/uv",
    ".local/share/virtualenvs",
    "go/pkg/mod",
];

/// Read-only home state every adapter gets: the tools themselves and the
/// configuration a build reads but must not rewrite.
const TOOLCHAIN_RO: &[&str] = &[
    ".local/bin",
    ".gitconfig",
    ".config/git",
    ".local/share/mise",
    ".asdf",
];

/// Credential files that live inside an allowlisted directory and are blanked
/// out again afterwards. Relative to `$HOME`.
///
/// These are the cases default-deny cannot reach on its own: a toolchain cache
/// the agent genuinely needs that happens to store a token beside it.
const CREDENTIAL_MASKS: &[&str] = &[
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".npmrc",
    ".config/git/credentials",
];

/// One upstream agent CLI.
#[derive(Debug, Clone)]
pub struct Adapter {
    /// Stable short name used by `apex agent run <id>` and stored in session
    /// records.
    pub id: &'static str,
    /// Human-facing name.
    pub display: &'static str,
    /// The binary to execute. Resolved through `PATH` at spawn time, so a
    /// user's own build of an agent still wins.
    pub program: &'static str,
    /// Home-relative paths this agent needs to write (its own session store).
    pub home_rw: &'static [&'static str],
    /// Home-relative paths this agent needs to read.
    pub home_ro: &'static [&'static str],
    /// Environment variables carrying this agent's credentials or endpoint
    /// configuration, inherited when present. Nothing else is inherited.
    pub env_pass: &'static [&'static str],
    /// Arguments that put this agent into [`NativeMode::Bypass`]. Empty when
    /// its CLI has no such control.
    pub native_bypass: &'static [&'static str],
    /// Arguments that put this agent into [`NativeMode::Ask`]. Empty when its
    /// CLI has no such control, which for most of them means asking is already
    /// the default and there is nothing to select.
    pub native_ask: &'static [&'static str],
    /// Whether upstream itself refuses its bypass mode when the process is
    /// root.
    ///
    /// Claude does — measured: it exits with "cannot be used with root/sudo
    /// privileges for security reasons". The runtime supports running as root,
    /// so without this the user would get an unexplained upstream error
    /// instead of a refusal that says which of the two rules stopped them.
    pub native_bypass_refused_as_root: bool,
}

/// Every adapter the runtime knows, in listing order.
///
/// `generic` is last and is the fallback for any binary without a specific
/// entry — the roadmap's `GenericPTYAdapter`. It is what makes "never require
/// a specific agent" true rather than aspirational.
pub const ADAPTERS: &[Adapter] = &[
    Adapter {
        id: "claude",
        display: "Claude Code",
        program: "claude",
        home_rw: &[".claude", ".claude.json"],
        home_ro: &[],
        env_pass: &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ],
        native_bypass: &["--permission-mode", "bypassPermissions"],
        native_ask: &["--permission-mode", "manual"],
        native_bypass_refused_as_root: true,
    },
    Adapter {
        id: "opencode",
        display: "OpenCode",
        program: "opencode",
        home_rw: &[".local/share/opencode", ".config/opencode", ".cache/opencode"],
        home_ro: &[],
        env_pass: &[
            "OPENCODE_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        native_bypass: &["--auto"],
        native_ask: &[],
        native_bypass_refused_as_root: false,
    },
    Adapter {
        id: "codex",
        display: "Codex CLI",
        program: "codex",
        home_rw: &[".codex"],
        home_ro: &[],
        env_pass: &["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"],
        // Not `--dangerously-bypass-approvals-and-sandbox`: that also removes
        // codex's own sandbox, and dimension 1 is the approval policy alone.
        native_bypass: &["-a", "never"],
        native_ask: &["-a", "on-request"],
        native_bypass_refused_as_root: false,
    },
    Adapter {
        id: "gemini",
        display: "Gemini CLI",
        program: "gemini",
        home_rw: &[".gemini", ".config/google-generativeai"],
        home_ro: &[],
        env_pass: &["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENAI_USE_VERTEXAI"],
        // Left empty rather than guessed: the binary was not installed on the
        // machine the others were read from, and a wrong flag here is a
        // session that will not start.
        native_bypass: &[],
        native_ask: &[],
        native_bypass_refused_as_root: false,
    },
    Adapter {
        id: "kimi",
        display: "Kimi CLI",
        program: "kimi",
        home_rw: &[".kimi", ".config/kimi"],
        home_ro: &[],
        env_pass: &["KIMI_API_KEY", "MOONSHOT_API_KEY"],
        // `--auto`, not `-y/--yolo`: yolo still lets the agent ask questions.
        native_bypass: &["--auto"],
        native_ask: &[],
        native_bypass_refused_as_root: false,
    },
    Adapter {
        id: "generic",
        display: "Generic PTY",
        program: "",
        home_rw: &[],
        home_ro: &[],
        env_pass: &[],
        native_bypass: &[],
        native_ask: &[],
        native_bypass_refused_as_root: false,
    },
];

/// The adapter used when the user has expressed no preference.
pub const DEFAULT_AGENT: &str = "claude";

/// Look an adapter up by id.
pub fn by_id(id: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|a| a.id == id)
}

/// The `generic` adapter, which is guaranteed to exist.
pub fn generic() -> &'static Adapter {
    by_id("generic").expect("the generic adapter is compiled in")
}

/// Recognise an already-running program as one of the known agents, for
/// sessions the runtime adopts rather than starts.
pub fn by_program(program: &str) -> &'static Adapter {
    let base = program.rsplit('/').next().unwrap_or(program);
    ADAPTERS
        .iter()
        .find(|a| !a.program.is_empty() && a.program == base)
        .unwrap_or_else(generic)
}

/// Every adapter id, for `--help` text and shell completion.
pub fn ids() -> Vec<&'static str> {
    ADAPTERS.iter().map(|a| a.id).collect()
}

impl Adapter {
    /// The program this adapter runs. For `generic` the caller supplies it,
    /// since the whole point is that it launches anything.
    pub fn resolve_program(&self, explicit: Option<&str>) -> Option<String> {
        match explicit {
            Some(p) if !p.is_empty() => Some(p.to_string()),
            _ if !self.program.is_empty() => Some(self.program.to_string()),
            _ => None,
        }
    }

    /// The arguments that select `mode` for this agent, or `None` when this
    /// adapter has no way to express it.
    ///
    /// [`NativeMode::Inherit`] is always expressible and is always the empty
    /// list: §4.1 says APEX passes nothing and lets the agent's own profile
    /// decide, so "inherit" is the absence of a flag rather than a flag.
    pub fn native_mode_args(&self, mode: NativeMode) -> Option<Vec<String>> {
        let flags = match mode {
            NativeMode::Inherit => return Some(Vec::new()),
            NativeMode::Bypass => self.native_bypass,
            NativeMode::Ask => self.native_ask,
        };
        if flags.is_empty() {
            return None;
        }
        Some(flags.iter().map(|s| s.to_string()).collect())
    }

    /// Why this adapter will not run in `mode`, or `None` when it will.
    ///
    /// A pure function of the adapter, the mode and whether the runtime is
    /// root, so the refusal is testable without spawning anything.
    pub fn refuses_native_mode(&self, mode: NativeMode, as_root: bool) -> Option<String> {
        if self.native_mode_args(mode).is_none() {
            return Some(format!(
                "{} has no flag for the {mode} permission mode, so APEX cannot select it; \
                 run with `--native inherit` and set it in the agent's own configuration",
                self.display
            ));
        }
        if as_root && mode == NativeMode::Bypass && self.native_bypass_refused_as_root {
            return Some(format!(
                "{} refuses its bypass permission mode when it runs as root, so the session \
                 would exit immediately; run the agent as your own user, or use \
                 `--native inherit`",
                self.display
            ));
        }
        None
    }

    /// Build the argument list.
    ///
    /// A prompt is passed as a single trailing positional argument, which is
    /// the form every one of these CLIs accepts for an opening instruction.
    /// No flags are invented beyond the permission mode the policy asked for,
    /// and that one comes first so anything in `extra` — which the user
    /// controls — still overrides it.
    pub fn build_args(
        &self,
        native: NativeMode,
        prompt: Option<&str>,
        extra: &[String],
    ) -> Vec<String> {
        let mut args: Vec<String> = self.native_mode_args(native).unwrap_or_default();
        args.extend(extra.iter().cloned());
        if let Some(p) = prompt {
            if !p.is_empty() {
                args.push(p.to_string());
            }
        }
        args
    }

    /// Add this adapter's home requirements, the shared toolchain state and the
    /// credential masks to a sandbox spec.
    pub fn apply_sandbox(&self, spec: &mut SandboxSpec) {
        let home = spec.home.clone();
        let join = |rel: &str| -> PathBuf { home.join(rel) };

        for rel in TOOLCHAIN_RW.iter().chain(self.home_rw.iter()) {
            let p = join(rel);
            if !spec.rw.contains(&p) {
                spec.rw.push(p);
            }
        }
        for rel in TOOLCHAIN_RO.iter().chain(self.home_ro.iter()) {
            let p = join(rel);
            if !spec.ro.contains(&p) {
                spec.ro.push(p);
            }
        }
        for rel in CREDENTIAL_MASKS {
            let p = join(rel);
            if !spec.mask.contains(&p) {
                spec.mask.push(p);
            }
        }
        for name in self.env_pass {
            let name = name.to_string();
            if !spec.env_pass.contains(&name) {
                spec.env_pass.push(name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        SandboxSpec::new(
            crate::policy::AgentPolicy::default(),
            PathBuf::from("/home/tester"),
            PathBuf::from("/run/user/1000"),
        )
    }

    #[test]
    fn every_adapter_id_is_unique() {
        let mut ids = ids();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate adapter id");
    }

    #[test]
    fn the_default_agent_exists() {
        assert!(by_id(DEFAULT_AGENT).is_some());
    }

    #[test]
    fn the_generic_adapter_exists_and_has_no_program_of_its_own() {
        let g = generic();
        assert_eq!(g.id, "generic");
        assert!(g.program.is_empty());
        assert_eq!(g.resolve_program(None), None);
        assert_eq!(g.resolve_program(Some("htop")), Some("htop".to_string()));
    }

    #[test]
    fn an_unknown_program_falls_back_to_generic() {
        assert_eq!(by_program("some-new-agent").id, "generic");
        assert_eq!(by_program("").id, "generic");
    }

    #[test]
    fn a_known_program_is_recognised_through_an_absolute_path() {
        assert_eq!(by_program("/usr/bin/claude").id, "claude");
        assert_eq!(by_program("/home/andre/.local/bin/codex").id, "codex");
        assert_eq!(by_program("opencode").id, "opencode");
    }

    #[test]
    fn a_prompt_becomes_the_last_positional_argument() {
        let a = by_id("claude").unwrap();
        let inherit = NativeMode::Inherit;
        assert_eq!(
            a.build_args(inherit, Some("fix the tests"), &[]),
            ["fix the tests"]
        );
        assert_eq!(
            a.build_args(inherit, Some("go"), &["--verbose".to_string()]),
            ["--verbose", "go"]
        );
        assert!(a.build_args(inherit, None, &[]).is_empty());
        // An empty prompt is not an argument.
        assert!(a.build_args(inherit, Some(""), &[]).is_empty());
    }

    #[test]
    fn inherit_passes_no_permission_flag_at_all() {
        // §4.1: "If Claude's profile defaults to bypassPermissions, APEX should
        // not override it." The only way to not override a setting is to say
        // nothing about it, so inherit must add no argument for any adapter.
        for a in ADAPTERS {
            assert_eq!(
                a.native_mode_args(NativeMode::Inherit),
                Some(Vec::new()),
                "{} added an argument for inherit",
                a.id
            );
            assert!(a.build_args(NativeMode::Inherit, None, &[]).is_empty(), "{}", a.id);
        }
    }

    #[test]
    fn the_permission_mode_comes_first_so_the_user_can_still_override_it() {
        let a = by_id("claude").unwrap();
        let args = a.build_args(
            NativeMode::Bypass,
            Some("go"),
            &["--permission-mode".to_string(), "plan".to_string()],
        );
        assert_eq!(
            args,
            [
                "--permission-mode",
                "bypassPermissions",
                "--permission-mode",
                "plan",
                "go"
            ]
        );
    }

    #[test]
    fn an_adapter_that_cannot_express_a_mode_refuses_it_instead_of_ignoring_it() {
        // Silently dropping the flag would leave the user believing
        // confirmations were off while the agent kept asking.
        let gemini = by_id("gemini").unwrap();
        assert_eq!(gemini.native_mode_args(NativeMode::Bypass), None);
        let why = gemini
            .refuses_native_mode(NativeMode::Bypass, false)
            .expect("a refusal");
        assert!(why.contains("inherit"), "{why}");

        // opencode can bypass but has no flag for forcing prompts.
        let opencode = by_id("opencode").unwrap();
        assert!(opencode.refuses_native_mode(NativeMode::Bypass, false).is_none());
        assert!(opencode.refuses_native_mode(NativeMode::Ask, false).is_some());

        // And inherit is never refused, by anything, ever.
        for a in ADAPTERS {
            for as_root in [false, true] {
                assert!(
                    a.refuses_native_mode(NativeMode::Inherit, as_root).is_none(),
                    "{} refused inherit",
                    a.id
                );
            }
        }
    }

    #[test]
    fn claudes_own_refusal_of_bypass_as_root_is_reported_before_the_session_starts() {
        // Measured: `claude --permission-mode bypassPermissions` under sudo
        // prints "cannot be used with root/sudo privileges for security
        // reasons" and exits. The runtime supports root sessions, so the user
        // needs to be told which rule stopped them rather than watching a
        // session die on start.
        let claude = by_id("claude").unwrap();
        assert!(claude.refuses_native_mode(NativeMode::Bypass, false).is_none());
        let why = claude
            .refuses_native_mode(NativeMode::Bypass, true)
            .expect("a refusal as root");
        assert!(why.contains("root"), "{why}");
        // The restriction is upstream's and applies to bypass alone.
        assert!(claude.refuses_native_mode(NativeMode::Ask, true).is_none());
    }

    #[test]
    fn local_bin_is_allowed_or_agents_installed_there_cannot_run() {
        // opencode and codex install to ~/.local/bin. The home tmpfs would
        // otherwise mask the very binary the session is trying to execute.
        let mut s = spec();
        by_id("opencode").unwrap().apply_sandbox(&mut s);
        assert!(s.ro.contains(&PathBuf::from("/home/tester/.local/bin")));
    }

    #[test]
    fn an_adapter_gets_its_own_config_writable() {
        let mut s = spec();
        by_id("claude").unwrap().apply_sandbox(&mut s);
        assert!(s.rw.contains(&PathBuf::from("/home/tester/.claude")));
        assert!(s.rw.contains(&PathBuf::from("/home/tester/.claude.json")));
    }

    #[test]
    fn one_adapter_does_not_get_another_adapters_configuration() {
        let mut s = spec();
        by_id("claude").unwrap().apply_sandbox(&mut s);
        assert!(!s.rw.contains(&PathBuf::from("/home/tester/.codex")));
        assert!(!s.rw.contains(&PathBuf::from("/home/tester/.gemini")));
        assert!(!s.env_pass.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn toolchain_caches_are_writable_because_a_build_writes_them() {
        let mut s = spec();
        generic().apply_sandbox(&mut s);
        for rel in [".cargo", ".npm", "go/pkg/mod"] {
            assert!(
                s.rw.contains(&PathBuf::from(format!("/home/tester/{rel}"))),
                "{rel} must be writable"
            );
        }
    }

    #[test]
    fn credential_files_inside_allowed_directories_are_masked() {
        let mut s = spec();
        by_id("claude").unwrap().apply_sandbox(&mut s);
        // ~/.cargo is writable for the registry cache, so the token beside it
        // has to be blanked explicitly.
        assert!(s.rw.contains(&PathBuf::from("/home/tester/.cargo")));
        assert!(s
            .mask
            .contains(&PathBuf::from("/home/tester/.cargo/credentials.toml")));
        assert!(s.mask.contains(&PathBuf::from("/home/tester/.npmrc")));
    }

    #[test]
    fn masks_survive_into_the_built_argv_after_the_binds() {
        use crate::sandbox::build_argv;
        let mut s = spec();
        s.cwd = PathBuf::from("/home/tester/p");
        by_id("claude").unwrap().apply_sandbox(&mut s);
        let argv = build_argv(&s, "claude", &[]).unwrap();

        let cargo_bind = argv
            .windows(3)
            .position(|w| w[0] == "--bind-try" && w[1] == "/home/tester/.cargo")
            .expect("cargo bind");
        let cred_mask = argv
            .windows(3)
            .position(|w| {
                w[0] == "--ro-bind-try"
                    && w[1] == "/dev/null"
                    && w[2] == "/home/tester/.cargo/credentials.toml"
            })
            .expect("credential mask");
        assert!(
            cargo_bind < cred_mask,
            "the mask must come after the bind that would otherwise expose it"
        );
    }

    #[test]
    fn no_adapter_declares_a_secret_that_is_not_its_own() {
        // A stray SSH_AUTH_SOCK or GITHUB_TOKEN in a passthrough list would
        // quietly hand every session a credential the sandbox is masking.
        for a in ADAPTERS {
            for name in a.env_pass {
                assert!(
                    !matches!(*name, "SSH_AUTH_SOCK" | "GPG_AGENT_INFO" | "GITHUB_TOKEN"),
                    "{} declares {name}",
                    a.id
                );
            }
        }
    }

    #[test]
    fn applying_the_same_adapter_twice_does_not_duplicate_entries() {
        let mut s = spec();
        let a = by_id("claude").unwrap();
        a.apply_sandbox(&mut s);
        let rw = s.rw.len();
        let masks = s.mask.len();
        let env = s.env_pass.len();
        a.apply_sandbox(&mut s);
        assert_eq!(s.rw.len(), rw);
        assert_eq!(s.mask.len(), masks);
        assert_eq!(s.env_pass.len(), env);
    }
}
