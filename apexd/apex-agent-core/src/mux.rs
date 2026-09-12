//! Terminal-multiplexer layout templates: an editor beside an agent beside a
//! terminal, in tmux or zellij.
//!
//! ## Which owns the PTY
//!
//! The daemon does, and that decision shapes everything else here.
//!
//! An agent session's PTY is created by `apex-agentd` ([`crate::protocol`] and
//! the daemon's `pty` module); the `apex` CLI never makes one. It is only ever
//! an attacher, proxying bytes over the control socket, and the session
//! survives it leaving. A multiplexer also owns PTYs, so composing the two has
//! exactly two possible shapes and only one of them works:
//!
//! - **A multiplexer pane runs `apex agent attach`.** Two PTYs in series: the
//!   multiplexer's, which is the transport, and the daemon's, which is the
//!   durable one. Detaching from tmux, killing tmux, or losing the terminal
//!   leaves the agent running.
//! - **The daemon runs a multiplexer as the session command.** The multiplexer
//!   server would then live inside the session's bwrap confinement — its
//!   socket, its other panes, and every program in them confined to one agent's
//!   policy. One session could hold only one agent. And the durable thing would
//!   be inside the ephemeral one, making the multiplexer a single point of
//!   failure for agent state.
//!
//! So the multiplexer is a VIEWPORT onto sessions the daemon owns, never a host
//! for them. That is why a template stores what each pane should RUN rather
//! than any session id, why reopening a template attaches to whatever sessions
//! now exist, and why nothing here has to survive a reboot: the panes are
//! rebuilt, and the agents they attach to are the daemon's business.
//!
//! Resize composes correctly for free. `apex agent attach` installs a SIGWINCH
//! handler that sends a `Resize` control frame, so a tmux reattach at a
//! different size reaches the daemon's PTY through `TIOCSWINSZ`. And the detach
//! key is `ctrl-]`, which collides with neither tmux's `C-b` nor zellij's
//! `Ctrl-p`.
//!
//! ## Not a second layout mechanism
//!
//! `apex project layout save|show|restore` remembers the DESKTOP WINDOWS a
//! project has open, captured live. This remembers the SHAPE a project's
//! terminal work takes, authored rather than captured. They are two fields of
//! one [`crate::layout::ProjectLayout`] and one store — `open` records the
//! template it used, so `show` reports both and reopening needs no argument.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The multiplexers APEX drives, in preference order.
///
/// Both ship in the image. tmux is first because it is the one people already
/// have configured; the choice is overridable with `--mux` or `$APEX_MUX`.
pub const BACKENDS: &[&str] = &["tmux", "zellij"];

/// Editors to look for, in preference order, after `$VISUAL` and `$EDITOR`.
///
/// The image installs neovim and nano; `nano` is last because it is the
/// fallback that is always present rather than the one anybody chose.
pub const EDITOR_CANDIDATES: &[&str] = &["nvim", "vim", "hx", "helix", "nano"];

/// What one pane of a template is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneKind {
    /// The editor, opened on the project root.
    Editor,
    /// An agent: attached to a live session of this project, or a new one.
    Agent,
    /// A plain shell in the project.
    Terminal,
    /// What the agent changed.
    Diff,
}

/// A named arrangement, in the vocabulary both multiplexers already have.
///
/// Deliberately only two. A template language with percentages and nesting
/// would be a third layout format to maintain, and the two shapes below are
/// what "editor beside an agent" and "several agents" actually need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    /// One large pane on the left, the rest stacked down the right.
    MainVertical,
    /// Equal panes filling the window.
    Tiled,
}

impl Arrangement {
    pub fn as_str(self) -> &'static str {
        match self {
            Arrangement::MainVertical => "main-vertical",
            Arrangement::Tiled => "tiled",
        }
    }
}

/// One of the named templates.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    pub name: &'static str,
    pub summary: &'static str,
    pub kinds: &'static [PaneKind],
    pub arrangement: Arrangement,
    /// Whether `--agents N` repeats the agent pane.
    pub repeats_agent: bool,
}

