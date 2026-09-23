---
title: "APEX Shell Fluid + Visual Design Roadmap"
subtitle: "Shape language, animation motifs, visual system and per-surface UI specification"
version: "1.0"
date: "2026-09-20"
status: "Design specification before implementation"
companion: "APEX-Shell-Master-UIUX-Roadmap-v3.md"
---

# APEX Shell Fluid + Visual Design Roadmap v1

## 0. Why this document exists

The previous roadmap correctly addressed timing, interruption, lifecycle and interaction latency, but that is not enough.

A shell can be perfectly smooth and still look generic.

APEX needs its own visual and motion character. The persistent notches, connected popups and dark Matugen-driven surface already give it a base identity. The next pass should turn that identity into a deliberate system with several related fluid behaviors instead of one rounded-rectangle treatment reused everywhere.

This document defines what those shapes and interfaces should look and feel like.

# 1. Design character

APEX should feel:

- compact;
- technical;
- fluid where surfaces physically connect;
- calm where the user is managing settings;
- responsive rather than floaty;
- dark and high-contrast without becoming black-on-black;
- dynamic without constant visual noise;
- clearly desktop-oriented, not a phone UI stretched across a monitor.

APEX should avoid:

- jelly animation on every object;
- huge Material cards everywhere;
- excessive acrylic/blur;
- random gradients;
- neon outlines around every surface;
- every popup using the same shape;
- visible overshoot that makes the interface look elastic;
- long stagger animations;
- controls that are too tiny to target;
- walls of identical boxes.

# 2. Shape language overview

APEX gets eight motion/shape families.

| ID | Family | Primary use | Character |
| --- | --- | --- | --- |
| `CENTER_BLOOM` | Center bloom | Dashboard | Symmetric, connected, expressive |
| `RIGHT_POUR` | Right pour | Network, Audio, right quick panels | Anchored, viscous, downward/leftward |
| `LEFT_SPILL` | Left spill | ArchMenu, Power, left surfaces | Horizontal emergence, asymmetric |
| `STACK_REFLOW` | Stack reflow | Notifications | Physical card motion, not blob morph |
| `LENS_REVEAL` | Lens reveal | Launcher/search | Fast, focused, lightweight |
| `QUIET_SHEET` | Quiet sheet | Nexus/settings | Stable, restrained, minimal morph |
| `CAPSULE` | Capsule | OSD/status | Compact, quick, soft |
| `PIVOT_POP` | Pivot pop | Context menus | Pointer-origin scale/fade |

These should look related through shared radii, colors, acceleration and material, but not identical.

# 3. Fluid geometry engine

## 3.1 Current limitation

The current `PopupShape.qml` is a `Canvas` with fixed branches such as `top`, `left`, `right`, `pill-right`, `bottom` and `bottom-right`. The paths use quadratic curves and rounded corners.

This is good for static joins. It is not enough for expressive shape morphs because the path vocabulary is fixed.

## 3.2 Target architecture

Introduce a parametric geometry model.

Possible conceptual structure:

```text
FluidSurface.qml
  owns state, progress and content clip

FluidShapeModel.qml
  produces normalized path parameters

FluidShapeRenderer.qml
  renders cubic Bezier geometry

FluidMorph.qml
  maps progress, direction and family to shape parameters
```

The implementation may combine these if QML ergonomics demand it, but keep the responsibilities separate.

## 3.3 Rendering approach

Evaluate `QtQuick.Shapes` first.

Why:

- `ShapePath` and cubic path segments are better suited to animated control points than repainting a large Canvas path by hand;
- geometry can remain declarative;
- cubic curves give finer tangent control than the current quadratic-only flare approach.

Keep Canvas as a fallback/reference until the new renderer is proven.

A signed-distance-field or metaball shader may be explored later for extreme fluidity, but it should not be the baseline because it adds rendering and maintenance complexity.

## 3.4 Normalized geometry

Define shape geometry in a normalized 0..1 coordinate space where possible, then map it into actual surface dimensions.

Important inputs:

```text
progress
anchorEdge
anchorCenter
anchorWidth
anchorHeight
targetWidth
targetHeight
neckWidth
neckDepth
shoulderTension
bodyRoundness
leadingBias
trailingLag
asymmetry
cornerStart
cornerEnd
```

Avoid directly hand-animating dozens of absolute pixels in individual popup files.

## 3.5 Continuity rules

Every connected fluid shape must satisfy these visual requirements:

- The surface keeps contact with its origin throughout the transition.
- Curves leave the bar with matching or deliberately controlled tangent direction.
- The neck does not visibly kink.
- Concave shoulders do not collapse into points.
- The body does not self-intersect.
- Corners do not suddenly switch radius.
- The body cannot briefly become thinner than its content-safe minimum.
- Shape anti-aliasing must remain stable during the morph.

The target should be visually close to C1 continuity at joins, even if the renderer does not expose a formal continuity constraint system.

# 4. CENTER_BLOOM

## 4.1 Purpose

Used for Dashboard and only a small number of major center-notch transformations.

This is the signature APEX motion.

## 4.2 Starting silhouette

At progress 0:

- center notch width equals its normal bar state;
- height is normal notch height;
- shoulders are the normal top-bar concave transitions;
- no separate panel is visible.

## 4.3 Morph behavior

The first part of the movement should widen more quickly than it deepens.

Suggested qualitative sequence:

### 0-20%

- immediate width response;
- neck remains visually tight;
- bottom edge begins moving down;
- shoulder curvature becomes slightly more pronounced.

### 20-60%

- body depth grows strongly;
- shoulders open into a wider concave neck;
- lower corners start becoming visible;
- target width is mostly established.

### 60-100%

- body reaches target depth;
- neck settles into final connected shape;
- corner radii settle;
- no rubber rebound.

## 4.4 Shape personality

Think "bloom" rather than "balloon".

Good:

- confident expansion;
- soft shoulder transition;
- visually heavy body anchored to a narrow origin.

Bad:

- body wobbling;
- vertical stretch before width;
- big overshoot;
- pill simply scaling into a rectangle.

## 4.5 Content choreography

The body leads.

At roughly 40-60 ms after the morph begins:

- tab/header opacity starts;
- content enters from 8-14 px below;
- content should already use its target-width layout.

Do not make the content squeeze through every intermediate panel width.

## 4.6 Closing

Closing is not just the entrance reversed in time.

- content leaves first;
- body collapses faster;
- neck stays attached until the final frame;
- surface unmaps on actual completion.

# 5. RIGHT_POUR

## 5.1 Purpose

Used for right-side quick action surfaces such as:

- Network;
- Audio;
- Bluetooth/VPN/Hotspot containers where part of the Network surface;
- similar future right-side controls.

## 5.2 Character

This should look as if the right notch is extending downward and then allowing a body to pour left into the screen.

It should not be a mirrored Dashboard.

## 5.3 Morph sequence

### 0-25%

- right anchor edge remains fixed;
- a short vertical stem extends downward;
- width expands only slightly.

### 25-70%

- body widens leftward;
- top-left neck transitions from notch connection into body;
- lower-left corner trails slightly behind the right edge.

### 70-100%

- left edge and lower-left corner settle;
- height reaches final value;
- content finishes its small entrance.

## 5.4 Shape details

Use asymmetry intentionally.

- right edge can remain close to straight and screen-aligned;
- left edge does more of the expressive movement;
- upper connection is visually tighter than lower body;
- final panel may use a slightly different lower corner radius from Dashboard while remaining within the same radius system.

## 5.5 Timing

Keep this faster than Dashboard.

Suggested Balanced target:

```text
enter: 180-210 ms
exit: 125-150 ms
```

# 6. LEFT_SPILL

## 6.1 Purpose

Used for:

- ArchMenu;
- power/session controls;
- left-edge shell tools.

## 6.2 Character

The left side should feel like a surface pushing horizontally out from the bar/edge, then unfolding vertically.

This differentiates it from RIGHT_POUR.

## 6.3 Morph sequence

### 0-30%

- horizontal extrusion establishes body width;
- connection to the left bar remains obvious.

### 30-75%

- body height opens;
- top and bottom shoulder geometry separates;
- right edge becomes the leading visible body edge.

### 75-100%

- corners and neck settle;
- content completes entrance.

## 6.4 Avoid literal mirroring

Do not implement RIGHT_POUR and simply flip x coordinates.

The left side is more command/system oriented. Its motion can feel firmer and slightly more horizontal.

# 7. STACK_REFLOW

## 7.1 Purpose

Notifications.

Notifications should not use a large liquid panel animation for every state change. Their defining motion is the relationship between cards.

## 7.2 New card

A new card:

- appears from the shell edge/origin;
- moves a short distance;
- fades quickly;
- causes existing cards to move out of the way using smooth physical reflow.

## 7.3 Drag dismiss

While dragged, the card follows input directly.

Optional supporting feedback:

- opacity falls slightly as it crosses dismiss threshold;
- neighbor spacing responds subtly;
- threshold can be indicated by a small background/action cue.

On release:

- dismiss with release velocity if threshold passed;
- spring/smooth back into place otherwise.

## 7.4 Stack reflow

Use a spring or velocity-aware smoothed animation with very low/no visible oscillation.

The reflow should make cards feel like objects occupying space, not list indices being reassigned.

# 8. LENS_REVEAL

## 8.1 Purpose

Launcher and focused command/search experiences.

## 8.2 Character

Fast, concentrated and lightweight.

A command surface should not use the same slow physical mass as Dashboard.

## 8.3 Visual idea

A narrow search/lens region establishes itself first, then the result region reveals behind it.

Possible sequence:

- search shell appears with small scale/opacity movement;
- input is active immediately;
- results clip/reveal downward or outward over 100-160 ms;
- selected result surface is already visible once results exist.

## 8.4 No keyboard delay

The animation is purely visual. Search focus and typing begin immediately.

# 9. QUIET_SHEET

## 9.1 Purpose

Nexus/settings and other long-lived work surfaces.

## 9.2 Character

Stable and professional.

No liquid connection is required because Nexus is not conceptually spilling out of a bar control.

## 9.3 Motion

- subtle fade;
- optional scale 0.985 -> 1;
- backdrop fades separately;
- around 180-220 ms Balanced;
- no visible overshoot.

Page navigation inside Nexus can still use connected selection and small directional motion.

# 10. CAPSULE

## 10.1 Purpose

- volume;
- brightness;
- microphone;
- recording state;
- passive status indicators.

## 10.2 Shape

Compact pill/capsule with enough internal padding to handle icon + value.

It may widen slightly as content changes, but should not perform a complex blob morph.

## 10.3 Repeated input

The OSD stays present while values are repeatedly updated.

Only the value/indicator moves. Entrance animation is not replayed for every keypress.

# 11. PIVOT_POP

## 11.1 Purpose

Context menus and small pointer/invoker anchored menus.

## 11.2 Motion

- transform origin near invoker;
- scale around 0.96-0.98 -> 1;
- opacity 0 -> 1;
- 100-150 ms entrance;
- faster exit.

No bar fluidity should be invented when there is no connected bar origin.

# 12. Surface material system

The current `Theme.background` approach is too limited for a full visual hierarchy.

Create derived roles rather than dozens of hardcoded RGBA overlays.

## 12.1 Suggested roles

```text
surfaceBase
surfaceRaised
surfaceOverlay
surfaceHigh
surfaceSelected
surfaceHover
surfacePressed
outlineSoft
outlineStrong
accent
accentContainer
onAccent
onAccentContainer
```

## 12.2 Matugen integration

Keep wallpaper-derived color identity.

Derive stable surface roles from the loaded palette so a wallpaper change changes the personality without destroying hierarchy.

## 12.3 Fixed status colors

Keep danger/warning/success as reliable semantic colors where current APEX intentionally does so.

Do not let a red wallpaper make danger state invisible.

# 13. Depth and separation

APEX should use restrained depth.

Use a small number of techniques:

- surface tone difference;
- soft outline;
- small shadow/elevation;
- limited blur/transparency;
- spacing.

Do not stack all of them on every component.

## 13.1 Blur policy

Blur can help large overlays separate from wallpaper, but it should be selective.

Possible use:

- Nexus backdrop;
- a large hero surface if benchmarking proves it safe;
- lock/greeter.

Avoid:

- every list card blurred individually;
- animated blur radius;
- heavy multi-pass blur under frequently moving content.

# 14. Typography

APEX needs a clearer type ladder.

Suggested semantic levels:

| Role | Use |
| --- | --- |
| Caption | tertiary labels, metadata |
| Body small | compact controls |
| Body | standard text |
| Body strong | emphasized values/actions |
| Section | local section headings |
| Heading | popup/page headings |
| Page title | Nexus/Dashboard primary title |
| Display | rare hero text |
| Mono | technical values/code-like data |

Rules:

- page title and section heading should not be visually interchangeable;
- numeric telemetry may use a dedicated number/mono treatment where useful;
- avoid bolding every selected row;
- use weight and contrast intentionally.

# 15. Spacing and layout

Use a central spacing scale:

```text
4 / 8 / 12 / 16 / 24 / 32
```

Prefer alignment over extra containers.

Rules:

- repeated rows align icons, titles, values and actions to stable columns;
- cards should not each invent their own internal padding;
- major surfaces should have a clear page grid;
- dense desktop information is allowed, but groups need breathing room;
- avoid mobile-like single-column cards when desktop space can show related data more efficiently.

# 16. Radius system

Use semantic radii.

Suggested relationship:

```text
XS: tiny controls / tags
S: small buttons / inputs
M: normal controls / list selection
L: cards / quick panels
XL: hero surfaces
Full: pills / circular controls
```

Fluid surfaces can interpolate between radius roles during morph, but the final state should land on the normal radius system.

# 17. Top bar redesign

The top bar should become cleaner and more intentional before adding more visual complexity elsewhere.

## 17.1 Left notch

Role:

- system/command affordance;
- power/session entry;
- layout/workspace or related shell controls.

Visual character:

- slightly firmer geometry;
- compact grouping;
- clear interactive region.

## 17.2 Center notch

Role:

- primary APEX identity;
- time/media/active state;
- Dashboard origin.

Visual character:

- cleanest silhouette;
- enough negative space that CENTER_BLOOM reads clearly when activated;
- no constant decorative animation.

## 17.3 Right notch

Role:

- status;
- audio/network/battery;
- notifications;
- right quick surface origin.

Visual character:

- modular status groups;
- selected control state clearly explains which quick panel is open;
- should visually tolerate width changes without content jitter.

# 18. Buttons

APEX buttons should feel physical without becoming playful.

## Default

- clear but low-contrast surface;
- readable label/icon;
- soft outline only where useful.

## Hover

- surface brightens or shifts one level;
- icon/text gains contrast;
- no big size change.

## Pressed

- approximately 0.975 scale;
- stronger fill;
- immediate response.

## Selected/toggled

- accent container or clear selected surface;
- selected state must remain obvious without relying on motion.

## Disabled

- reduced contrast;
- no hover cursor;
- no press animation.

# 19. Switches and toggles

The toggle thumb/selection element should be one physical object.

Use:

- fast spatial movement;
- coordinated track color change;
- no obvious bounce;
- keyboard focus ring outside the control.

# 20. Sliders

## Track

- clear inactive/active separation;
- reasonable thickness;
- enough contrast at all Matugen palettes.

## Thumb

- slightly larger on hover/drag only if useful;
- direct pointer tracking;
- no easing while dragging.

## Programmatic value change

Use smoothed target tracking rather than repeatedly starting fixed-duration animations.

# 21. Selection pills and tabs

A selection surface should be a shared object.

Do not make each tab own a competing active background.

The selection object can adjust width as labels differ, but should keep stable height and radius.

Text/icon color may begin changing slightly before the pill finishes moving so the state feels responsive.

# 22. Lists and rows

Rows need a coherent grid.

Standardize:

- icon column;
- title/subtitle block;
- trailing value;
- trailing action;
- selected background;
- hover background;
- focus state.

Avoid each settings row becoming its own bordered card unless it represents a genuinely distinct object.

# 23. Dashboard visual redesign

## 23.1 Home

Should feel like an overview, not a widget wall.

Use larger grouped regions rather than many tiny disconnected cards.

Possible hierarchy:

- primary status / current context;
- active media / current task;
- quick system health;
- recent/important actions.

## 23.2 System

Use denser desktop layout.

- aligned telemetry;
- charts only where they answer a question;
- avoid multiple gauges competing for attention;
- warning states become obvious through semantic color and hierarchy.

## 23.3 Agents

Make states visually scannable:

- active;
- waiting;
- needs user;
- finished/error.

Use status surface/icon treatment rather than constant pulse animations.

## 23.4 Tasks

Kanban can retain cards, but spacing, actions and drag feedback should use the same component system as the rest of APEX.

## 23.5 Apps

Launcher/search area should look like a command surface, not another Dashboard widget page.

# 24. Network visual redesign

The Network quick surface should be optimized for common decisions.

Top area:

- current connectivity state;
- Wi-Fi toggle;
- active network;
- compact status.

Main area:

- network list;
- strong selected/current network state;
- connection progress/error feedback.

Bottom:

- tabs or mode switch for Wi-Fi/Bluetooth/VPN/Hotspot where retained;
- advanced settings link to Nexus.

Avoid an overly tall empty panel when lists are short.

# 25. Notifications visual redesign

Cards should have:

- clear app/source identity;
- title hierarchy;
- body text;
- time/metadata;
- actions when available;
- expand affordance only when needed.

The notification center should feel like one stack area, not a collection of unrelated boxes.

Unread/urgent state can use a small accent marker or stronger surface level rather than glowing borders.

# 26. Launcher visual redesign

The launcher should have the cleanest command hierarchy in the shell.

Recommended structure:

1. search field;
2. current mode/provider hint only if useful;
3. result list/grid;
4. optional action hint/footer.

Keep UI chrome minimal.

