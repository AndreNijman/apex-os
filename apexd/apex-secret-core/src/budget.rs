//! §13.14: what a project is willing to spend in a day, and what it has spent.
//!
//! ```toml
//! [agent.budget]
//! operations_daily = 200          # every brokered operation, all providers
//! cloudflare_daily = 5.00         # money, and see below about pricing it
//!
//! [agent.budget.operations]       # caps on one operation each
//! "cloudflare.worker.deploy" = 5
//! "cloudflare.workers-ai.run" = 50
//!
//! [agent.budget.price]            # what the owner says an operation costs
//! "cloudflare.workers-ai.run" = 0.011
//! ```
//!
//! §13.14 asks for per-task and per-project budgets and for operation caps —
//! *"deployment count, R2 upload volume, AI spend, and API request count"*. What
//! follows is which of those this build can honestly enforce, and what it does
//! about the ones it cannot.
//!
//! ## Nothing here is Cloudflare's
//!
//! The keys read `cloudflare_daily` because §13.14 writes them that way, but
//! the prefix is just the credential's own name: `github_daily` works, and so
//! does the name of a provider written after this file. A budget is spent by
//! *operations under a stored credential*, and the framework already knows
//! which credential and which operation — so this is [`crate::operation`]'s
//! neighbour rather than the Cloudflare provider's.
//!
//! ## Usage is counted from the audit trail
//!
//! Not from a counter file. The trail is already the record of every brokered
//! operation, it is root-owned, and a separate ledger would be a second record
//! that can disagree with the first — the same argument P1-015's destroy plan
//! is built on. Counting is exact rather than sampled: the daemon reads its own
//! whole trail, so there is no window that can hide an operation.
//!
//! ## Money, and the thing this build cannot do
//!
//! §13.14's example is `cloudflare_daily = 5.00`. **This build cannot price a
//! Cloudflare operation.** Cloudflare's prices are not in the API, they change,
//! they depend on a plan this machine cannot see, and an AI run's cost depends
//! on the model and the tokens. So there are two honest options and one
//! dishonest one:
//!
//! * **refuse the operation** — which is `apex-agentd`'s budget rule, and the
//!   right rule when a limit *might* be enforceable and could not be read;
//! * **let the owner say what things cost** — `[agent.budget.price]`, which
//!   makes the money budget enforceable against numbers somebody chose and can
//!   check;
//! * silently accept the line and enforce nothing. That is the one this
//!   repository has a name for, and it is what a `cloudflare_daily` that reads
//!   as a comment would be.
//!
//! So both of the first two are here. A money budget with a price for every
//! operation that runs is enforced exactly. A money budget that meets an
//! operation it has no price for is [`Spend::Unmeasurable`], and the framework
//! **refuses** — because "I cannot tell what this costs" is not "it is within
//! budget", and the owner who wrote a five dollar cap did not mean *unless it
//! is hard*.
//!
//! An operation priced at `0.00` is a real answer and not a missing one: it
//! says this owner treats a deployment as free, which for a Workers deployment
//! on a paid plan is true.
//!
//! ## What is NOT here: volume
//!
//! §13.14 names *"R2 upload volume"*. [`crate::audit::AuditLine`] records the
//! operation and the resource and not how many bytes went — so a volume cap
//! would have to be enforced against a number nothing measures. It is not
//! offered, and a project that writes `r2_bytes_daily` gets
//! [`BudgetError::Unsupported`] when the file is read, naming the keys that do
//! work. A cap that silently did nothing would be worse than no cap, because
//! somebody would stop watching.

use std::collections::BTreeMap;

use crate::project::{ProjectConfig, ProjectError, MICROS};

/// A day, in milliseconds. Budgets in §13.14 are daily.
pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The keys `[agent.budget]` accepts besides the `<service>_daily` money ones.
const COUNT_KEYS: &[&str] = &["operations_daily"];

/// The sub-tables it accepts.
const SUB_TABLES: &[&str] = &["operations", "price"];

/// The suffix that makes a key a money budget for one stored credential.
const MONEY_SUFFIX: &str = "_daily";

/// Why a budget could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetError {
    /// The file is wrong in a way [`ProjectConfig`] already describes.
    Project(ProjectError),
    /// A key under `[agent.budget]` this build does not enforce.
    ///
    /// Refused rather than ignored. An unknown key in a budget is somebody
    /// capping something, and a cap that quietly does nothing is worse than no
    /// cap because they stop watching it.
    Unsupported { key: String, supported: String },
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetError::Project(e) => write!(f, "{e}"),
            BudgetError::Unsupported { key, supported } => write!(
                f,
                "[agent.budget] sets '{key}', and this build does not enforce \
                 it — so it is refused rather than ignored, because a cap that \
                 does nothing is worse than no cap. What it does enforce: \
                 {supported}"
            ),
        }
    }
}

