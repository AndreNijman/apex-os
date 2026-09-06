//! Which destinations an allowlisted session may reach.
//!
//! Dimension 5's `allowlist` mode is two decisions, and this module is both of
//! them, with no I/O in either:
//!
//! 1. may this session open a connection to *this name and port*;
//! 2. now that the name has resolved, is *this address* one the rule that
//!    matched was talking about.
//!
//! Keeping them here, as functions from values to a [`Verdict`], is what makes
//! the destination policy testable without a network — the same reason
//! `sandbox::build_argv` is a pure function of its spec. A test asserts the
//! whole decision table; `apex-agentd`'s `egress` module does the resolving and
//! the connecting and asks these two questions in between.
//!
//! ## Default deny, and a rule is a name plus a port
//!
//! An empty allowlist denies everything, and a name nothing matched is denied
//! rather than reaching a fallback. A rule is written `host` or `host:port`,
//! one per line, and a rule with no port means 443 — the only port an agent
//! reaches for without being told to. Another port is another rule, because
//! `api.example.com:*` is a rule that looks narrow and is not: the same name
//! usually answers on 22 as well.
//!
//! `*.example.com` matches `a.example.com` and `a.b.example.com` and NOT
//! `example.com` itself. That is the fail-closed reading — a wildcard that
//! silently included the apex would grant the one name most likely to be the
//! interesting one — and listing both is one extra line. A wildcard needs at
//! least two labels after it, so `*.com` and `*` are refused outright: they are
//! not an allowlist, they are an open network with extra steps.
//!
//! ## Why the resolved address is checked as well as the name
//!
//! The daemon that opens the connection is in the *host's* network namespace.
//! Everything the user's machine can reach, it can reach: a development server
//! on `127.0.0.1`, a printer, a router's admin page, a cloud metadata endpoint
//! on `169.254.169.254`. A name-based allowlist says nothing about any of that,
//! because whoever controls the DNS for an allowed name controls what it
//! resolves to.
//!
//! So [`Allowlist::accepts_address`] refuses loopback, private, link-local,
//! unique-local, carrier-NAT, multicast and unspecified addresses unless the
//! rule that matched was an IP literal naming that exact address. Reaching a
//! machine on the LAN is then something the user writes down as an address,
//! which is also the only form in which it can be written down honestly.
//!
//! ## What this does not decide
//!
//! Anything about the contents of the connection. Once a destination is
//! allowed, everything the session sends to it is allowed, in both directions
//! and in any volume. This is a destination policy, not a data-loss one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The port a rule means when it does not say.
pub const DEFAULT_PORT: u16 = 443;

/// The longest a hostname may be, from the DNS wire format.
const MAX_HOST_LEN: usize = 253;

/// A destination a session asked to reach.
///
/// The host is stored normalised — lowercased, with one trailing dot removed —
/// so `API.Example.COM.` and `api.example.com` cannot be two different
/// destinations, one of which a rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    host: String,
    port: u16,
}

impl Destination {
    /// Parse a `host:port` authority, as a CONNECT request carries it.
    ///
    /// `port` is required: the proxy protocol always supplies one, and
    /// inventing a default here would mean a request that omitted it got
    /// checked against a port it was not going to use.
    pub fn parse(authority: &str) -> Result<Destination, DestinationError> {
        let (host, port) = split_authority(authority)
            .ok_or_else(|| DestinationError::Malformed(authority.to_string()))?;
        let port: u16 = port
            .parse()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| DestinationError::BadPort(port.to_string()))?;
        Destination::new(host, port)
    }

    /// A destination from an already-separated host and port.
    pub fn new(host: &str, port: u16) -> Result<Destination, DestinationError> {
        let host = normalise_host(host)?;
        Ok(Destination { host, port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address this destination names, when it names one directly rather
    /// than through the DNS.
    pub fn literal_address(&self) -> Option<IpAddr> {
        parse_ip(&self.host)
    }
}

impl std::fmt::Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// A destination that could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationError {
    /// Not a `host:port` at all.
    Malformed(String),
    /// The port is missing, zero or not a number.
    BadPort(String),
    /// The host is empty, over-long, or carries something a hostname may not.
    BadHost(String),
}

impl std::fmt::Display for DestinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DestinationError::Malformed(s) => {
                write!(f, "'{}' is not a host:port destination", s.escape_debug())
            }
            DestinationError::BadPort(s) => {
                write!(f, "'{}' is not a port number", s.escape_debug())
            }
            DestinationError::BadHost(s) => write!(
                f,
                "'{}' is not a hostname APEX will resolve; names must be ASCII, so an \
                 internationalised one has to be written in its punycode form",
                s.escape_debug()
            ),
        }
    }
}