/// Every template. Three, because the roadmap asks for two shapes —
/// editor+agent+terminal, and multi-agent — and `review` is the same first
/// shape with the third pane pointed at the diff, which is the other half of
/// the sentence "an editor beside an agent beside a diff".
pub const TEMPLATES: &[Template] = &[
    Template {
        name: "dev",
        summary: "editor, agent, terminal",
        kinds: &[PaneKind::Editor, PaneKind::Agent, PaneKind::Terminal],
        arrangement: Arrangement::MainVertical,
        repeats_agent: false,
    },
    Template {
        name: "review",
        summary: "editor, agent, what it changed",
        kinds: &[PaneKind::Editor, PaneKind::Agent, PaneKind::Diff],
        arrangement: Arrangement::MainVertical,
        repeats_agent: false,
    },
    Template {
        name: "agents",
        summary: "several agents side by side",
        kinds: &[PaneKind::Agent],
        arrangement: Arrangement::Tiled,
        repeats_agent: true,
    },
];

pub fn template(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// One pane, resolved: what to run and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    /// Shown on the multiplexer's own pane title.
    pub title: String,
    pub cwd: String,
    /// Empty means "the shell the multiplexer would start anyway", which is the
    /// only honest way to say `$SHELL` — the user's login shell is the
    /// multiplexer's business and it already knows it.
    pub argv: Vec<String>,
}

/// A whole template, resolved against one project at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub session: String,
    pub arrangement: &'static str,
    pub panes: Vec<Pane>,
}

/// The multiplexer session name for a project: `apex-<name>-<digest>`.
///
/// Three parts, each earning its place. `apex-` so `tmux ls` says which
/// sessions are APEX's. The project's DIRECTORY NAME, not its path slug,
/// because this is what a status bar shows and what somebody types after
/// `tmux attach -t`, and `apex-var-home-andre-projects-apex-os` is neither
/// readable nor typable. And six hex of the root path, because a directory name
/// is not unique — `~/work/api` and `~/oss/api` are two projects — and two
/// projects sharing a session would silently attach one to the other's panes.
///
/// Sanitised because tmux does not mangle a `.` or `:` in a session name, it
/// errors: without this, a project directory with a dot in it would fail to
/// open with a message about tmux syntax.
pub fn session_name(name: &str, root: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    let base = if cleaned.is_empty() { "project" } else { cleaned };
    format!("apex-{base}-{}", short_digest(root))
}

/// Six hex characters of FNV-1a over the project root.
///
/// FNV rather than a hash crate: this is a name disambiguator, not a security
/// boundary, and the property it needs is that the same path always produces
/// the same six characters on every machine and every release — which a
/// `DefaultHasher` explicitly does not promise.
fn short_digest(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", h & 0xff_ffff)
}

/// Pick a multiplexer: `--mux`/`$APEX_MUX` if it names one that is installed,
/// otherwise the first installed of [`BACKENDS`].
///
/// `lookup` answers "is this program installed", so the choice is testable
/// without depending on what happens to be on the machine running the tests.
pub fn choose_backend<F>(preferred: Option<&str>, lookup: F) -> Result<String, String>
where
    F: Fn(&str) -> bool,
{
    if let Some(name) = preferred.filter(|n| !n.is_empty()) {
        if !BACKENDS.contains(&name) {
            return Err(format!(
                "{name} is not a multiplexer APEX drives — {}",
                BACKENDS.join(" or ")
            ));
        }
        if !lookup(name) {
            return Err(format!("{name} is not installed"));
        }
        return Ok(name.to_string());
    }
    for b in BACKENDS {
        if lookup(b) {
            return Ok((*b).to_string());
        }
    }
    Err(format!(
        "no multiplexer installed — this needs {}",
        BACKENDS.join(" or ")
    ))
}

/// The editor argv: `$VISUAL`, then `$EDITOR`, then the first installed of
/// [`EDITOR_CANDIDATES`].
///
/// `$VISUAL` before `$EDITOR` is the long-standing convention and it matters
/// here: `$EDITOR` is often set to something line-oriented for git, and opening
/// that full-screen in a pane is not what anybody meant.
///
/// A `$VISUAL` that is not installed falls through rather than being trusted,
/// because a pane whose command does not exist opens and dies immediately.
pub fn choose_editor<F>(visual: Option<&str>, editor: Option<&str>, lookup: F) -> Option<Vec<String>>
where
    F: Fn(&str) -> bool,
{
    for candidate in [visual, editor].into_iter().flatten() {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        // `$EDITOR` is allowed to carry flags — "code -w", "emacsclient -t".
        let mut words = candidate.split_whitespace();
        let program = words.next()?;
        if lookup(program) {
            let mut argv = vec![program.to_string()];
            argv.extend(words.map(str::to_string));
            return Some(argv);
        }
    }
    EDITOR_CANDIDATES
        .iter()
        .find(|e| lookup(e))
        .map(|e| vec![(*e).to_string()])
}