impl std::error::Error for BudgetError {}

impl From<ProjectError> for BudgetError {
    fn from(e: ProjectError) -> BudgetError {
        BudgetError::Project(e)
    }
}

/// What a project declared it is willing to spend in a day.
///
/// Empty means no budget, which is what every project has until somebody writes
/// one. That is deliberately the permissive default: a budget is a thing an
/// owner opts into, and a machine that refused every operation until a number
/// was written would be a machine nobody finished setting up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Budget {
    /// Every brokered operation this project makes, per day, across providers.
    pub operations_daily: Option<u64>,
    /// A cap on one operation id, per day.
    pub per_operation_daily: BTreeMap<String, u64>,
    /// Money per stored credential per day, in millionths.
    pub money_daily: BTreeMap<String, u64>,
    /// What the owner says one operation costs, in millionths.
    pub price: BTreeMap<String, u64>,
}

impl Budget {
    /// Read `[agent.budget]` out of `<root>/apex.toml`, with "there is no such
    /// file" folded into "there is no budget".
    ///
    /// **Only `Absent` is folded**, and the reason is the one
    /// [`crate::identity::Identities::read_or_unbound`] gives for the same
    /// shape: a project whose `apex.toml` is unreadable, malformed, owned by
    /// somebody else or reached through a symlink comes back as an error,
    /// because proceeding as though it had no budget is how an agent that can
    /// `chmod 000 apex.toml` would remove the cap it is running under. A
    /// permission denial is not an absence, and here the difference is a
    /// budget that stops enforcing.
    ///
    /// Most projects have no `apex.toml` at all, and every one of them must
    /// keep working — folding `Absent` is what makes `git.push` in a plain
    /// repository cost nothing.
    pub fn read_or_unbudgeted(
        root: &std::path::Path,
        owner_uid: u32,
        owner_name: &str,
    ) -> Result<Budget, BudgetError> {
        match ProjectConfig::read(root, owner_uid, owner_name) {
            Ok(config) => Budget::of(&config),
            Err(ProjectError::Absent { .. }) => Ok(Budget::default()),
            Err(other) => Err(BudgetError::Project(other)),
        }
    }

    /// Read `[agent.budget]` out of a project file.
    pub fn of(config: &ProjectConfig) -> Result<Budget, BudgetError> {
        let mut budget = Budget {
            operations_daily: config.count(&["agent", "budget", "operations_daily"])?,
            ..Budget::default()
        };

        // Every remaining scalar under `[agent.budget]` has to be a
        // `<service>_daily` money budget, or it is a key this build does not
        // enforce and says so.
        for key in config.scalar_keys(&["agent", "budget"]) {
            if COUNT_KEYS.contains(&key.as_str()) {
                continue;
            }
            let Some(service) = key.strip_suffix(MONEY_SUFFIX).filter(|s| !s.is_empty()) else {
                return Err(BudgetError::Unsupported {
                    key,
                    supported: supported(),
                });
            };
            let Some(micros) = config.money(&["agent", "budget", &key])? else {
                continue;
            };
            budget.money_daily.insert(service.to_string(), micros);
        }

        for table in config.sections(&["agent", "budget"]) {
            if !SUB_TABLES.contains(&table.as_str()) {
                return Err(BudgetError::Unsupported {
                    key: format!("{table}]"),
                    supported: supported(),
                });
            }
        }
        for operation in config.scalar_keys(&["agent", "budget", "operations"]) {
            let cap = config
                .count(&["agent", "budget", "operations", &operation])?
                .unwrap_or(0);
            budget.per_operation_daily.insert(operation, cap);
        }
        for operation in config.scalar_keys(&["agent", "budget", "price"]) {
            let micros = config
                .money(&["agent", "budget", "price", &operation])?
                .unwrap_or(0);
            budget.price.insert(operation, micros);
        }
        Ok(budget)
    }

    /// Whether the project wrote any budget at all.
    pub fn is_empty(&self) -> bool {
        self.operations_daily.is_none()
            && self.per_operation_daily.is_empty()
            && self.money_daily.is_empty()
    }

    /// Whether a money budget applies to operations under `service`.
    pub fn money_for(&self, service: &str) -> Option<u64> {
        self.money_daily.get(service).copied()
    }

