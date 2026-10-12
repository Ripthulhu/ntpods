# Development

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the code layout, build commands and
tests. Keep changes within the existing modules where possible. Comments should
explain constraints and reasons that are not clear from the code.

# Interface

Preserve the existing LibrePods Apple design when fixing bugs or adding features.
Extend existing components; a broad restyle requires an explicit request.
Keep the listening slider's glass appearance, input batching and resource reuse.

Read [DESIGN.md](DESIGN.md) before UI work and inspect the affected controls in
`windows/winui/NTPods.WinUI`. Reuse the shared styles and theme resources in
`App.xaml`. The current interface and the rules below take precedence over
older design notes if they disagree.

- Keep the LibrePods Apple appearance: one focused, centered settings column,
  balanced side margins, rounded inset groups, restrained dividers, proportional
  AirPods artwork, green switches and battery rings, and blue control accents.
  Preserve both light and dark appearances.
- Keep the layout quiet. Connection status belongs in the device header; battery
  information belongs with its artwork. Put routine pending, success and
  unavailable explanations in tooltips or accessibility help. Show actionable
  errors where needed. Do not restore the removed announcement bar or add debug
  messages, technical labels, or repeated status lines to the main page.
- Preserve the continuous listening-mode gesture and the integrated 0–100
  Adaptive slider. Keep icons loaded during gestures so they do not flash.
  Delayed device reports must not interrupt dragging. Show Off only when the
  model and firmware support it: legacy ANC models have Off directly; newer Pro
  models use their reported or saved Allow Off setting. Hide Off while Adaptive
  occupies the expanded region, and omit Adaptive on models that lack it.
- Preserve the frameless black Dynamic Island on the primary monitor, its
  artwork, independent battery readouts, rounded shape and staged motion.
  Do not replace it with a rectangular notification window or Windows toast.
  It must not take focus. Routine battery updates must not restart dismissal,
  and Show batteries must not reconnect audio.
- Keep known device artwork visible during nearby charging and missing model
  reports. Retain last-known battery percentages with their original age, dim
  stale readings, and hide stale charging indicators. An unavailable new reading
  must not erase a known case percentage.
  Dim an explicitly unavailable bud even when its partner remains connected.
  Reading age alone must not dim artwork for a connected, available bud.
  Keep cached artwork and battery data tied to the paired device identity. Use
  `windows/device-models.json` for model names and capabilities in both the
  backend and UI; unknown models must not inherit Pro 3 sensor or microphone support.
  When exact artwork is missing, reuse the closest bundled earbud or headset
  artwork instead of placeholder icons. Artwork reuse must never
  change the device's reported name or supported controls.
- Keep motion smooth and interruptible. Use compositor animations or rendering
  callbacks while animating; stop frame work when idle. Preserve the artwork
  fades, subtle app entrance, centered charging icons, and scrollbar that fades
  quickly after scrolling stops. Respect Windows reduced-motion preferences.
- New controls must fit the existing groups, spacing, typography and concise
  wording. Preserve native keyboard behavior, visible focus, accessible names,
  high contrast, and localized text without clipped labels or resource-key text.
  Submenu headers share the existing 16-DIP normal text style; section captions
  and supporting text retain their smaller sizes.

For visual changes, compare the affected view before and after in its relevant
states, including light/dark and a narrow window. Check animation interruptions,
cached data and missing data when they are affected. Keep unrelated styling unchanged.

# Writing

Use plain, direct language in documentation, UI text and comments. Cut filler,
repeated disclaimers and explanations of obvious details. Keep technical facts,
useful warnings and licensing notices. Project instructions should state the
rules without conversation quotes, approval anecdotes or assistant work logs.

# Before every commit

Apply the `ponytail:ponytail` and `no-ai-slop` skills before committing to any
branch or remote in this repository.

- Use Ponytail to review the code, callers, tests and configuration. Prefer the
  smallest complete change; preserve validation, cleanup and error handling.
- Use no-ai-slop to review changed documentation, comments, UI text and the
  proposed commit message. Preserve technical facts, uncertainty and licensing
  notices. Read its `eval.md` and fix any failed checks.
- Fix review findings and run the checks needed for those fixes before committing.
  Report remaining hardware or release blockers without calling them passes.
- If either skill is unavailable, report that before committing.

The working fork is `https://github.com/Ripthulhu/ntpods`; the upstream project is
`https://github.com/arctumn/ntpods`. Keep recordings, local hardware identifiers,
private certificates and generated build artifacts out of commits.
