---
title: "APEX Shell Master UI/UX Roadmap"
subtitle: "Motion, fluid surfaces, visual design, interaction architecture, performance and rollout"
version: "3.0"
date: "2026-09-20"
status: "Pre-implementation roadmap"
companion: "APEX-Shell-Fluid-Visual-Design-Roadmap-v1.md"
---

# APEX Shell Master UI/UX Roadmap v3

## 0. Purpose

This is the implementation roadmap for the next APEX Shell UI/UX pass. It replaces the previous motion-only roadmap.

The goal is not simply to make animations smoother. The goal is to make the shell feel like one designed desktop environment with a clear visual language, distinct surface shapes, fluid geometry, responsive controls, coherent navigation, strong accessibility, and measurable frame pacing.

The companion document, **APEX Shell Fluid + Visual Design Roadmap v1**, defines the actual shape families, visual system, motion motifs, component styling and surface-specific design rules. This master roadmap defines the engineering order, dependencies, acceptance gates and rollout.

## 1. Finished-state target

APEX should feel immediate when the user interacts with it and expressive only when the interaction deserves it.

The finished shell should behave like this:

- The center notch physically becomes the Dashboard through a distinctive center bloom morph.
- Right-side quick surfaces pour or unfurl from the right notch rather than using the same resize animation as Dashboard.
- Left-side surfaces spill from the left edge with a different silhouette and timing pattern.
- Notifications behave as a physical stack, not a set of rectangles whose positions teleport.
- Launcher interaction prioritizes typing latency, with a light lens/reveal treatment rather than a large slow blob morph.
- Nexus is intentionally calmer and more stable than the rest of the shell.
- OSDs use a compact capsule motion language.
- Context menus use quick pivoted scale/fade motion instead of fluid morphing.
- One selection object travels between tabs and navigation rows instead of every item independently changing state.
- Buttons react on pointer-down, not after a long color tween.
- Large shape motion leads content motion, giving the impression that content lives inside the shell surface.
- Every animation can be interrupted or reversed without visual jumps or stale state.
- Reduced Motion changes the whole shell, including hardcoded legacy animations.
- Hyprland windows and workspaces use motion that feels related to APEX Shell.
- The UI itself has a stronger hierarchy: spacing, typography, surface levels, controls, icon treatment and color roles are standardized.

## 2. Current implementation findings

The current shell already solved a substantial set of background-work problems. Lazy loading, refcounted services and fullscreen unmapping should be preserved. This project should not repeat that optimization pass.

The remaining UI/UX issues are structural.

### 2.1 Motion timing is too global in some places and too local in others

`SettingsService.animDuration` defaults to 320 ms. `Theme.animDuration` feeds many major surface animations.

At the same time, individual components contain literal timings from roughly 80 to 400 ms.

This creates two opposite problems:

- unrelated large interactions share the same 320 ms weight;
- small interactions invent their own timing locally.

The result is a shell with animation, but not a coherent motion system.

### 2.2 Large geometry relies heavily on `InOutCubic`

Dashboard, Network, Notifications, ArchMenu and related surfaces repeatedly use width/height animation with `Easing.InOutCubic`.

`InOutCubic` is smooth, but its slow beginning can feel like input latency. Frequent desktop interactions should normally react hard at the start and settle softly.

### 2.3 Popup lifetime is coupled to guessed animation time

Patterns such as:

```qml
interval: Theme.animDuration + 20
```

are used to keep windows mapped during close transitions.

This makes lifecycle correctness depend on a timing number rather than actual transition completion.

### 2.4 Current fluid geometry is visually limited

`PopupShape.qml` and `SeamlessBarShape.qml` are Canvas paths built mostly from fixed rounded corners and quadratic flare curves.

They can resize and join the bar cleanly, but every large popup is still derived from the same small vocabulary:

- rectangle;
- rounded rectangle;
- concave flare;
- straight joined edge.

This is why different APEX surfaces can feel like the same blob at different dimensions.

### 2.5 Internal layout changes throughout outer shape animation

Several surfaces animate their actual width and height while child layouts are live inside them. This can cause repeated relayout during every intermediate frame.

The new system should separate:

1. outer shell geometry;
2. clipping/masking;
3. stable target content layout;
4. content entrance/exit motion.