impl std::error::Error for DestinationError {}

/// Which names one rule covers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostPattern {
    /// One name, exactly.
    Exact(String),
    /// Every name under this one, but not this one. Stored without the `*.`.
    Subdomains(String),
    /// One address, written down as an address.
    Address(IpAddr),
}

/// One line of the allowlist: which names, and which port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    host: HostPattern,
    port: u16,
}

impl Rule {
    /// Parse one line.
    pub fn parse(text: &str) -> Result<Rule, RuleError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RuleError::Empty);
        }
        if text.contains("://") {
            return Err(RuleError::HasScheme(text.to_string()));
        }
        if text.contains('/') {
            return Err(RuleError::HasPath(text.to_string()));
        }

        let (host, port) = match split_authority(text) {
            Some((host, port)) => {
                let port: u16 = port
                    .parse()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(|| RuleError::BadPort(port.to_string()))?;
                (host.to_string(), port)
            }
            None => (text.to_string(), DEFAULT_PORT),
        };

        let host = if let Some(rest) = host.strip_prefix("*.") {
            // `*.` on its own has nothing behind it at all, which is the same
            // complaint as `*.com` and deserves the same answer.
            if rest.is_empty() {
                return Err(RuleError::TooBroad(text.to_string()));
            }
            let name = normalise_host(rest).map_err(|_| RuleError::BadHost(host.clone()))?;
            // Two labels minimum. `*.com` is not a narrower policy than no
            // policy, it just reads like one.
            if name.split('.').filter(|l| !l.is_empty()).count() < 2 {
                return Err(RuleError::TooBroad(text.to_string()));
            }
            if name.contains('*') {
                return Err(RuleError::BadWildcard(text.to_string()));
            }
            HostPattern::Subdomains(name)
        } else if host.contains('*') {
            // `a*.example.com`, `*example.com`, a bare `*`. All of them are a
            // wildcard this refuses to guess at.
            return Err(RuleError::BadWildcard(text.to_string()));
        } else {
            let name = normalise_host(&host).map_err(|_| RuleError::BadHost(host.clone()))?;
            match parse_ip(&name) {
                Some(addr) => HostPattern::Address(addr),
                None => HostPattern::Exact(name),
            }
        };

        Ok(Rule { host, port })
    }

    /// Whether this rule covers `dest`'s name, ignoring the port.
    fn covers_host(&self, dest: &Destination) -> bool {
        match &self.host {
            HostPattern::Exact(name) => name == &dest.host,
            HostPattern::Subdomains(suffix) => dest
                .host
                .strip_suffix(suffix.as_str())
                .and_then(|head| head.strip_suffix('.'))
                .is_some_and(|head| !head.is_empty()),
            HostPattern::Address(addr) => dest.literal_address() == Some(*addr),
        }
    }

    /// The line that would produce this rule, for `apex agent status` and for
    /// the refusal a session is shown.
    pub fn as_line(&self) -> String {
        let host = match &self.host {
            HostPattern::Exact(name) => name.clone(),
            HostPattern::Subdomains(name) => format!("*.{name}"),
            HostPattern::Address(addr) => addr.to_string(),
        };
        if self.port == DEFAULT_PORT {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(&self.as_line())
    }
}

