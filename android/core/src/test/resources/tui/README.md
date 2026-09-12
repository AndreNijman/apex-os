# TUI fixtures

Recorded on 2026-09-12 on the L16, against a real PTY, with
`android/tools/record-tui-fixture.py`. Nothing here is hand-written: each file
is the literal byte stream one of the agent CLIs produced in its first few
seconds on an 80x24 `xterm-256color` PTY with a scratch `HOME`.

* `*-queries.bin` — recorded against a terminal that answered **nothing**.
* `*-session.bin` — recorded against a terminal that answered the queries, in
  the shapes xterm uses.

The pair is the evidence for "works with the TUIs without rewriting them".
opencode is the clearest case: 284 bytes and a blank screen when its questions
go unanswered, 12665 bytes and a painted interface when they are answered.

Versions at recording time: Claude Code v2.1.260, Codex (OpenAI), opencode.
**gemini is absent from this machine**, so no fixture exists for it and nothing
here claims one was tested.

To re-record:

```
python3 android/tools/record-tui-fixture.py claude out.bin 5 /tmp/scratch-home
```
