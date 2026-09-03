#!/usr/bin/env node
// ─────────────────────────────────────────────────────────────────────────────
// apex-plugin-verdict.js — ask APEX Shell's OWN plugin validator what it thinks
// of a plugin directory, and print the answer as JSON.
//
// WHY THIS FILE IS SIX LINES OF LOGIC AND A LONG COMMENT
//
// The shell's plugin platform (roadmap §16) keeps every decision about a
// manifest in `src/services/plugins/manifest.js`: the permission vocabulary,
// which permissions apiVersion 1 will actually grant, the id and version
// charsets, the API compatibility policy, the import allowlist, the forbidden
// constructs, and the human-readable text for every refusal. That file is plain
// JavaScript rather than QML for exactly this reason — the QML engine loads it
// with `import "manifest.js" as Manifest`, Node `require`s the very same file
// in the shell's own tests, and now so does this.
//
// NOT A COPY. `require` of the shipped file, at
// /usr/share/apex-shell/src/services/plugins/manifest.js, which is where
// Containerfile.base vendors the shell. If `apex plugin` reimplemented any of
// those rules there would be two answers to "may this plugin load", they would
// drift, and the CLI would tell a user their plugin is fine while the shell
// refused it — or worse, the reverse.
//
// WHAT IS NOT IN manifest.js, AND THEREFORE IS DUPLICATED
//
// Four refusals live in `PluginService.qml`, not in manifest.js, because they
// are facts about the DIRECTORY rather than about the manifest:
//
//   * the plugin directory contains a symlink at any depth
//   * no .qml file in it
//   * more than one .qml file
//   * the manifest's `entry` is not the one .qml that is there
//
// `apex-plugin` measures those facts in shell and this file applies them, in
// the same order `PluginService._decide()` does. That ordering and those four
// conditions are the only duplication in this feature, it is declared here, and
// `tests/test-apex-plugin.sh` fails if PluginService.qml grows a fifth
// structural refusal or changes the fields its own scan emits — so the drift is
// detectable by a test rather than hoped against.
//
// Even the WORDING of those four refusals comes from manifest.js:
// `describeRefusal()` already knows every reason code, so this file passes the
// code through it instead of writing English of its own.
//
// USAGE
//
//   apex-plugin-verdict.js validid <manifest.js> <id>
//       exit 0 if the id is one manifest.js would accept, 1 if not.
//
//   apex-plugin-verdict.js verdict <manifest.js> <dir> <id> <qml> <links> <name>
//       print one JSON object: { ok, reason, detail, describe, … }.
//       Exit 0 whatever the verdict; exit 2 only if it could not run at all.
// ─────────────────────────────────────────────────────────────────────────────

"use strict";

const fs = require("fs");
const path = require("path");

function die(message) {
    process.stderr.write("apex-plugin: " + message + "\n");
    process.exit(2);
}

const argv = process.argv.slice(2);
const mode = argv[0];
const manifestPath = argv[1];

if (!mode || !manifestPath) {
    die("usage: apex-plugin-verdict.js <validid|verdict> <manifest.js> …");
}

let Manifest;
try {
    Manifest = require(path.resolve(manifestPath));
} catch (e) {
    die("cannot load the shell's plugin validator at " + manifestPath + ": " + e.message);
}

// A manifest.js that loaded but exports nothing usable is worse than one that
// did not load: every verdict below would be `undefined is not a function`
// halfway through, and the CLI would look broken rather than the coupling.
for (const name of ["validateManifest", "scanSource", "validId", "describeRefusal"]) {
    if (typeof Manifest[name] !== "function") {
        die(manifestPath + " does not export " + name + "(); it is not the shell's plugin validator");
    }
}

if (mode === "validid") {
    process.exit(Manifest.validId(argv[2]) ? 0 : 1);
}

if (mode !== "verdict") {
    die("unknown mode " + mode);
}

const dir = argv[2];
const id = argv[3];
const qmlCount = parseInt(argv[4], 10);
const symlinks = parseInt(argv[5], 10);
const qmlName = argv[6] || "";

if (!dir || !id || Number.isNaN(qmlCount) || Number.isNaN(symlinks)) {
    die("verdict needs <dir> <id> <qmlCount> <symlinks> <qmlName>");
}

// An unreadable file reads as empty, which is what PluginService does: its
// FileView `onLoadFailed` sets the text to "" and lets the validator refuse.
// Matching that means an unreadable manifest and a manifest full of nonsense
// get the same reason code from the same function, rather than one of them
// getting a message invented here.
function readOrEmpty(file) {
    try {
        return fs.readFileSync(file, "utf8");
    } catch (e) {
        return "";
    }
}

function refuse(reason, detail) {
    const r = { ok: false, reason: reason, detail: detail === undefined ? "" : detail };
    r.describe = Manifest.describeRefusal(r);
    process.stdout.write(JSON.stringify(r) + "\n");
    process.exit(0);
}

// ── the same order PluginService._decide() uses ──────────────────────────────
// Structural first: those are facts about the directory, so no amount of
// manifest editing changes them, and checking them first is what stops a
// manifest being parsed out of a directory that is already refused.

// The id charset, through manifest.js. `apex-plugin` also refuses a path
// separator before it builds any path — that is a second lock on the same door,
// not a second opinion about what an id is.
if (!Manifest.validId(id)) {
    refuse("bad-id", id);
}
if (symlinks > 0) {
    refuse("entry-outside-plugin", "the plugin directory contains a symlink");
}
if (qmlCount === 0) {
    refuse("entry-missing", "no .qml in the plugin directory");
}
if (qmlCount > 1) {
    refuse("extra-qml", qmlCount + " .qml files; apiVersion 1 allows one");
}

const grant = Manifest.validateManifest(
    readOrEmpty(path.join(dir, "plugin.json")),
    id,
    Manifest.API_VERSION
);
if (!grant.ok) {
    refuse(grant.reason, grant.detail);
}

// The manifest names an entry; the directory holds exactly one .qml. They have
// to be the same file, or the scan below would be checking something other than
// what the shell would load.
if (grant.entry !== qmlName) {
    refuse("entry-missing", "entry is " + grant.entry + " but the directory holds " + qmlName);
}

const scan = Manifest.scanSource(readOrEmpty(path.join(dir, grant.entry)));
if (!scan.ok) {
    refuse(scan.reason, scan.detail);
}

process.stdout.write(
    JSON.stringify({
        ok: true,
        reason: "",
        detail: "",
        describe: "",
        id: grant.id,
        name: grant.name,
        version: grant.version,
        apiVersion: grant.apiVersion,
        entry: grant.entry,
        extensionPoint: grant.extensionPoint,
        permissions: grant.permissions,
        networkHosts: grant.networkHosts,
        description: grant.description,
        // The host's own version, so `apex plugin info` can say what this
        // machine implements rather than only what the plugin asked for.
        hostApiVersion: Manifest.API_VERSION,
        // The full vocabulary and the enforceable subset, both from
        // manifest.js. `apex plugin info` prints them so a user can see that a
        // permission exists and is deliberately not granted, which is the
        // distinction the shell refuses at load rather than granting silently.
        allPermissions: Manifest.PERMISSIONS,
        implementedPermissions: Manifest.IMPLEMENTED_PERMISSIONS,
    }) + "\n"
);
