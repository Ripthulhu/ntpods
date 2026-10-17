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

After pairing, show Paired and fresh readings from the exact paired device:
AirPods and case, or one headphone reading. Unknown or stale parts show an em
dash. Keep the card until Continue, close or lid-close; do not expire the
successful state.

Reference: LibrePods rewrite 62ef3358, PopupWindow.kt and PairingArtwork.kt.
Use DESIGN.md for sizes and discovery behavior. Check light, dark, high
contrast, reduced motion, long names and narrow work areas.