### 2.6 Visual hierarchy is under-specified

Current APEX has strong individual visual ideas, but many local values remain one-off:

- radii;
- padding;
- control heights;
- opacity levels;
- hover fills;
- selected fills;
- icon treatment;
- surface elevation.

The UI should be redesigned as a component system, not just retimed.

## 3. Program rules

These rules apply to every phase.

### 3.1 Input wins over animation

No decorative animation may delay:

- typing;
- clicking;
- keyboard focus;
- switching tabs;
- dismissing a surface;
- continuous drag tracking.

### 3.2 Shape motion and content motion are separate systems

The shell shape can be expressive. Content should generally move less.

### 3.3 Different surface classes get different motion motifs

Do not reuse the same morph everywhere.

The design families are defined in the companion visual roadmap.

### 3.4 Exits are usually faster than entrances

Closing should not force the user to watch the full opening animation in reverse.

### 3.5 User-controlled gestures are direct

While a pointer or touch gesture owns movement, the target should follow it directly. Physics is used on release, not underneath active input.

### 3.6 No new raw motion literals

Animation timing and curves should come from a central motion system unless an exception is documented.

### 3.7 No blind performance cargo culting

Do not copy Qt render-loop flags, shaders, blur settings or compositor animation values from another shell without measuring them on APEX.

### 3.8 Preserve current functional behavior unless a phase explicitly changes it

UI work must not regress:

- multi-monitor behavior;
- fullscreen handling;
- input regions;
- focus handling;
- lazy loading;
- service refcounting;
- lock behavior;
- compositor compatibility.

# MILESTONE A - FOUNDATION

## Phase 0. Baseline, inventory and visual capture

### Tasks

- Inventory every `Behavior`, `Transition`, `NumberAnimation`, `ColorAnimation`, `PropertyAnimation`, `SmoothedAnimation`, `SpringAnimation`, `SequentialAnimation`, `ParallelAnimation`, animation-related Timer and direct `duration:` literal.
- Inventory every surface shape and shape branch.
- Record videos of the current shell at normal speed and slow motion.
- Capture screenshots of all major surfaces in default, hover, pressed, selected, disabled and focused states.
- Record cold and warm open timings.
- Record frame timing at 60 Hz and at least one high-refresh target.
- Record input-to-visible-response timing for Dashboard, Network, Notifications, Launcher and Nexus.
- Record transition reversal behavior.

### Surface list

- Top bar and all three notches
- Dashboard
- Network
- Audio
- Notifications center
- Notification toast
- Clipboard
- Wallpaper
- ArchMenu / power
- QuickControl
- Context menus
- Launcher
- Nexus
- OSDs
- Lock/greeter transition where shell-controlled

### Done when

- Every animation has a category and owner.
- Every major surface has a visual baseline.
- Current frame pacing and latency are known.
- There is enough baseline material to prove improvement or regression.

## Phase 1. Central motion architecture

Create a first-class motion system, for example:

```text
src/theme/Motion.qml
```

Expose it through Theme or import it directly in motion-aware components.

### Required token classes

```text
micro
hover
pressIn
pressOut
state
selection
page
surfaceEnterSmall
surfaceExitSmall
morphEnter
morphExit
notificationShift
hero
```

### Initial Balanced targets

| Token | Starting value |
| --- | ---: |
| Hover | 80 ms |
| Press in | 65 ms |
| Press release | 115 ms |
| Tiny state change | 130 ms |
| Selection travel | 160 ms |
| Page transition | 200 ms |
| Small surface enter | 190 ms |
| Small surface exit | 135 ms |
| Large morph enter | 240 ms |
| Large morph exit | 175 ms |
| Notification reflow | 200 ms |
| Typical hero limit | 320 ms |

### Required curve families

- standard
- standardDecel
- standardAccel
- emphasized
- emphasizedDecel
- emphasizedAccel
- fastSpatial
- defaultSpatial
- slowSpatial

### User settings

Replace the raw-duration-first UX with:

```text
Motion speed
- Snappy
- Balanced
- Relaxed

Reduce Motion

Advanced
- duration scale
```

### Reduced Motion policy

Reduced Motion should:

- remove large translation;
- remove shape morph spectacle;
- remove scale bounce;
- remove spring overshoot;
- retain short opacity and color feedback;
- retain necessary state communication.