    /// Every operation id this budget names that is not in `known`.
    ///
    /// `[agent.budget.operations]` and `[agent.budget.price]` are keyed by
    /// operation id, and a key is matched against the trail by string equality
    /// — so `"cloudflare.worker.deply" = 0` is a cap on nothing, which is the
    /// exact failure this module's unknown-key rule exists to prevent, arriving
    /// one table lower down. The vocabulary is the registry's and the registry
    /// is the daemon's, so the list is passed in rather than known here:
    /// [`Budget::of`] must stay usable by anything that only wants to read the
    /// file.
    ///
    /// Sorted and deduplicated, so a refusal names the same keys twice running.
    pub fn unknown_operations(&self, known: &[String]) -> Vec<String> {
        let mut out: Vec<String> = self
            .per_operation_daily
            .keys()
            .chain(self.price.keys())
            .filter(|id| !known.iter().any(|k| k == *id))
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

fn supported() -> String {
    format!(
        "`{}`, a `<credential>{MONEY_SUFFIX}` amount of money, \
         [agent.budget.operations] with a count per operation, and \
         [agent.budget.price] with what an operation costs. Volume caps are \
         not here: nothing in this build measures bytes, so `r2_bytes_daily` \
         would be a line that did nothing",
        COUNT_KEYS.join("`, `")
    )
}

/// What this project has already spent today.
///
/// Counted from the audit trail rather than kept in a ledger — see the module
/// note. The fields are what the trail can answer exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// The start of the day these numbers are for, ms since the epoch.
    pub day_start_ms: u64,
    /// Brokered operations that ran and succeeded today, all providers.
    pub operations: u64,
    /// The same, per operation id.
    pub per_operation: BTreeMap<String, u64>,
    /// The same, per stored credential.
    pub per_service: BTreeMap<String, u64>,
    /// The same, per stored credential AND operation id together.
    ///
    /// The two maps above are each a projection of this one, and money is added
    /// up from *this* one rather than from `per_operation`. The difference is
    /// not bookkeeping: a money budget is written against one credential
    /// (`cloudflare_daily`), and the same operation id can run under two of
    /// them. Summing `per_operation` would charge a second account's
    /// deployments to the first, and — worse — would make an unpriced operation
    /// under an unrelated credential render this credential's whole day
    /// [`Spend::Unmeasurable`], which refuses. A `git.push` under `gh` must not
    /// be able to stop a Cloudflare deployment.
    pub per_service_operation: BTreeMap<(String, String), u64>,
}

/// The UTC day `ms` falls in, as the millisecond it started at.
///
/// UTC and not local time, deliberately. A budget that reset at local midnight
/// would reset twice a year in a place that changes its clocks, and the trail
/// this is counted from is timestamped in epoch milliseconds with no zone.
pub fn day_start(ms: u64) -> u64 {
    ms - (ms % DAY_MS)
}

impl Usage {
    /// Count today out of the trail.
    ///
    /// `lines` must be the trail as the daemon holds it — **every** line, not a
    /// window of the last few hundred. A budget counted from a window is a
    /// budget a busy machine can hide an operation from, and undercounting here
    /// lets an operation through rather than refusing one, which is the
    /// direction that matters.
    ///
    /// Only a completed, successful operation counts: a refusal never reached a
    /// provider, and a failure is a request the far side did not act on.
    /// Counting either would make a broken credential exhaust a budget.
    ///
    /// `uid` filters the trail to one account, because the store, the grants
    /// and `apex secret audit` are all per-account and a budget that is not
    /// would let one user's work refuse another's — in the one direction a
    /// budget must never be wrong.
    pub fn of<'a>(
        lines: impl IntoIterator<Item = &'a crate::audit::AuditLine>,
        uid: u32,
        project: &str,
        now_ms: u64,
    ) -> Usage {
        let day_start_ms = day_start(now_ms);
        let mut usage = Usage {
            day_start_ms,
            ..Usage::default()
        };
        for line in lines {
            if line.event != crate::audit::AuditEvent::Used || line.exit_code != Some(0) {
                continue;
            }
            if line.uid != uid || line.ms < day_start_ms {
                continue;
            }
            // An exact match on the root, never a prefix — the rule P1-015's
            // destroy plan established. A worktree lives under the project, so
            // a prefix match would charge every worktree's work to the
            // project's budget and to its own.
            if line.project.as_deref() != Some(project) {
                continue;
            }
            usage.operations += 1;
            *usage.per_operation.entry(line.operation.clone()).or_insert(0) += 1;
            *usage.per_service.entry(line.provider.clone()).or_insert(0) += 1;
            *usage
                .per_service_operation
                .entry((line.provider.clone(), line.operation.clone()))
                .or_insert(0) += 1;
        }
        usage
    }

    /// Record one more operation, as if the trail already held it.
    ///
    /// Used for the in-flight reservations the daemon keeps while a budgeted
    /// operation is being performed — see `apex-secretd`'s service. It is here
    /// rather than there so that a reservation and a trail line increment the
    /// same four fields by the same rule; two places that both know how to add
    /// one to a usage would be two places that can disagree.
    pub fn add(&mut self, service: &str, operation: &str) {
        self.operations += 1;
        *self.per_operation.entry(operation.to_string()).or_insert(0) += 1;
        *self.per_service.entry(service.to_string()).or_insert(0) += 1;
        *self
            .per_service_operation
            .entry((service.to_string(), operation.to_string()))
            .or_insert(0) += 1;
    }