/// A line that is not a rule.
///
/// Each names what was wrong rather than reporting the line as invalid,
/// because the whole allowlist is dropped when one line does not parse and the
/// user has to be able to find the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleError {
    Empty,
    HasScheme(String),
    HasPath(String),
    BadPort(String),
    BadHost(String),
    /// A wildcard that is not a leading `*.`.
    BadWildcard(String),
    /// A wildcard with fewer than two labels behind it.
    TooBroad(String),
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::Empty => write!(f, "an allowed destination cannot be blank"),
            RuleError::HasScheme(s) => write!(
                f,
                "'{}' is a URL; an allowed destination is a host and a port, because the \
                 allowlist is checked before there is a request to have a scheme",
                s.escape_debug()
            ),
            RuleError::HasPath(s) => write!(
                f,
                "'{}' has a path; the allowlist decides which host a session may open a \
                 connection to, and a tunnelled connection has no path to check",
                s.escape_debug()
            ),
            RuleError::BadPort(s) => write!(
                f,
                "'{}' is not a port number; write one rule per port, since a name that \
                 answers on 443 usually answers on 22 as well",
                s.escape_debug()
            ),
            RuleError::BadHost(s) => write!(
                f,
                "'{}' is not a hostname; names must be ASCII, so an internationalised one \
                 has to be written in its punycode form",
                s.escape_debug()
            ),
            RuleError::BadWildcard(s) => write!(
                f,
                "'{}' is not a wildcard this understands; the only form is a leading `*.`, \
                 as in `*.example.com`",
                s.escape_debug()
            ),
            RuleError::TooBroad(s) => write!(
                f,
                "'{}' would allow a whole suffix of the internet; a wildcard needs at least \
                 two labels after it, as in `*.example.com`",
                s.escape_debug()
            ),
        }
    }
}

impl std::error::Error for RuleError {}

/// Why a destination was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    /// The allowlist has no rule for this name.
    NoRule,
    /// A rule names this host, on other ports.
    WrongPort { allowed: Vec<u16> },
    /// The name resolved into a range the matching rule did not name.
    LocalAddress(IpAddr),
}

/// The answer to one destination question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(Denial),
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Verdict::Allow)
    }

    /// The line a refused session is shown.
    ///
    /// Says what was refused and what would have to change, because the
    /// session cannot read the allowlist — it lives in the daemon's
    /// configuration, outside the sandbox — and an agent told only "denied"
    /// will retry it forever.
    pub fn explain(&self, dest: &Destination) -> String {
        match self {
            Verdict::Allow => format!("{dest} is allowed"),
            Verdict::Deny(Denial::NoRule) => format!(
                "{dest} is not on this session's network allowlist; add it with \
                 `apex agent allow {}` if it should be",
                dest.host()
            ),
            Verdict::Deny(Denial::WrongPort { allowed }) => format!(
                "{} is allowed on {}, not on {}; each port is its own rule",
                dest.host(),
                allowed
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                dest.port()
            ),
            Verdict::Deny(Denial::LocalAddress(addr)) => format!(
                "{dest} resolved to {addr}, which is on this machine or its network; a \
                 name on the allowlist does not carry permission to reach the LAN, so \
                 allow the address itself if that is what you meant"
            ),
        }
    }
}

/// The destinations an `allowlist` session may reach.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowlist {
    rules: Vec<Rule>,
}