### CI guard

Add a lint/check that rejects new literal animation durations outside the motion system and documented exceptions.

### Done when

- Motion tokens exist.
- Motion speed changes the whole migrated system coherently.
- Reduced Motion is central, not opt-in per component.
- New arbitrary timing literals cannot silently accumulate.

## Phase 2. Visual design tokens

Create a stronger design system before redesigning individual pages.

### Spacing

Use a deliberate scale:

```text
4 / 8 / 12 / 16 / 24 / 32
```

### Radius roles

Define semantic roles instead of local numbers:

```text
radiusXS
radiusS
radiusM
radiusL
radiusXL
radiusFull
```

### Surface roles

Expand the current color system beyond one `background` role.

Target concepts:

```text
surfaceBase
surfaceRaised
surfaceOverlay
surfaceSelected
surfaceHover
surfacePressed
outlineSoft
outlineStrong
accent
accentContainer
onAccent
```

These can still derive from Matugen and the existing palette.

### Typography roles

```text
caption
bodySmall
body
bodyStrong
section
heading
pageTitle
display
mono
```

### Control heights

```text
compact: 32
normal: 36-40
prominent: 40+
```

Visual controls may be compact while keeping larger invisible hit regions.

### Icon system

Create a single icon wrapper so icon baseline, size, opacity, weight and active treatment are consistent even while Nerd Font glyphs remain in use.

Long term, SVG or another normalized icon source can be evaluated, but icon-source migration is not required to begin the UI pass.

### Done when

- Shared visual tokens exist.
- Common components no longer need random local radii/padding/opacity values for ordinary states.

## Phase 3. Shared interaction primitives

Create reusable controls such as:

```text
ApexPressable
ApexButton
ApexIconButton
ApexToggle
ApexSlider
ApexSelectionPill
ApexFocusRing
```

### Press behavior

Pointer down must produce visible response immediately.

Balanced starting values:

```text
scale 1.0 -> ~0.975 in 55-75 ms
release -> 1.0 in 100-130 ms
```

Do not add visible cartoon bounce.

### Hover behavior

Prefer:

- subtle surface tint;
- slight border/elevation change;
- icon/text contrast shift.

Avoid scaling everything on hover.

### Keyboard behavior

- Space/Enter gives equivalent activation feedback.
- Focus-visible state appears for keyboard navigation.
- Pointer use does not leave strong focus rings everywhere.

### Migration targets

- CfgButton
- ProfileButton
- small icon buttons
- top bar controls
- Nexus controls
- tab controls
- quick settings tiles
- common row actions

### Done when

- The most common APEX interactions all share one physical response language.

# MILESTONE B - FLUID SURFACE ENGINE

## Phase 4. Replace fixed shape branches with a parametric surface system

The current Canvas-based `PopupShape.qml` should stop being the final abstraction for all large surfaces.

Create a new fluid shape layer. The exact final class names can change, but the architecture should separate:

```text
FluidSurface
FluidShapeModel
FluidShapeRenderer
FluidMorph
```

### Rendering direction

Preferred first implementation to evaluate:

- `QtQuick.Shapes` / `ShapePath`;
- cubic Bezier segments;
- normalized control points;
- animated control-point properties;
- clipping/masking independent from content layout.

Keep the existing Canvas renderer as a fallback/reference until the new renderer proves correct and performant.

A shader/SDF/metaball renderer is an optional later experiment, not the first implementation requirement.

### Core shape parameters

The model should be able to express at least:

```text
progress 0..1
anchor edge
anchor center
anchor width
anchor height
target width
target height
neck width
neck depth
shoulder tension
corner radius start/end
leading-edge bias
trailing-edge lag
asymmetry
overshoot amount
```

### Geometry quality requirements

- C1 tangent continuity at bar-to-surface joints where possible.
- No visible kinks at the neck.
- No self-intersections.
- No momentary negative/zero body regions.
- No corner radius popping.
- Anchor contact must remain visually stable.
- Anti-aliasing must remain crisp at 1x and scaled UI sizes.

### Done when

- A single normalized progress value can drive a clean shape morph.
- Surface geometry no longer depends on only a handful of fixed Canvas switch branches.
- At least three visually different fluid families can be expressed by the same underlying system.

## Phase 5. Implement distinct APEX shape families