    /// What today's operations cost under `service`, given the owner's prices.
    ///
    /// `Err` names the first operation with no price, which is the whole reason
    /// this is not a sum with a zero default: an unpriced operation contributes
    /// an unknown amount, and adding zero for it would report a spend that is
    /// certainly too low.
    ///
    /// Only what ran under `service` is looked at, priced or not. An unpriced
    /// operation under a *different* credential is not this budget's problem
    /// and must not make this budget unmeasurable — see
    /// [`Usage::per_service_operation`].
    pub fn money_spent(&self, service: &str, price: &BTreeMap<String, u64>) -> Result<u64, String> {
        let mut total = 0u64;
        for ((ran_under, operation), count) in &self.per_service_operation {
            if ran_under != service {
                continue;
            }
            let Some(each) = price.get(operation) else {
                return Err(operation.clone());
            };
            total = total.saturating_add(each.saturating_mul(*count));
        }
        Ok(total)
    }
}

/// What a budget says about one operation that is about to run.
///
/// Three answers, and the third is why this is not a `bool`. A cap that could
/// not be evaluated is not a cap that passed — the same distinction
/// `apex-agentd`'s own budget module is built on, where a controller that could
/// not be read refuses the session rather than starting it unbudgeted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spend {
    /// No cap this operation would cross.
    Within,
    /// A cap it would cross, in words.
    Over(String),
    /// A cap was declared and this build cannot evaluate it. Never a pass.
    Unmeasurable(String),
}

/// [`Spend::Within`]'s word, as it appears in [`crate::audit::AuditLine::spend`].
pub const WITHIN: &str = "within";

/// [`Spend::Over`]'s word.
pub const OVER: &str = "over";

/// [`Spend::Unmeasurable`]'s word.
///
/// A constant because the daemon writes it on refusals that never reach
/// [`check`] — a budget file that could not be read, a cap on an operation no
/// provider implements — and those have to land in the trail under the same
/// word as the ones that do, or a reader grepping for budget refusals would
/// miss exactly the cases where a budget stopped working.
pub const UNMEASURABLE: &str = "unmeasurable";

impl Spend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Spend::Within => WITHIN,
            Spend::Over(_) => OVER,
            Spend::Unmeasurable(_) => UNMEASURABLE,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Spend::Within => None,
            Spend::Over(w) | Spend::Unmeasurable(w) => Some(w),
        }
    }
}

/// Whether one more `operation` under `service` fits inside `budget`.
///
/// The comparison is `used >= limit`, not `used + 1 > limit`, and they are the
/// same thing written two ways — the first cannot overflow. A limit of zero
/// refuses everything, which is what a zero cap means.
pub fn check(budget: &Budget, usage: &Usage, service: &str, operation: &str) -> Spend {
    if let Some(limit) = budget.operations_daily {
        if usage.operations >= limit {
            return Spend::Over(format!(
                "this project has made {} brokered operations today and \
                 [agent.budget] operations_daily is {limit}",
                usage.operations
            ));
        }
    }
    if let Some(limit) = budget.per_operation_daily.get(operation) {
        let used = usage.per_operation.get(operation).copied().unwrap_or(0);
        if used >= *limit {
            return Spend::Over(format!(
                "this project has run {operation} {used} times today and \
                 [agent.budget.operations] caps it at {limit}"
            ));
        }
    }
    let Some(limit) = budget.money_for(service) else {
        return Spend::Within;
    };
    // The operation about to run has to have a price, or the answer to "would
    // this cross the budget" is not known. Refused rather than treated as free.
    let Some(each) = budget.price.get(operation).copied() else {
        return Spend::Unmeasurable(format!(
            "[agent.budget] sets {service}{MONEY_SUFFIX} = {}, and nothing here \
             knows what {operation} costs. Cloudflare's prices are not in its \
             API and depend on a plan this machine cannot see, so this build \
             will not guess: add `\"{operation}\" = 0.00` under \
             [agent.budget.price] if it is free to you, or a real number if it \
             is not",
            money(limit)
        ));
    };
    let spent = match usage.money_spent(service, &budget.price) {
        Ok(spent) => spent,
        Err(unpriced) => {
            return Spend::Unmeasurable(format!(
                "[agent.budget] sets {service}{MONEY_SUFFIX} = {}, and this \
                 project ran {unpriced} today, which has no price under \
                 [agent.budget.price]. What has been spent today therefore \
                 cannot be added up, so whether one more operation fits is not \
                 known",
                money(limit)
            ));
        }
    };
    if spent.saturating_add(each) > limit {
        return Spend::Over(format!(
            "this project has spent {} under '{service}' today, {operation} \
             costs {}, and [agent.budget] {service}{MONEY_SUFFIX} is {}",
            money(spent),
            money(each),
            money(limit)
        ));
    }
    Spend::Within
}