impl Allowlist {
    /// Parse every line, or refuse the whole list.
    ///
    /// All or nothing on purpose. Dropping the lines that did not parse would
    /// tighten the policy, which is the safe direction, but it would do it
    /// silently in the one file where a typo means a session cannot reach the
    /// thing it was started to reach — and the failure would land minutes
    /// later, inside the agent, as a network error.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Allowlist, RuleError> {
        let mut rules = Vec::new();
        for line in lines {
            let rule = Rule::parse(line.as_ref())?;
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
        Ok(Allowlist { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// The rules, as the lines that would produce them.
    pub fn lines(&self) -> Vec<String> {
        self.rules.iter().map(Rule::as_line).collect()
    }

    /// Question one: may a connection to this name and port be opened at all.
    ///
    /// Default deny. An empty allowlist refuses everything, which is what an
    /// allowlist nobody has filled in should do.
    pub fn decide(&self, dest: &Destination) -> Verdict {
        if self.rules.iter().any(|r| r.covers_host(dest) && r.port == dest.port) {
            return Verdict::Allow;
        }
        let mut allowed: Vec<u16> = self
            .rules
            .iter()
            .filter(|r| r.covers_host(dest))
            .map(|r| r.port)
            .collect();
        if allowed.is_empty() {
            return Verdict::Deny(Denial::NoRule);
        }
        allowed.sort_unstable();
        allowed.dedup();
        Verdict::Deny(Denial::WrongPort { allowed })
    }

    /// Question two: having resolved the name, is this address one the rule
    /// that matched was talking about.
    ///
    /// Called for every candidate address before the daemon connects to it,
    /// and the daemon connects to the address it checked rather than to the
    /// name again — resolving twice is how a check like this gets defeated by
    /// a second answer.
    pub fn accepts_address(&self, dest: &Destination, addr: IpAddr) -> Verdict {
        let decision = self.decide(dest);
        if !decision.is_allowed() {
            return decision;
        }
        let addr = unmap(addr);
        if !is_local_range(addr) {
            return Verdict::Allow;
        }
        // A rule that wrote the address down means it. A rule that wrote a
        // name down did not: whoever controls the name controls where it
        // points, and this machine's own network is not what "allow
        // api.example.com" was asking for.
        let named = self.rules.iter().any(|r| {
            r.port == dest.port && matches!(r.host, HostPattern::Address(a) if unmap(a) == addr)
        });
        if named {
            Verdict::Allow
        } else {
            Verdict::Deny(Denial::LocalAddress(addr))
        }
    }
}

/// Split `host:port`, handling the bracketed IPv6 form.
///
/// Returns `None` when there is no port, which is a rule without one rather
/// than an error — and for a bare IPv6 literal, which has colons everywhere
/// and must be written in brackets when it carries a port.
fn split_authority(text: &str) -> Option<(&str, &str)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?;
        return Some((host, port));
    }
    let (host, port) = text.rsplit_once(':')?;
    // More than one colon and no brackets: an unbracketed IPv6 literal.
    if host.contains(':') {
        return None;
    }
    Some((host, port))
}

/// Lowercase, drop one trailing dot, and refuse anything that is not a name.
///
/// ASCII only. A name with non-ASCII in it has a punycode form, and guessing
/// which one — the DNS wants the encoded form, a comparison against a rule
/// wants whichever the rule used — is how a rule and a destination end up
/// looking different while naming the same host.
fn normalise_host(host: &str) -> Result<String, DestinationError> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return Err(DestinationError::BadHost(host.to_string()));
    }
    let ok = host.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '*')
    });
    if !ok || host.starts_with('.') || host.contains("..") {
        return Err(DestinationError::BadHost(host.to_string()));
    }
    // A colon is only ever legitimate in an IPv6 literal.
    if host.contains(':') && parse_ip(host).is_none() {
        return Err(DestinationError::BadHost(host.to_string()));
    }
    Ok(host.to_ascii_lowercase())
}

fn parse_ip(host: &str) -> Option<IpAddr> {
    host.parse::<IpAddr>().ok()
}