Use the companion visual roadmap as the geometry specification.

Required families:

1. **CENTER_BLOOM** - Dashboard and large center-notch expansions.
2. **RIGHT_POUR** - Network/audio/other right-side quick surfaces.
3. **LEFT_SPILL** - Power/ArchMenu and left-side surfaces.
4. **STACK_REFLOW** - Notifications. Primarily card physics rather than blob morphing.
5. **LENS_REVEAL** - Launcher/search surfaces where a lighter reveal is better than a full fluid body.
6. **QUIET_SHEET** - Nexus/settings. Minimal morphing.
7. **CAPSULE** - OSDs and compact passive indicators.
8. **PIVOT_POP** - Context menus and small pointer-origin menus.

### Important rule

Do not give every surface a different animation just to be different.

Differences must follow surface role and origin. Similar surfaces should still feel related.

### Done when

- A user can identify the class of surface partly from how it moves and changes shape.
- Dashboard, right quick panel, left quick panel and Nexus do not look like the same rounded rectangle animation.

## Phase 6. Surface lifecycle state machine

Replace duration-coupled window lifetime with explicit transition state.

Required conceptual states:

```text
Closed -> Opening -> Open -> Closing
```

or equivalent progress-driven logic.

### Requirements

- surface remains mapped until actual close completion;
- reopening during close reverses smoothly from current state;
- closing during open reverses smoothly;
- no stale close timers;
- focus release/acquisition is intentional;
- input region follows visible geometry safely;
- Reduced Motion uses the same state machine.

### Migration order

Start with a small popup, then migrate:

- Network
- Audio
- Notifications
- Clipboard
- Wallpaper
- ArchMenu
- Dashboard
- other transient surfaces

### Done when

No primary popup lifetime depends on `animDuration + padding`.

# MILESTONE C - CONNECTED NAVIGATION AND CORE SURFACES

## Phase 7. Connected tab and navigation system

Refactor `TabSwitcher.qml`.

### Shared selection object

There should be one moving selection surface, not one active background per tab.

Animate its:

- x/y;
- width/height;
- corner treatment if necessary.

### Page direction

If moving right through tabs:

```text
old content: 0 -> -10/16 px + fade
new content: +10/16 px -> 0 + fade
```

Reverse when moving left.

Target roughly 180-210 ms Balanced.

### Apply to

- Dashboard
- Network
- any Audio subpages
- Nexus navigation
- ArchMenu subpages if retained

### Done when

Selection visibly travels rather than disappearing from one item and appearing on another.

## Phase 8. Dashboard v2

Dashboard is the primary APEX hero surface.

### Shape

Use `CENTER_BLOOM`.

The shape should:

- stay visibly connected to the center notch;
- open symmetrically at first;
- widen and deepen with controlled neck tension;
- let the outer body settle before the final content motion completes;
- avoid looking like a rubber balloon.

### Content

Use stable target layout behind the animated outer mask where practical.

Sequence:

1. immediate notch response;
2. body morph begins;
3. 40-60 ms later, tabs/header/content begin subtle entrance;
4. content reaches final position before or with final shape settle.

### Navigation

Use shared selection pill plus directional page changes.

### Product role

Dashboard owns daily operational workflows:

- Home
- System overview
- Agents
- Tasks
- Apps/launcher entry

Full configuration should migrate toward Nexus.

### Performance

Do not regress current lazy pages or service refcounting.

### Done when

Dashboard is the strongest visual motion in the shell without delaying actual interaction.

## Phase 9. Right-side quick surfaces v2

Applies to Network, Audio and similar right-side quick panels.

### Shape

Use `RIGHT_POUR`.

The body should feel as though it hangs from or pours out of the right notch:

- connection edge remains fixed;
- vertical extension begins immediately;
- body widens left with slightly delayed trailing geometry;
- lower-left corner resolves later than the anchor edge;
- no large bounce.

This must look different from the center Dashboard bloom.

### UX

Quick panels remain quick.

Advanced configuration routes to Nexus.

### Navigation

Use shared tabs and directional content motion where a quick panel has multiple modes.

### Done when

A right-side panel visually belongs to the right bar control rather than looking like a detached generic panel.

## Phase 10. Left-side surfaces v2

Applies to ArchMenu, power and related left edge UI.

### Shape

