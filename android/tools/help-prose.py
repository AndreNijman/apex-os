#!/usr/bin/env python3
"""Every word the in-app guide shows, and nothing else.

slopcheck reads prose. Handing it Help.kt hands it Kotlin as well, and
`private fun h(t: String) = Block(Kind.H, t)` three times in a row is flagged
as three sentences of the same length — which is true, and is not prose. The
desktop has the same split for the same reason: its words live in
AgentHelpContent.qml so tests/check-agent-help.sh can read them without QML
around them.

So this pulls out exactly what a user sees: the block texts, the kv terms, the
section titles and the entry label. Nothing else from the file reaches the
checker, and nothing a user sees is left out of it.
"""
import re
import sys

src = open(sys.argv[1]).read()

# The guide is everything from `val sections` to the `covered` property.
start = src.index("val sections: List<Section> = listOf(")
end = src.index("/** Every desktop section this guide answers to. */")
body = src[start:end]

out = []
m = re.search(r'const val ENTRY_LABEL: String = "([^"]*)"', src)
if m:
    out.append(m.group(1))

# Kotlin joins adjacent literals with `+`. A concatenation split across lines is
# one sentence to a reader and would otherwise reach the checker as fragments,
# so runs of `"..." +` are rejoined before anything is measured.
for run in re.finditer(r'((?:"(?:[^"\\]|\\.)*"\s*\+\s*)*"(?:[^"\\]|\\.)*")', body):
    text = "".join(re.findall(r'"((?:[^"\\]|\\.)*)"', run.group(1)))
    text = text.replace('\\"', '"').replace("\\n", "\n").strip()
    if text:
        out.append(text)

print("\n\n".join(out))