Selection should be strong enough to follow by keyboard at a glance.

# 27. Nexus visual redesign

Nexus should use a denser desktop settings language.

Left navigation:

- one shared selected surface;
- concise labels;
- clear category grouping if page count grows.

Content:

- page title and subtitle;
- sections with consistent spacing;
- controls aligned to a stable column/grid;
- advanced options visually secondary;
- avoid every settings row becoming a floating card.

# 28. Loading, empty and error states

Define these once.

## Loading

Use subtle skeleton/progress where latency is real.

Do not use decorative spinners for operations that normally complete instantly.

## Empty

Use:

- one clear icon/illustration treatment;
- one sentence explaining state;
- one primary action if useful.

## Error

Use semantic danger treatment and a recovery action.

Avoid modal dialogs for recoverable inline errors.

# 29. Micro-motion rules

## Good micro-motion

- press compression;
- active marker movement;
- toggle thumb movement;
- small icon state change;
- height change that prevents a layout jump;
- opacity supporting a spatial transition.

## Bad micro-motion

- every icon floating on hover;
- every card scaling on hover;
- text sliding for ordinary state changes;
- endless ambient pulses;
- three separate animated effects for one click.

# 30. Shape-motion quality rules

Every fluid animation should be reviewed against this list.

### Origin

Can the user tell where the surface came from?

### Silhouette

Does the intermediate shape look intentional, not mathematically accidental?

### Continuity

Are joins smooth with no kinks?

### Weight

Does a large surface move with more mass than a small menu without feeling slow?

### Interruption

Can it reverse from any point without snapping?

### Content relationship

Does content feel carried by the surface rather than separately fading on top of it?

### Contrast

Is the shape still clear over light and dark wallpapers?

### Reduced Motion

Does the same feature remain understandable with the spatial motion removed?

# 31. Performance rules for fluid rendering

Fluidity is not worth dropped frames.

Measure each family independently.

Prefer:

- a small, stable number of cubic path segments;
- transform/opacity for content;
- cached static layers;
- one outer shape renderer per surface.

Avoid:

- per-frame object creation;
- high-frequency JS allocation;
- rebuilding large child trees during shape changes;
- multiple large blurred layers;
- animated high-cost shaders without profiling.

# 32. Prototype order

Do not redesign every surface simultaneously.

Prototype in this order:

1. Shared motion tokens
2. Shared visual tokens
3. Static parametric fluid shape renderer
4. CENTER_BLOOM path prototype with no real content
5. Interrupt/reverse CENTER_BLOOM
6. RIGHT_POUR
7. LEFT_SPILL
8. Stable-content clipping architecture
9. Dashboard integration
10. Network integration
11. Notification reflow
12. Launcher lens reveal
13. Nexus quiet sheet
14. Component visual refresh
15. Page-level redesign

# 33. Visual prototype gate

Before integrating a new shape family into production UI, create a dedicated prototype harness showing:

- open;
- close;
- reverse at 25%;
- reverse at 50%;
- reverse at 75%;
- resize target while open;
- Reduced Motion;
- 0.85x / 1.0x / 1.5x UI scale;
- dark/light wallpaper extremes;
- 60 and high refresh rate capture.

The harness should make bad intermediate geometry obvious before it is buried inside a real popup.

# 34. Visual acceptance scorecard

Score each new primary surface from 1-5 on:

| Category | Question |
| --- | --- |
| Identity | Does it look like APEX rather than a generic Qt panel? |
| Origin | Is its connection to the invoking control obvious? |
| Shape quality | Are intermediate silhouettes clean? |
| Motion quality | Does it react immediately and settle naturally? |
| Hierarchy | Is the important content obvious first? |
| Density | Is it desktop-appropriate rather than cramped or oversized? |
| Consistency | Does it use shared controls/tokens? |
| Accessibility | Does it remain clear with Reduced Motion? |
| Performance | Does it hold frame pacing on target hardware? |

A primary surface should not ship with any category below 3. The target for major APEX surfaces is 4+ in every category.

# 35. Final visual target

The strongest APEX identity should come from three things working together:

1. **Connected geometry** - important surfaces actually grow out of the shell regions that own them.
2. **Different physical roles** - Dashboard blooms, right quick controls pour, left system controls spill, notifications reflow, Launcher reveals, Nexus stays calm.
3. **A disciplined UI system** - typography, spacing, controls, surfaces, icons and colors feel related everywhere.

The result should look more advanced without looking busier.

When APEX is idle, the desktop should remain quiet. When the user acts, the shell should answer immediately and move with enough character that the interface feels designed rather than assembled.