Use `LEFT_SPILL`.

This should emphasize horizontal emergence more than RIGHT_POUR:

- a short horizontal extrusion establishes the body;
- top and bottom shoulders unfold after;
- final body settles with asymmetrical but controlled geometry.

### Done when

Left surfaces have a recognizable edge-spill character without mirroring RIGHT_POUR pixel-for-pixel.

## Phase 11. Notifications v2

Notifications should use card physics, not a giant liquid blob.

### New notification

- card appears from the right/top origin appropriate to the shell;
- stack makes room smoothly;
- card opacity and translation are coordinated.

### Dismiss drag

While dragging:

- card follows pointer directly;
- neighboring stack can begin reacting only when useful;
- release uses velocity-aware settle or dismissal.

### Reflow

Use near-critically-damped spring or smoothed movement for remaining cards.

### Expand/collapse

Height change and neighbor displacement occur together.

### Clear All

Optional 20-30 ms micro-stagger, total stagger <=100 ms.

### Done when

No notification insertion/removal causes unrelated cards to snap.

## Phase 12. Launcher v2

Launcher is a command surface. Perceived latency matters more than visual spectacle.

### Shape/motion

Use `LENS_REVEAL` or another light connected reveal, not the full Dashboard fluid morph.

### Requirements

- TextInput usable immediately.
- First keypress cannot be lost.
- Search results do not all animate on every keystroke.
- Shared result selection surface follows keyboard/mouse selection.
- Provider or mode changes can use small purposeful transitions.
- No process startup sits on the critical input path.

### Done when

Launcher feels faster than the current shell even if its complete visual reveal still takes around 150-190 ms.

## Phase 13. Nexus v2

Nexus is a management surface and should deliberately reject the more playful fluid motifs.

### Shape/motion

Use `QUIET_SHEET`:

- centered sheet;
- subtle opacity;
- approximately 0.985 -> 1 scale if retained;
- 180-220 ms;
- no visible bounce;
- backdrop animates separately.

### Navigation

- one moving active navigation background/marker;
- 8-12 px directional page travel;
- around 180 ms.

### Information architecture

Nexus becomes the full management/configuration home.

### Done when

Nexus feels calmer, denser and more deliberate than Dashboard while still sharing the same control system.

## Phase 14. OSDs, toasts and context menus

### OSDs

Use `CAPSULE`:

- short movement;
- shape can widen slightly as content changes;
- repeated volume/brightness updates change the current OSD rather than replaying entrance motion.

### Toasts

Use a compact surface reveal, coordinated with notification visual language.

### Context menus

Use `PIVOT_POP`:

- origin at pointer/invoker;
- 0.96-0.98 -> 1 scale;
- fast opacity;
- short 100-150 ms entrance;
- even faster exit.

Do not apply fluid bar morphs where there is no bar connection.

# MILESTONE D - UI REDESIGN

## Phase 15. Top bar/notch visual redesign

The top bar is the persistent face of APEX.

### Improve

- notch proportions;
- active/hover surfaces;
- spacing between icon/text groups;
- consistent icon baseline;
- dynamic-width behavior;
- selected/active control treatment;
- subtle depth separation from wallpaper;
- focus mode transition.

### Shape rules

The three notches should be related but not visually identical modules.

- left: command/system affordance;
- center: primary status/hero origin;
- right: quick status and transient surface origin.

### Avoid

- excessive gradients;
- constant glow;
- over-bright outlines;
- decorative animation while idle.

## Phase 16. Common component visual refresh

Redesign shared components once, then reuse them.

Priority:

- buttons;
- icon buttons;
- switches;
- sliders;
- segmented/tab controls;
- list rows;
- cards;
- chips/status pills;
- text fields;
- search fields;
- tooltips;
- confirmation dialogs.

Each component should define:

- default;
- hover;
- pressed;
- selected;
- disabled;
- keyboard focus;
- danger/warning/success variants where applicable.

## Phase 17. Page-level visual redesign

Do a layout and hierarchy pass on:

- Dashboard Home
- Dashboard System
- Agents
- Tasks
- Launcher
- Network
- Notifications
- Nexus pages

Rules:

- fewer arbitrary boxes inside boxes;
- stronger section hierarchy;
- more consistent empty states;
- deliberate alignment grid;
- content density appropriate to desktop use;
- no mobile-card UI copied blindly onto a desktop.

