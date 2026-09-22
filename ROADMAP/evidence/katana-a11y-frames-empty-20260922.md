# The shell's accessibility frames are empty — measured, 2026-09-22

On katana, on the image built this session, in a live Hyprland session with the
bridge on. Walked one level deeper than the earlier count.

```
quickshell: 12 frames
  frame 0..11   name=''   role=frame   children=0
```

Twelve frames on the AT-SPI bus. **Every one has no name and no children.**

## What this changes

`katana-a11y-20260922.md` recorded `ChildCount=8` and called the runtime half
of P2-003 closed. That was the count of *frames*, and it was right about the
bridge: quickshell-git publishes where released 0.3.1 published nothing.

It was not a statement about whether a screen reader can use the shell, and
this is. **A screen reader reaches twelve nameless windows with nothing inside
them.** No names, no roles below frame, no `Action` interface, nothing to
navigate. Functionally that is the same experience as the one node it replaced.

## The distinction that matters for whoever fixes it

The frames arriving proves the **factory** is installed — that was the
`916a0dd` fix, and it works. Frames with `children=0` says the **scene graph
below each window is not being walked**, which is a different mechanism and is
not fixed by anything measured so far.

So P2-003's remaining work splits in two, and only the first is APEX's:

1. **Is apex-shell's markup reachable at all?** The repository already asserts
   it at source level — `check-lockscreen-a11y.sh` is 33/0/0 and its mutation
   pair catches 20 — and the same evidence file called that markup
   "source-correct and runtime-unreachable". This measurement says it still is.
   Adding more `Accessible.name` to QML cannot help while the tree below the
   frame is empty; that would be writing markup nothing walks.
2. **Why the contentItem subtree does not publish.** Upstream-shaped. Qt builds
   a QQuickWindow's accessible tree from its `contentItem`, and a frame with
   zero children means that walk is not happening or is finding nothing
   accessible. Not diagnosed here, and not guessed at.

## Do not read this as a regression

Nothing got worse. The earlier reading was a true measurement of a narrower
question. The count also moved 8 → 12 for a known reason: four surfaces per
output, and this session has more surfaces up than the one that produced the 8.

The honest status of P2-003 is: **the bridge is fixed and proven; the tree is
still empty; and the next step is a diagnosis, not markup.**