/// Resolve a template into a plan.
///
/// `live` is the ids of this project's running sessions, most recent first.
/// Each agent pane takes the next one and attaches to it; when they run out the
/// pane starts a new session instead. That is the whole of "attach and restore
/// cleanly": reopening after a reboot finds no live sessions and starts fresh
/// ones, and reopening while agents are working reattaches to those agents
/// rather than starting duplicates beside them.
pub fn build(
    t: &Template,
    name: &str,
    root: &Path,
    editor: Option<&[String]>,
    live: &[u32],
    agents: usize,
) -> Plan {
    let cwd = root.to_string_lossy().into_owned();
    let mut kinds: Vec<PaneKind> = t.kinds.to_vec();
    if t.repeats_agent && agents > 1 {
        let extra = agents.saturating_sub(1).min(MAX_AGENT_PANES - 1);
        for _ in 0..extra {
            kinds.push(PaneKind::Agent);
        }
    }

    let mut used = 0usize;
    let mut panes = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let (title, argv) = match kind {
            PaneKind::Editor => (
                "editor".to_string(),
                editor.map(<[String]>::to_vec).unwrap_or_default(),
            ),
            PaneKind::Agent => {
                let argv = match live.get(used) {
                    Some(id) => vec![
                        "apex".into(),
                        "agent".into(),
                        "attach".into(),
                        id.to_string(),
                    ],
                    None => vec!["apex".into(), "agent".into(), "run".into()],
                };
                let title = match live.get(used) {
                    Some(id) => format!("agent {id}"),
                    None => "agent".to_string(),
                };
                used += 1;
                (title, argv)
            }
            PaneKind::Terminal => ("terminal".to_string(), Vec::new()),
            PaneKind::Diff => (
                "diff".to_string(),
                vec!["apex".into(), "agent".into(), "diff".into()],
            ),
        };
        panes.push(Pane { title, cwd: cwd.clone(), argv });
    }

    Plan {
        session: session_name(name, &cwd),
        arrangement: t.arrangement.as_str(),
        panes,
    }
}

/// How many agent panes `--agents` will build.
///
/// Bounded because each pane is a real agent process with a real context
/// window, and "--agents 400" should be a refusal rather than a machine that
/// stops responding.
pub const MAX_AGENT_PANES: usize = 8;