## Phase 18. Color/material system v2

Expand current Matugen-driven palette into functional surface roles.

### Keep

- wallpaper-derived personality;
- fixed danger/warning/success colors where reliability matters.

### Add

Derived surface levels with predictable contrast.

Possible roles:

```text
surface0 / base
surface1 / raised
surface2 / overlay
surface3 / strong selected
accentContainer
onAccentContainer
outlineSoft
outlineStrong
```

### Blur/transparency

Use only where it materially improves separation.

Do not make every panel translucent.

If transparency is added:

- preserve text contrast;
- preserve GPU budget;
- keep fallback solid surfaces;
- avoid animated blur radius unless proven safe.

# MILESTONE E - SYSTEM-WIDE UX

## Phase 19. Information architecture cleanup

Use four surface roles:

### Glance

- OSD
- small status
- passive toast

### Quick Action

- Network
- Audio
- Notifications
- Power

### Command

- Launcher
- Search
- Clipboard command flow

### Management

- Nexus

Rules:

- one primary home for each advanced setting;
- quick surfaces link to Nexus for advanced configuration;
- Dashboard is operational, not a second settings app;
- duplicate functionality is removed or intentionally mirrored with a documented reason.

## Phase 20. Hyprland motion unification

Do this with the planned compositor configuration modernization.

Separate compositor motion classes for:

- windows in;
- windows out;
- window move/resize;
- workspaces;
- fades;
- layers;
- border changes where useful.

### Motion goals

- opening: fast initial response, decelerated settle;
- closing: shorter and more accelerated;
- move/resize: direct;
- workspace keyboard switches: directional and quick;
- touchpad workspace gesture: direct while fingers are down, physics only on release.

Avoid double-animation of APEX layer surfaces when the shell already owns their geometry motion.

## Phase 21. Accessibility and keyboard UX

### Required

- complete Reduced Motion;
- logical Tab/Shift-Tab order;
- arrow-key navigation in tab/list controls;
- Enter/Space activation;
- Escape for transient layers;
- focus restoration;
- static visual state cues that do not rely on animation;
- usable contrast in selected/disabled/focus states;
- generous hit targets.

### Test Reduced Motion as its own product mode

Do not just turn timings to zero and assume it works.

# MILESTONE F - PERFORMANCE AND HARDENING

## Phase 22. Frame pacing and rendering optimization

### Frame budgets

| Refresh rate | Frame budget |
| --- | ---: |
| 60 Hz | 16.67 ms |
| 120 Hz | 8.33 ms |
| 144 Hz | 6.94 ms |

### Profile

- GUI thread;
- render thread;
- frame misses;
- binding evaluations;
- layout passes;
- texture uploads;
- effects/shaders;
- first-open latency;
- warm-open latency;
- input-to-visible-response latency.

### Rules

- no process forks in animation-critical paths;
- preserve lazy loading and service refcounting;
- prefer stable content layout under animated outer mask;
- use transforms/opacity for content when possible;
- do not animate expensive effects without measurement;
- cache static visual sources where useful.

## Phase 23. Motion and shape quality QA

Every fluid shape must be inspected in slow motion.

Reject:

- neck kinks;
- sudden radius changes;
- body self-intersection;
- one-frame shape collapse;
- anchor drift;
- visible mask mismatch;
- aliasing changes during scaling;
- rubbery overshoot;
- content clipping at intermediate frames;
- frame hitches at shape-control-point changes.

## Phase 24. Stress matrix

Test:

- repeated popup open/close spam;
- transition reversal at arbitrary progress;
- tab switch during popup opening;
- immediate Launcher typing;
- held volume/brightness keys;
- notification arrival/removal/drag/expand combinations;
- mixed refresh monitors;
- different resolutions/scales;
- monitor disconnect while a surface is open;
- fullscreen games;
- focus mode;
- lock while popup/dashboard/Nexus is open;
- Hyprland and supported alternate compositor behavior.

# 25. Suggested PR sequence

