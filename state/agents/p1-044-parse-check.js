#!/usr/bin/env node
// P1-044 — did the shell's parsers ever meet the CLI's real output?
//
// tests/firewall-test.js drives src/services/firewall.js against fixtures. This
// asks the question one level down: were those fixtures faithful? Every payload
// below was captured on 2026-09-07 from the helper on apex-os branch
// `task/p1-044-firewall-live`, including the two readings the first pass could
// not take, because they need root and a policy actually loaded in a kernel.
//
//   node ROADMAP/state/agents/p1-044-parse-check.js
//
// FW=/path/to/firewall.js overrides where the parser is read from; the default
// is the shell worktree this was written against.
"use strict";
const FW = process.env.FW
    || "/var/tmp/apex-work/wt-p1-044-shell/src/services/firewall.js";
const fw = require(FW);

// captured: `apex firewall list`, on the L16
const LIST = "NAME          PROTO PORT   DESCRIPTION\nssh           tcp   22     Remote shell, and how APEX remote agents and `apex host run` reach this machine\nhttp          tcp   80     A web server you are running\nhttps         tcp   443    A web server you are running, over TLS\nmdns          udp   5353   Local name discovery, for printers and `apex host`\nsamba         tcp   445    Windows file sharing\nnfs           tcp   2049   NFS file sharing\nipp           tcp   631    Sharing a printer attached to this machine\nsteam-remote  udp   27036  Steam Remote Play from another machine on this network\nsunshine      tcp   47989  Sunshine game streaming host\nollama        tcp   11434  A local model server, reachable from other machines\nsyncthing     tcp   22000  Syncthing peer connections\n";
// captured: `apex firewall status` as andre on the L16 — it cannot look
const STATUS_NONROOT = "apex-firewall: policy: cannot read the ruleset; reading it needs root\napex-firewall:   try: sudo apex firewall status\n\nalways allowed, and not removable here:\n  established replies, loopback, ICMP, DHCP, mDNS/LLMNR, ssh\n\nexceptions you have added:\n  (none)\n";
// captured: `/run/apex-fw/libexec/apex-firewall status` as root on katana over
// a policy loaded in the kernel, with syncthing allowed and a deliberately
// malformed broken.conf next to it
const STATUS_ROOT = "apex-firewall: policy: incoming dropped by default, outgoing allowed\n\nalways allowed, and not removable here:\n  established replies, loopback, ICMP, DHCP, mDNS/LLMNR, ssh\n\nexceptions you have added:\n  broken        could not be applied \u2014 rejected: tcp notaport\n  syncthing     tcp 22000";

let pass=0, fail=0;
const ok=(d,c)=>{ if(c){console.log("  PASS  "+d); pass++;} else {console.log("  FAIL  "+d); fail++;} };

// ── the catalogue, as `apex firewall list` really prints it ──
const cat = fw.parseCatalogue(0, LIST);
ok("the header row is not a service", !cat.some(r=>r.name==="NAME"));
ok("all 11 catalogue entries parsed", cat.length===11);
const ssh = cat.find(r=>r.name==="ssh");
ok("a description containing backticks survives whole",
   ssh && ssh.description === "Remote shell, and how APEX remote agents and `apex host run` reach this machine");
ok("steam-remote is udp 27036", cat.some(r=>r.name==="steam-remote"&&r.proto==="udp"&&r.port==="27036"));

// ── status, non-root on a machine that cannot look ──
const nr = fw.parseStatus(0, STATUS_NONROOT);
ok("a non-root read is 'unreadable', not 'notloaded'", nr.policy==="unreadable");
ok("and it is not mistaken for an empty exception list", nr.ok===true && nr.exceptions.length===0);
ok("the always-allowed line is read", nr.alwaysAllowed.indexOf("established replies")===0);

// ── status, root, policy loaded, one good + one rejected exception ──
const rt = fw.parseStatus(0, STATUS_ROOT);
ok("a loaded policy reads as loaded", rt.policy==="loaded");
ok("both exceptions are listed", rt.exceptions.length===2);
const good = rt.exceptions.find(e=>e.name==="syncthing");
const bad  = rt.exceptions.find(e=>e.name==="broken");
ok("the working one carries its proto and port", good && good.proto==="tcp" && good.port==="22000" && good.rejected===false);
ok("the rejected one is flagged rejected", bad && bad.rejected===true);
ok("and carries the reason the helper gave", bad && bad.detail==="tcp notaport");
ok("the rejected one claims no port", bad && bad.port==="");

// ── the unit, in the three states systemctl really reports ──
const u = (l,a)=>fw.parseUnit(0, "LoadState="+l+"\nActiveState="+a+"\n");
const absent  = u("not-found","inactive");
const stopped = u("loaded","inactive");
const running = u("loaded","active");
ok("LoadState=not-found is 'absent', not 'inactive'", absent==="absent");
ok("a present but stopped unit is 'inactive'", stopped==="inactive");
ok("a running unit is 'active'", running==="active");
ok("absent and stopped are different answers", absent!==stopped);

// ── the shown commands ──
ok("allow is shown with sudo and never run", fw.allowCommand("syncthing")==="sudo apex firewall allow syncthing");
ok("deny is shown with sudo and never run",  fw.denyCommand("syncthing")==="sudo apex firewall deny syncthing");

// ── an image that predates `apex firewall` ──
const old = fw.parseStatus(2, "");
ok("an unrecognised subcommand does not read as 'nothing is open'", old.ok===false);

console.log("\nreal-output parse: "+pass+" passed, "+fail+" failed");
process.exit(fail?1:0);