/// The plan on the wire to the multiplexer adapter: one pane per line, tab
/// separated, `title <TAB> cwd <TAB> argv…`.
///
/// Tabs, and not a shell string, for the reason the window layout gives for
/// storing argv vectors: the adapter passes these to the multiplexer as
/// separate arguments, so nothing in a pane command can be read as a shell
/// metacharacter. A tab in a path would break the encoding, so it is rejected
/// rather than escaped — a path with a tab in it is not worth a quoting scheme
/// that has to be right in two languages.
pub fn encode(plan: &Plan) -> Result<String, String> {
    let mut out = String::new();
    for p in &plan.panes {
        for field in std::iter::once(&p.title).chain(std::iter::once(&p.cwd)).chain(p.argv.iter()) {
            if field.contains('\t') || field.contains('\n') {
                return Err(format!("a tab or newline in {field:?} cannot be passed to the multiplexer"));
            }
        }
        out.push_str(&p.title);
        out.push('\t');
        out.push_str(&p.cwd);
        for a in &p.argv {
            out.push('\t');
            out.push_str(a);
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(list: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |n: &str| list.contains(&n)
    }

    // ── backend choice ──────────────────────────────────────────────────────

    #[test]
    fn tmux_wins_when_both_are_installed() {
        assert_eq!(choose_backend(None, installed(&["tmux", "zellij"])), Ok("tmux".into()));
    }

    #[test]
    fn the_one_that_is_installed_wins() {
        assert_eq!(choose_backend(None, installed(&["zellij"])), Ok("zellij".into()));
    }

    #[test]
    fn an_explicit_choice_wins_over_the_order() {
        assert_eq!(
            choose_backend(Some("zellij"), installed(&["tmux", "zellij"])),
            Ok("zellij".into())
        );
    }

    #[test]
    fn an_explicit_choice_that_is_not_installed_is_refused_not_substituted() {
        // Silently opening tmux for somebody who asked for zellij would be a
        // surprise, and they would not find out until the keybindings were
        // wrong.
        let e = choose_backend(Some("zellij"), installed(&["tmux"])).unwrap_err();
        assert!(e.contains("not installed"), "{e}");
    }

    #[test]
    fn a_multiplexer_apex_does_not_drive_is_named_as_such() {
        let e = choose_backend(Some("screen"), installed(&["screen"])).unwrap_err();
        assert!(e.contains("tmux"), "{e}");
        assert!(e.contains("zellij"), "{e}");
    }

    #[test]
    fn no_multiplexer_at_all_is_reported_rather_than_invented() {
        let e = choose_backend(None, installed(&[])).unwrap_err();
        assert!(e.contains("no multiplexer installed"), "{e}");
    }

    // ── editor choice ───────────────────────────────────────────────────────

    #[test]
    fn visual_beats_editor() {
        // $EDITOR is routinely a line editor set for git. Opening that
        // full-screen in a pane is not what anybody meant.
        assert_eq!(
            choose_editor(Some("nvim"), Some("ed"), installed(&["nvim", "ed"])),
            Some(vec!["nvim".to_string()])
        );
    }

    #[test]
    fn an_editor_keeps_its_flags() {
        assert_eq!(
            choose_editor(Some("emacsclient -t"), None, installed(&["emacsclient"])),
            Some(vec!["emacsclient".to_string(), "-t".to_string()])
        );
    }

    #[test]
    fn an_uninstalled_editor_falls_through_rather_than_opening_a_dead_pane() {
        assert_eq!(
            choose_editor(Some("nosuchedit"), None, installed(&["nano"])),
            Some(vec!["nano".to_string()])
        );
    }

    #[test]
    fn an_empty_editor_variable_is_not_a_choice() {
        assert_eq!(
            choose_editor(Some(""), Some("  "), installed(&["nvim"])),
            Some(vec!["nvim".to_string()])
        );
    }

    #[test]
    fn no_editor_anywhere_is_reported_rather_than_guessed() {
        assert_eq!(choose_editor(None, None, installed(&[])), None);
    }

    // ── session names ───────────────────────────────────────────────────────

    #[test]
    fn a_session_name_is_readable_and_carries_no_character_tmux_refuses() {
        // tmux errors on `.` and `:` in a session name rather than mangling
        // them, which would surface as an unexplained failure at open time.
        let n = session_name("apex-os", "/home/a/Projects/apex-os");
        assert!(n.starts_with("apex-apex-os-"), "{n}");
        let n = session_name("my.proj:v2", "/p");
        assert!(n.starts_with("apex-my-proj-v2-"), "{n}");
        assert!(!n.contains('.') && !n.contains(':'), "{n}");
        // A name that sanitises away entirely still has to be a valid session.
        assert!(session_name("...", "/p").starts_with("apex-project-"));
    }

    #[test]
    fn two_projects_with_the_same_directory_name_get_different_sessions() {
        // Otherwise opening one would attach to the other's panes, silently.
        assert_ne!(
            session_name("api", "/home/a/work/api"),
            session_name("api", "/home/a/oss/api")
        );
    }

    #[test]
    fn a_session_name_is_stable_for_the_same_project() {
        // It is written into a saved record and typed after `tmux attach -t`,
        // so it has to be the same six characters on every machine and every
        // release — which is why this is FNV and not DefaultHasher.
        assert_eq!(
            session_name("demo", "/p/demo"),
            session_name("demo", "/p/demo")
        );
        assert_eq!(short_digest("/p/demo").len(), 6);
    }

    // ── building a plan ─────────────────────────────────────────────────────

    fn ed() -> Vec<String> {
        vec!["nvim".to_string()]
    }

    #[test]
    fn dev_is_an_editor_an_agent_and_a_terminal() {
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p/demo"), Some(&ed()), &[], 1);
        assert_eq!(plan.session, session_name("demo", "/p/demo"));
        assert_eq!(plan.arrangement, "main-vertical");
        let titles: Vec<&str> = plan.panes.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["editor", "agent", "terminal"]);
        assert!(plan.panes.iter().all(|p| p.cwd == "/p/demo"));
        // The terminal pane runs nothing: the multiplexer already knows which
        // shell to start, and naming one here would override the user's.
        assert!(plan.panes[2].argv.is_empty());
    }

    #[test]
    fn an_agent_pane_attaches_to_a_live_session_rather_than_starting_a_second() {
        // The property the whole composition rests on. Reopening a template
        // while an agent is working must not start a duplicate beside it.
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[12], 1);
        assert_eq!(plan.panes[1].argv, vec!["apex", "agent", "attach", "12"]);
        assert_eq!(plan.panes[1].title, "agent 12");
    }

    #[test]
    fn an_agent_pane_starts_a_session_when_there_is_none_to_attach_to() {
        // After a reboot the daemon has no sessions, so the same template has
        // to start them instead — which is the "restore cleanly" half.
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[], 1);
        assert_eq!(plan.panes[1].argv, vec!["apex", "agent", "run"]);
    }

    #[test]
    fn review_points_the_third_pane_at_the_diff() {
        let t = template("review").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[], 1);
        assert_eq!(plan.panes[2].argv, vec!["apex", "agent", "diff"]);
    }

    #[test]
    fn several_agents_take_several_live_sessions_in_order() {
        let t = template("agents").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[3, 5], 3);
        assert_eq!(plan.arrangement, "tiled");
        assert_eq!(plan.panes.len(), 3);
        assert_eq!(plan.panes[0].argv, vec!["apex", "agent", "attach", "3"]);
        assert_eq!(plan.panes[1].argv, vec!["apex", "agent", "attach", "5"]);
        // Two live sessions and three panes: the third starts a new one rather
        // than attaching twice to the same agent, which would give two views of
        // one session and look like two agents.
        assert_eq!(plan.panes[2].argv, vec!["apex", "agent", "run"]);
    }

    #[test]
    fn the_agent_count_is_bounded() {
        // Each pane is a real agent with a real context window. "--agents 400"
        // has to be a refusal, not a machine that stops responding.
        let t = template("agents").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[], 400);
        assert_eq!(plan.panes.len(), MAX_AGENT_PANES);
    }

    #[test]
    fn a_template_that_does_not_repeat_ignores_the_agent_count() {
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p"), Some(&ed()), &[], 5);
        assert_eq!(plan.panes.len(), 3);
    }

    #[test]
    fn with_no_editor_the_editor_pane_is_a_shell_rather_than_a_dead_pane() {
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p"), None, &[], 1);
        assert!(plan.panes[0].argv.is_empty());
    }

    #[test]
    fn every_template_is_addressable_by_name_and_nothing_else_is() {
        for t in TEMPLATES {
            assert_eq!(template(t.name).map(|x| x.name), Some(t.name));
        }
        assert!(template("nope").is_none());
    }

    // ── the wire format ─────────────────────────────────────────────────────

    #[test]
    fn a_plan_encodes_one_tab_separated_line_per_pane() {
        let t = template("dev").unwrap();
        let plan = build(t, "demo", Path::new("/p/demo"), Some(&ed()), &[9], 1);
        let text = encode(&plan).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "editor\t/p/demo\tnvim");
        assert_eq!(lines[1], "agent 9\t/p/demo\tapex\tagent\tattach\t9");
        // A pane with no command is title and cwd and nothing more.
        assert_eq!(lines[2], "terminal\t/p/demo");
    }

    #[test]
    fn a_tab_in_a_path_is_refused_rather_than_silently_splitting_a_pane() {
        let plan = Plan {
            session: "apex-x".into(),
            arrangement: "tiled",
            panes: vec![Pane {
                title: "editor".into(),
                cwd: "/p/we\tird".into(),
                argv: vec![],
            }],
        };
        assert!(encode(&plan).is_err());
    }
}