1. Baseline tooling and motion inventory
2. Motion tokens and CI duration lint
3. Surface/color/spacing/type tokens
4. Shared interaction primitives
5. Fluid shape model and renderer prototype
6. CENTER_BLOOM prototype
7. RIGHT_POUR + LEFT_SPILL prototypes
8. Surface lifecycle state machine
9. Shared selection/tab/page transition system
10. Dashboard v2
11. Network/Audio right-side surfaces
12. Left-side ArchMenu/power surfaces
13. Notifications stack physics
14. Launcher v2
15. Nexus v2
16. OSD/context/toast motion
17. Top bar visual redesign
18. Shared component visual refresh
19. Page-level visual redesign
20. Information architecture cleanup
21. Hyprland motion unification
22. Performance profiling fixes
23. Accessibility pass
24. Stress QA and release hardening

Do not combine all of these into one huge rewrite branch.

# 26. Milestone release gates

## Gate A - Foundation

- central motion tokens exist;
- visual tokens exist;
- common controls use shared interaction behavior;
- Reduced Motion architecture is central.

## Gate B - Fluid engine

- at least three shape families are clean and interruptible;
- no large surface depends on guessed close timing;
- shape rendering passes visual and performance checks.

## Gate C - Core surfaces

- Dashboard, right quick surfaces, left surfaces, notifications, Launcher and Nexus use their intended motion families;
- page navigation is connected;
- immediate input behavior is preserved.

## Gate D - UI redesign

- common components have consistent state visuals;
- top bar and primary pages use the new hierarchy;
- color/material roles are consistent.

## Gate E - System UX

- duplicate settings/workflow responsibilities are cleaned up;
- compositor and shell motion feel related;
- keyboard and accessibility behavior are coherent.

## Gate F - Release

- primary interactions remain within practical frame budgets on target hardware;
- no major transition jumps when reversed;
- no significant functional regression;
- no stale popup surfaces/input masks;
- Reduced Motion is complete;
- mixed refresh and multi-monitor behavior are stable.

# 27. Agent execution contract

An implementation agent following this roadmap should obey these rules:

1. Do not begin page-specific visual changes before the shared token/control work for that component type exists.
2. Do not replace every Canvas shape at once. Keep the old shape renderer until new families are proven.
3. Do not add a new hardcoded animation duration when a semantic token fits.
4. Do not make a surface more animated merely to make it different. Difference comes from origin, role, silhouette and sequencing.
5. Do not block input until animation completion.
6. Do not add process spawning or heavy synchronous I/O to interaction-critical paths.
7. Do not regress lazy loading, service refcounting or fullscreen unmapping.
8. Do not use visible bounce as the default meaning of "spring".
9. Do not assume a change is smoother because it has a higher animation duration.
10. For every phase, add or update a test, benchmark or visual QA artifact that makes regression detectable.
11. Preserve compositor fallback behavior where practical.
12. Keep each PR narrow enough that shape, motion, UX and performance regressions can be bisected.

# 28. Definition of done

The project is complete when all of the following are true:

- APEX has one central motion language.
- APEX has a documented visual design token system.
- Large connected surfaces use distinct shape families appropriate to their location and role.
- Dashboard, right quick surfaces and left quick surfaces no longer look like the same rounded panel animation.
- Fluid geometry has clean tangents and no visible shape artifacts.
- Popup lifecycle is progress/state-driven.
- Animations reverse cleanly.
- Shared selection indicators move between tabs/navigation rows.
- Notifications reflow physically.
- Launcher accepts input immediately.
- Nexus remains calmer than Dashboard by design.
- OSDs update in place rather than replaying entrance motion.
- Common controls have consistent hover, press, focus, selected and disabled states.
- Surface hierarchy, typography, spacing and icon treatment are visibly more coherent.
- Quick actions and management surfaces have clear responsibilities.
- Reduced Motion works across the entire shell.
- Hyprland motion does not feel disconnected from shell motion.
- Primary interactions meet practical frame pacing targets on target hardware.
- Multi-monitor, mixed-refresh, fullscreen and lock workflows remain reliable.

# 29. Final design principle

APEX should not feel like a collection of widgets with animation added to them.

It should feel like a desktop made from a small number of related physical materials and surface behaviors. The center of the bar blooms into a workspace. The right side pours into quick controls. The left side spills into system actions. Notifications rearrange as a stack. Command surfaces reveal quickly. Management surfaces remain stable. Controls answer immediately.

The user should notice that APEX feels better long before they notice the individual animations that made it better.