/// An IPv4-mapped IPv6 address is the IPv4 address.
///
/// Without this, `::ffff:127.0.0.1` walks straight past every check below —
/// it is not `is_loopback` as an `Ipv6Addr`, and the kernel connects it to
/// 127.0.0.1 anyway.
fn unmap(addr: IpAddr) -> IpAddr {
    match addr {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Whether an address is on this machine or the network it sits on.
///
/// Written out rather than assembled from the standard library's predicates
/// because half of the ones needed — unique-local, link-local unicast,
/// carrier-grade NAT — are still unstable, and a guard that covered only the
/// stable half would be a guard with holes in the ranges most worth guarding.
fn is_local_range(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_local_v4(v4),
        IpAddr::V6(v6) => is_local_v6(v6),
    }
}

fn is_local_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || v4.is_multicast()
        // 100.64.0.0/10, carrier-grade NAT: not the public internet either.
        || (a == 100 && (64..128).contains(&b))
}

fn is_local_v6(v6: Ipv6Addr) -> bool {
    let first = v6.segments()[0];
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // fc00::/7, unique local.
        || (first & 0xfe00) == 0xfc00
        // fe80::/10, link-local unicast.
        || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(lines: &[&str]) -> Allowlist {
        Allowlist::parse(lines).expect("parse")
    }

    fn dest(text: &str) -> Destination {
        Destination::parse(text).expect("parse")
    }

    #[test]
    fn an_empty_allowlist_denies_everything() {
        // The property that makes this an allowlist rather than a suggestion.
        let empty = Allowlist::default();
        assert!(empty.is_empty());
        for target in ["api.example.com:443", "127.0.0.1:8080", "[2001:db8::1]:443"] {
            assert_eq!(
                empty.decide(&dest(target)),
                Verdict::Deny(Denial::NoRule),
                "{target}"
            );
        }
    }

    #[test]
    fn an_exact_rule_matches_that_name_and_no_neighbour_of_it() {
        let a = list(&["api.example.com"]);
        assert!(a.decide(&dest("api.example.com:443")).is_allowed());
        for other in [
            "example.com:443",
            "api.example.com.evil.test:443",
            "notapi.example.com:443",
            "api.example.co:443",
            "xapi.example.com:443",
        ] {
            assert_eq!(
                a.decide(&dest(other)),
                Verdict::Deny(Denial::NoRule),
                "{other} matched an exact rule for api.example.com"
            );
        }
    }

    #[test]
    fn a_wildcard_covers_subdomains_and_not_the_name_itself() {
        // The fail-closed reading. A wildcard that silently included the apex
        // would grant the one name most likely to be the interesting one, and
        // listing both is one extra line.
        let a = list(&["*.example.com"]);
        for under in ["a.example.com:443", "a.b.example.com:443"] {
            assert!(a.decide(&dest(under)).is_allowed(), "{under}");
        }
        for outside in [
            "example.com:443",
            "notexample.com:443",
            "example.com.evil.test:443",
            // The suffix has to be preceded by a dot, or `myexample.com` would
            // match `*.example.com` on a plain string suffix test.
            "myexample.com:443",
        ] {
            assert_eq!(
                a.decide(&dest(outside)),
                Verdict::Deny(Denial::NoRule),
                "{outside} matched *.example.com"
            );
        }

        // Both, when both are wanted.
        let a = list(&["example.com", "*.example.com"]);
        assert!(a.decide(&dest("example.com:443")).is_allowed());
        assert!(a.decide(&dest("a.example.com:443")).is_allowed());
    }

    #[test]
    fn a_rule_without_a_port_means_443_and_nothing_else() {
        let a = list(&["api.example.com"]);
        assert!(a.decide(&dest("api.example.com:443")).is_allowed());
        for port in [22u16, 80, 8080, 3128] {
            let d = Destination::new("api.example.com", port).unwrap();
            assert_eq!(
                a.decide(&d),
                Verdict::Deny(Denial::WrongPort { allowed: vec![443] }),
                "port {port} came through a rule that named no port"
            );
        }
    }

    #[test]
    fn each_port_is_its_own_rule_and_the_refusal_says_which_ones_exist() {
        let a = list(&["git.example.com:443", "git.example.com:22"]);
        assert!(a.decide(&dest("git.example.com:443")).is_allowed());
        assert!(a.decide(&dest("git.example.com:22")).is_allowed());
        let denied = a.decide(&dest("git.example.com:9418"));
        assert_eq!(
            denied,
            Verdict::Deny(Denial::WrongPort { allowed: vec![22, 443] })
        );
        // The message has to be actionable: the session cannot read the
        // allowlist, so "denied" on its own gets retried forever.
        let text = denied.explain(&dest("git.example.com:9418"));
        assert!(text.contains("22, 443"), "{text}");
        assert!(text.contains("9418"), "{text}");
    }

    #[test]
    fn a_name_is_matched_case_and_trailing_dot_insensitively() {
        // A resolver hands back the absolute form, a user types the relative
        // one, and a browser lowercases. Three spellings of one host must not
        // be three different destinations, one of which the rule misses.
        let a = list(&["API.Example.COM"]);
        for spelling in [
            "api.example.com:443",
            "API.EXAMPLE.COM:443",
            "Api.Example.Com.:443",
        ] {
            assert!(a.decide(&dest(spelling)).is_allowed(), "{spelling}");
        }
    }

    #[test]
    fn an_address_is_only_reachable_when_it_was_written_down_as_an_address() {
        // The DNS-rebinding guard, which is the whole reason there are two
        // questions rather than one. The daemon that connects is in the host's
        // network namespace, so an allowed name pointing at 127.0.0.1 would
        // reach whatever the user is running there.
        let a = list(&["api.example.com"]);
        let d = dest("api.example.com:443");
        assert!(a.decide(&d).is_allowed(), "the name itself is allowed");

        for local in [
            "127.0.0.1",
            "127.6.6.6",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            // The one that walks past a check written against Ipv6Addr alone.
            "::ffff:127.0.0.1",
        ] {
            let addr: IpAddr = local.parse().unwrap();
            assert_eq!(
                a.accepts_address(&d, addr),
                Verdict::Deny(Denial::LocalAddress(unmap(addr))),
                "{local} was reachable through a name-based rule"
            );
        }

        // A public address through the same rule is fine.
        for public in ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(
                a.accepts_address(&d, public.parse().unwrap()).is_allowed(),
                "{public}"
            );
        }
    }

    #[test]
    fn an_address_rule_reaches_the_address_it_names_and_no_other() {
        // The escape hatch for a machine on the LAN, and it is narrow: the
        // user wrote the address down, so that address is allowed and its
        // neighbours are not.
        let a = list(&["192.168.1.10:8080"]);
        let d = Destination::new("192.168.1.10", 8080).unwrap();
        assert!(a.decide(&d).is_allowed());
        assert!(a
            .accepts_address(&d, "192.168.1.10".parse().unwrap())
            .is_allowed());

        // A different private address, through the same rule, is not.
        assert_eq!(
            a.accepts_address(&d, "192.168.1.11".parse().unwrap()),
            Verdict::Deny(Denial::LocalAddress("192.168.1.11".parse().unwrap()))
        );
        // And the address rule does not become a name rule.
        assert_eq!(
            a.decide(&Destination::new("nas.local", 8080).unwrap()),
            Verdict::Deny(Denial::NoRule)
        );
    }

    #[test]
    fn accepts_address_never_allows_what_decide_refused() {
        // The two questions are asked in order and the second cannot overrule
        // the first — otherwise an IP literal rule would become a way past the
        // name check.
        let a = list(&["api.example.com"]);
        let other = dest("other.example.com:443");
        assert_eq!(
            a.accepts_address(&other, "93.184.216.34".parse().unwrap()),
            Verdict::Deny(Denial::NoRule)
        );
    }

    #[test]
    fn a_wildcard_broad_enough_to_be_meaningless_is_refused() {
        for line in ["*", "*.", "*.com", "*.internal", "*.local"] {
            assert!(
                matches!(
                    Rule::parse(line),
                    Err(RuleError::TooBroad(_)) | Err(RuleError::BadWildcard(_))
                ),
                "{line} was accepted as a rule"
            );
        }
        // And it is refused for the whole list, not skipped.
        assert!(Allowlist::parse(&["api.example.com", "*.com"]).is_err());
    }

    #[test]
    fn a_wildcard_anywhere_but_the_front_is_refused_rather_than_guessed_at() {
        for line in ["a*.example.com", "*example.com", "ex*ample.com", "api.*.com"] {
            assert!(
                Rule::parse(line).is_err(),
                "{line} was accepted, and it is not clear what it would mean"
            );
        }
    }

    #[test]
    fn a_url_or_a_path_is_refused_with_the_reason() {
        for (line, want) in [
            ("https://api.example.com", "URL"),
            ("api.example.com/v1", "path"),
        ] {
            let err = Rule::parse(line).expect_err(line);
            assert!(err.to_string().contains(want), "{err}");
        }
    }

    #[test]
    fn a_name_that_is_not_ascii_is_refused_rather_than_transcoded() {
        // Guessing the punycode form is how a rule and a destination end up
        // looking different while naming one host — or, worse, the same while
        // naming two.
        assert!(Rule::parse("exämple.com").is_err());
        assert!(Destination::parse("exämple.com:443").is_err());
        // The punycode form itself is an ordinary name.
        assert!(Rule::parse("xn--exmple-cua.com").is_ok());
    }

    #[test]
    fn a_destination_needs_a_real_port() {
        for bad in [
            "api.example.com",
            "api.example.com:",
            "api.example.com:0",
            "api.example.com:https",
            "api.example.com:99999",
            "",
            ":443",
        ] {
            assert!(Destination::parse(bad).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn an_ipv6_destination_is_read_from_its_bracketed_form() {
        let d = dest("[2001:db8::1]:8443");
        assert_eq!(d.host(), "2001:db8::1");
        assert_eq!(d.port(), 8443);
        assert_eq!(d.to_string(), "[2001:db8::1]:8443");
        assert_eq!(d.literal_address(), Some("2001:db8::1".parse().unwrap()));

        let a = list(&["[2001:db8::1]:8443"]);
        assert!(a.decide(&d).is_allowed());
        assert!(!a.decide(&dest("[2001:db8::2]:8443")).is_allowed());
    }

    #[test]
    fn duplicate_rules_collapse_and_the_lines_round_trip() {
        let a = list(&[
            "api.example.com",
            "api.example.com",
            "api.example.com:443",
            "*.example.com:8443",
        ]);
        assert_eq!(a.len(), 2);
        assert_eq!(a.lines(), vec!["api.example.com", "*.example.com:8443"]);
        // And a list rebuilt from its own lines decides identically.
        let again = Allowlist::parse(&a.lines()).expect("reparse");
        assert_eq!(again, a);
    }

    #[test]
    fn a_blank_line_is_refused_so_a_stray_comma_is_not_a_silent_hole() {
        assert_eq!(Rule::parse("   "), Err(RuleError::Empty));
        assert!(Allowlist::parse(&["api.example.com", ""]).is_err());
    }

    #[test]
    fn every_refusal_says_what_to_do_next() {
        let a = list(&["api.example.com"]);
        let d = dest("other.example.com:443");
        let text = a.decide(&d).explain(&d);
        assert!(text.contains("other.example.com"), "{text}");
        assert!(text.len() > 40, "unhelpful refusal: {text}");

        let d = dest("api.example.com:443");
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let text = a.accepts_address(&d, local).explain(&d);
        assert!(text.contains("127.0.0.1"), "{text}");
        assert!(text.contains("allow the address"), "{text}");
    }
}