/// One cap, and how much of it is gone.
///
/// Rendered rather than numeric, because a money cap and a count cap are read
/// in different units and a reader that had to know which was which to print
/// `5.00` instead of `5000000` would be a second place that knows about
/// millionths.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cap {
    /// What it caps, as the project file spells it.
    pub name: String,
    /// `operations`, `operation` or `money`.
    pub kind: String,
    /// How much is gone.
    pub used: String,
    /// The cap itself.
    pub limit: String,
    /// Why this cap cannot be evaluated, when it cannot.
    ///
    /// A cap nothing can measure is **not** a cap with nothing used against it,
    /// and a report that showed `0.00 / 5.00` for a money budget with no price
    /// table would be telling somebody their budget was fine when in fact every
    /// operation under it is being refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmeasurable: Option<String>,
}

/// What a project's budget allows and what it has used today.
///
/// §13.14's *"usage visible in the task audit"*, as one answer from the daemon
/// rather than a count a client derives. The client cannot derive it honestly:
/// the trail is root-owned and `Request::Audit` hands back a bounded window, so
/// a client that counted what it was given would present an undercount as a
/// fact.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// The root these numbers are for, exactly as matched.
    pub project: String,
    /// The start of the day, ms since the epoch.
    pub day_start_ms: u64,
    /// Whether the project declared a budget at all. The caps list is empty
    /// either way when nothing is capped, and these are different facts.
    pub budgeted: bool,
    /// Brokered operations today, all credentials — whether or not anything
    /// caps them.
    pub operations: u64,
    /// Today's operations by id, and by credential.
    pub per_operation: BTreeMap<String, u64>,
    pub per_service: BTreeMap<String, u64>,
    /// Every cap the project declared, with what is left of it.
    pub caps: Vec<Cap>,
}

impl Report {
    /// Put a budget and a usage together.
    pub fn new(project: &str, budget: &Budget, usage: &Usage) -> Report {
        let mut caps = Vec::new();
        if let Some(limit) = budget.operations_daily {
            caps.push(Cap {
                name: "operations_daily".to_string(),
                kind: "operations".to_string(),
                used: usage.operations.to_string(),
                limit: limit.to_string(),
                unmeasurable: None,
            });
        }
        for (operation, limit) in &budget.per_operation_daily {
            caps.push(Cap {
                name: operation.clone(),
                kind: "operation".to_string(),
                used: usage
                    .per_operation
                    .get(operation)
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
                limit: limit.to_string(),
                unmeasurable: None,
            });
        }
        for (service, limit) in &budget.money_daily {
            let (used, unmeasurable) = match usage.money_spent(service, &budget.price) {
                Ok(spent) => (money(spent), None),
                Err(unpriced) => (
                    "?".to_string(),
                    Some(format!(
                        "{unpriced} ran today and has no price under \
                         [agent.budget.price], so what has been spent cannot be \
                         added up. Every operation under '{service}' is being \
                         refused until it has one"
                    )),
                ),
            };
            caps.push(Cap {
                name: format!("{service}{MONEY_SUFFIX}"),
                kind: "money".to_string(),
                used,
                limit: money(*limit),
                unmeasurable,
            });
        }
        Report {
            project: project.to_string(),
            day_start_ms: usage.day_start_ms,
            budgeted: !budget.is_empty(),
            operations: usage.operations,
            per_operation: usage.per_operation.clone(),
            per_service: usage.per_service.clone(),
            caps,
        }
    }
}

