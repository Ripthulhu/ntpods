---
version: 1
slug: "windows-winui-ntpods-winui-popup-connectpromptview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/ConnectPromptView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/ConnectPromptWindow.cs", "windows/winui/NTPods.WinUI/MainWindow.Pairing.cs"]
---

# AirPods pairing card

Mode: Operate. Show one white card at the bottom of the primary monitor with
dark text and a blue action in both app themes. High contrast uses Windows
system colors. Keep the title, bundled model artwork, status and action in the
same card through nearby, setup, connecting, retry and paired states.
Show the pairing gesture and "Then tap Connect" on the first nearby card,
before discovery starts. Use the advertised model when known; keep the generic
gesture for a name-only Classic invitation.

Slide the complete card as one sheet. A new lid-open invitation can reverse its
exit; reduced motion shows or hides it immediately. Automatic invitations do
not take focus. Keep the close action and accessible status in every state.
Keep a model-identified lid invitation open until the case closes or the user
dismisses it; waiting must not replace it with a generic discovery card.

Known Pro models reuse LibrePods' silent Pro-style video and matching poster,
preserving its frame rate and aspect ratio in a fitted viewport, without player
letterboxing or still artwork showing behind it. Other models, reduced motion, high
contrast and playback errors keep model-specific still artwork. Pause the
player at dismissal and release it after hide; never register media controls.
Show the decoded matching poster from the first visible frame. Keep it over
the video until the card entrance finishes and playback advances, then fade
it away over 120 ms. Repeated updates for the same model retain the player.
Crossfade internal content and status text over 350 ms without restarting on
identical updates. Keep only the latest queued status and expose only current
content to input and accessibility. These transitions do not replace the outer
card slide.

After pairing, show Paired and fresh readings from the exact paired device:
AirPods and case, or one headphone reading. Unknown or stale parts show an em
dash. Keep the card until Continue, close or lid-close; do not expire the
successful state.

Reference: LibrePods rewrite 62ef3358 for the card; 3303ba76 for
PairingArtwork.kt, the trimmed loop and its poster. See THIRD_PARTY.md.
Use DESIGN.md for sizes and discovery behavior. Check light, dark, high
contrast, reduced motion, long names and narrow work areas.