/// Millionths, as a decimal somebody reads.
///
/// Trailing zeros kept to two places, because money is written that way and
/// `5` next to `5.00` in the same message reads as two different things.
pub fn money(micros: u64) -> String {
    let whole = micros / MICROS;
    let frac = micros % MICROS;
    if frac % 10_000 == 0 {
        format!("{whole}.{:02}", frac / 10_000)
    } else {
        format!("{whole}.{frac:06}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEvent, AuditLine};
    use crate::capability::CapabilityRecord;
    use std::path::Path;

    fn config(text: &str) -> ProjectConfig {
        ProjectConfig::parse(Path::new("/p/apex.toml"), text).expect("parses")
    }

    fn line(
        provider: &str,
        operation: &str,
        project: &str,
        exit_code: Option<i32>,
        event: AuditEvent,
        ms: u64,
    ) -> AuditLine {
        let mut record = CapabilityRecord::new(provider, operation, "thing");
        record.project = Some(project.to_string());
        AuditLine {
            ms,
            exit_code,
            ..AuditLine::from_record("a1", event, 1000, 1, &record)
        }
    }

    fn used(operation: &str, ms: u64) -> AuditLine {
        line("cloudflare", operation, "/p", Some(0), AuditEvent::Used, ms)
    }

    #[test]
    fn a_project_with_no_budget_has_no_budget() {
        // The permissive default, stated as a test because the alternative —
        // refusing until somebody writes a number — is a machine nobody
        // finishes setting up.
        let budget = Budget::of(&config("")).expect("no budget is a budget");
        assert!(budget.is_empty());
        assert_eq!(
            check(&budget, &Usage::default(), "cloudflare", "cloudflare.worker.deploy"),
            Spend::Within
        );
    }

    #[test]
    fn a_cap_this_build_cannot_enforce_is_refused_when_the_file_is_read() {
        // §13.14 names "R2 upload volume", and nothing here measures bytes. A
        // project that writes such a cap must be told, at the moment the file
        // is read, rather than discovering months later that the line did
        // nothing.
        let err = Budget::of(&config(
            "[agent.budget]\nr2_bytes = 1000\n",
        ))
        .expect_err("an unenforceable cap is refused");
        let message = err.to_string();
        assert!(message.contains("r2_bytes"), "{message}");
        assert!(message.contains("operations_daily"), "{message}");

        // And an unknown sub-table, which is the other shape it takes.
        let err = Budget::of(&config(
            "[agent.budget.volume]\n\"cloudflare.r2.object.write\" = 1000\n",
        ))
        .expect_err("an unenforceable table is refused");
        assert!(err.to_string().contains("volume"), "{err}");
    }

    #[test]
    fn the_roadmaps_own_example_parses() {
        // §13.14's block, verbatim. If this stops parsing, the roadmap and the
        // build disagree about what an owner is supposed to write.
        let budget = Budget::of(&config(
            "[agent.budget]\ncloudflare_daily = 5.00\nworkers_ai_daily = 3.00\n",
        ))
        .expect("§13.14's example");
        assert_eq!(budget.money_for("cloudflare"), Some(5 * MICROS));
        assert_eq!(budget.money_for("workers_ai"), Some(3 * MICROS));
        assert!(!budget.is_empty());
    }

    #[test]
    fn money_is_kept_in_millionths_and_never_as_a_float() {
        // A thousand additions of 0.011 in f64 is not 11.0, and a budget that
        // drifts refuses one operation early or lets one too many through.
        let budget = Budget::of(&config(
            "[agent.budget]\ncloudflare_daily = 11.00\n\n\
             [agent.budget.price]\n\"cloudflare.workers-ai.run\" = 0.011\n",
        ))
        .expect("prices");
        assert_eq!(budget.price["cloudflare.workers-ai.run"], 11_000);

        let mut usage = Usage::default();
        for _ in 0..1000 {
            usage.add("cloudflare", "cloudflare.workers-ai.run");
        }
        assert_eq!(
            usage.money_spent("cloudflare", &budget.price),
            Ok(11 * MICROS),
            "a thousand runs at 0.011 is exactly 11.00"
        );
        // And the next one crosses, exactly rather than approximately.
        assert!(matches!(
            check(&budget, &usage, "cloudflare", "cloudflare.workers-ai.run"),
            Spend::Over(_)
        ));
    }

    #[test]
    fn a_money_budget_with_nothing_to_price_it_by_refuses_rather_than_passing() {
        // The whole argument of this module. `cloudflare_daily = 5.00` with no
        // price table cannot be evaluated, and "I cannot tell" is not "within
        // budget" — the owner who wrote a five dollar cap did not mean *unless
        // it is hard*.
        let budget =
            Budget::of(&config("[agent.budget]\ncloudflare_daily = 5.00\n")).expect("parses");
        let spend = check(
            &budget,
            &Usage::default(),
            "cloudflare",
            "cloudflare.worker.deploy",
        );
        assert_eq!(spend.as_str(), "unmeasurable", "{spend:?}");
        let why = spend.reason().expect("a reason");
        assert!(why.contains("[agent.budget.price]"), "{why}");
        assert!(why.contains("cloudflare.worker.deploy"), "{why}");

        // A price of zero is an ANSWER, not a missing one: this owner treats a
        // deployment as free, which on a paid Workers plan it is.
        let priced = Budget::of(&config(
            "[agent.budget]\ncloudflare_daily = 5.00\n\n\
             [agent.budget.price]\n\"cloudflare.worker.deploy\" = 0.00\n",
        ))
        .expect("parses");
        assert_eq!(
            check(&priced, &Usage::default(), "cloudflare", "cloudflare.worker.deploy"),
            Spend::Within
        );
    }

    #[test]
    fn a_spend_that_cannot_be_added_up_is_not_a_spend_within_budget() {
        // The second half of the same rule, and the one that is easy to get
        // wrong: the operation about to run is priced, but something that ALSO
        // ran today is not — so what has been spent is unknown, and whether
        // one more fits is unknown with it. Treating the unpriced one as free
        // would report a total that is certainly too low.
        let budget = Budget::of(&config(
            "[agent.budget]\ncloudflare_daily = 5.00\n\n\
             [agent.budget.price]\n\"cloudflare.worker.deploy\" = 0.00\n",
        ))
        .expect("parses");
        let mut usage = Usage::default();
        usage.add("cloudflare", "cloudflare.worker.deploy");
        usage.add("cloudflare", "cloudflare.workers-ai.run");

        assert_eq!(
            usage.money_spent("cloudflare", &budget.price),
            Err("cloudflare.workers-ai.run".to_string())
        );
        let spend = check(&budget, &usage, "cloudflare", "cloudflare.worker.deploy");
        assert_eq!(spend.as_str(), "unmeasurable", "{spend:?}");
        assert!(spend.reason().expect("why").contains("workers-ai"), "{spend:?}");
    }

    #[test]
    fn an_operation_under_another_credential_is_not_this_budgets_business() {
        // A money budget names ONE stored credential. What ran under a
        // different one is neither charged to it nor allowed to make it
        // unmeasurable — and the second half is the dangerous one, because
        // `unmeasurable` refuses: a `git.push` under `gh` that nobody priced
        // must not be able to stop a Cloudflare deployment for the rest of the
        // day. Summing the per-operation totals, which do not say which
        // credential ran them, does exactly that.
        let budget = Budget::of(&config(
            "[agent.budget]\ncloudflare_daily = 5.00\n\n\
             [agent.budget.price]\n\"cloudflare.worker.deploy\" = 1.00\n",
        ))
        .expect("parses");

        let mut usage = Usage::default();
        usage.add("cloudflare", "cloudflare.worker.deploy");
        // Under a different credential, and deliberately unpriced.
        usage.add("gh", "git.push");
        usage.add("gh", "git.push");

        assert_eq!(
            usage.money_spent("cloudflare", &budget.price),
            Ok(MICROS),
            "one priced deployment, and two pushes that are not this budget's"
        );
        assert_eq!(
            check(&budget, &usage, "cloudflare", "cloudflare.worker.deploy"),
            Spend::Within
        );

        // And the same operation id run under a SECOND Cloudflare credential is
        // that credential's spend, not this one's. Two accounts, one price
        // table, two separate five-dollar budgets.
        usage.add("cf-other", "cloudflare.worker.deploy");
        assert_eq!(usage.money_spent("cloudflare", &budget.price), Ok(MICROS));
        assert_eq!(usage.money_spent("cf-other", &budget.price), Ok(MICROS));
    }

    #[test]
    fn only_a_completed_successful_operation_counts_against_a_budget() {
        // A refusal never reached a provider and a failure is a request the far
        // side did not act on. Counting either would let a broken credential
        // exhaust a day's budget without spending anything.
        let now = DAY_MS * 20_000 + 5;
        let today = day_start(now);
        let trail = vec![
            used("cloudflare.worker.deploy", today + 1),
            line("cloudflare", "cloudflare.worker.deploy", "/p", None, AuditEvent::Refused, today + 2),
            line("cloudflare", "cloudflare.worker.deploy", "/p", Some(1), AuditEvent::Used, today + 3),
            // Yesterday, and another project.
            used("cloudflare.worker.deploy", today - 1),
            line("cloudflare", "cloudflare.worker.deploy", "/other", Some(0), AuditEvent::Used, today + 4),
        ];
        let usage = Usage::of(&trail, 1000, "/p", now);
        assert_eq!(usage.operations, 1, "{usage:#?}");
        assert_eq!(usage.day_start_ms, today);
        assert_eq!(usage.per_operation["cloudflare.worker.deploy"], 1);
        assert_eq!(usage.per_service["cloudflare"], 1);
        assert_eq!(
            usage.per_service_operation
                [&("cloudflare".to_string(), "cloudflare.worker.deploy".to_string())],
            1
        );
    }

    #[test]
    fn another_accounts_work_is_not_counted_against_this_ones_budget() {
        // The store is per-account, the grants are per-account, and `apex
        // secret audit` shows an account its own lines. A budget that summed
        // the machine would let one user exhaust another's cap in the same
        // shared checkout — refusing work that was never theirs, which is the
        // one direction a budget must not be wrong in.
        let now = DAY_MS * 20_000 + 5;
        let today = day_start(now);
        let mut theirs = used("cloudflare.worker.deploy", today + 1);
        theirs.uid = 1001;
        let trail = vec![used("cloudflare.worker.deploy", today + 2), theirs];

        assert_eq!(Usage::of(&trail, 1000, "/p", now).operations, 1);
        assert_eq!(Usage::of(&trail, 1001, "/p", now).operations, 1);
        assert_eq!(Usage::of(&trail, 1002, "/p", now).operations, 0);
    }

    #[test]
    fn a_worktree_under_a_project_is_not_the_project() {
        // Exact match, never a prefix — P1-015's rule, which a budget needs for
        // its own reason: a worktree at `<project>/.apex/worktrees/x` doing its
        // own deployments must not spend the parent's cap as well as its own,
        // and a project's cap must not be exhausted by work nobody did in it.
        let now = DAY_MS * 20_000 + 5;
        let today = day_start(now);
        let mut inner = used("cloudflare.worker.deploy", today + 1);
        inner.project = Some("/p/.apex/worktrees/x".to_string());
        let trail = vec![used("cloudflare.worker.deploy", today + 2), inner];

        assert_eq!(Usage::of(&trail, 1000, "/p", now).operations, 1);
        assert_eq!(
            Usage::of(&trail, 1000, "/p/.apex/worktrees/x", now).operations,
            1
        );
    }

    #[test]
    fn a_day_starts_at_utc_midnight_and_yesterday_is_not_today() {
        // Local time would reset twice a year where clocks change, and the
        // trail this is counted from carries epoch milliseconds with no zone.
        let midnight = day_start(1_757_635_200_123);
        assert_eq!(midnight, 1_757_635_200_000);
        assert_eq!(day_start(midnight), midnight, "midnight is its own day start");
        assert_eq!(day_start(midnight + DAY_MS - 1), midnight);
        assert_eq!(day_start(midnight + DAY_MS), midnight + DAY_MS);
    }

    #[test]
    fn a_cap_of_zero_refuses_and_a_cap_at_the_limit_refuses_the_next_one() {
        let budget = Budget::of(&config(
            "[agent.budget]\noperations_daily = 2\n\n\
             [agent.budget.operations]\n\"cloudflare.worker.deploy\" = 0\n",
        ))
        .expect("parses");
        // Zero means zero, not "unset".
        assert!(matches!(
            check(&budget, &Usage::default(), "cloudflare", "cloudflare.worker.deploy"),
            Spend::Over(_)
        ));
        // The overall cap bites at the limit rather than one past it.
        let mut usage = Usage {
            operations: 1,
            ..Usage::default()
        };
        assert_eq!(
            check(&budget, &usage, "cloudflare", "cloudflare.worker.read"),
            Spend::Within
        );
        usage.operations = 2;
        assert!(matches!(
            check(&budget, &usage, "cloudflare", "cloudflare.worker.read"),
            Spend::Over(_)
        ));
    }

    #[test]
    fn a_cap_on_an_operation_that_does_not_exist_is_a_cap_on_nothing() {
        // The unknown-key rule, one table lower. `[agent.budget]` refuses a key
        // it does not enforce; the two sub-tables are keyed by operation id and
        // matched against the trail by string equality, so a typo there is a
        // cap that silently never bites — which is the same defect wearing a
        // different hat. The vocabulary is the daemon's, so this reports rather
        // than refuses, and the daemon turns it into a refusal.
        let budget = Budget::of(&config(
            "[agent.budget.operations]\n\
             \"cloudflare.worker.deply\" = 5\n\
             \"cloudflare.worker.deploy\" = 5\n\n\
             [agent.budget.price]\n\
             \"cloudflare.worker.deploy\" = 0.00\n\
             \"clouflare.workers-ai.run\" = 0.01\n",
        ))
        .expect("parses");
        let known = vec![
            "cloudflare.worker.deploy".to_string(),
            "cloudflare.workers-ai.run".to_string(),
        ];
        assert_eq!(
            budget.unknown_operations(&known),
            vec![
                "cloudflare.worker.deply".to_string(),
                "clouflare.workers-ai.run".to_string()
            ]
        );
        // And a budget that names only real operations reports none, which is
        // the arm that stops this from refusing every project.
        let fine = Budget::of(&config(
            "[agent.budget.operations]\n\"cloudflare.worker.deploy\" = 5\n",
        ))
        .expect("parses");
        assert!(fine.unknown_operations(&known).is_empty());
    }

    #[test]
    fn an_amount_of_money_renders_as_money() {
        // The plan and the refusal both quote these back at somebody who wrote
        // `5.00` in a file, and `5000000` is not a number they would recognise.
        assert_eq!(money(5 * MICROS), "5.00");
        assert_eq!(money(0), "0.00");
        assert_eq!(money(11_000), "0.011000");
        assert_eq!(money(1_250_000), "1.25");
    }

    #[test]
    fn the_three_spend_answers_stay_distinct() {
        // Collapsing `unmeasurable` into either of the others is the defect
        // this type exists to prevent: into `within` and the budget does
        // nothing, into `over` and a project with a price table it has not
        // finished writing can do nothing at all. Both are wrong; only one is
        // dangerous, which is why the arm exists rather than defaulting.
        let all = [
            Spend::Within,
            Spend::Over("x".into()),
            Spend::Unmeasurable("y".into()),
        ];
        let mut seen: Vec<&str> = all.iter().map(Spend::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "two arms render the same");
        assert_eq!(Spend::Within.reason(), None);
    }
}
