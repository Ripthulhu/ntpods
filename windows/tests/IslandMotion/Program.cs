using NTPods.WinUI.Popup;

static void Check(bool condition, string message)
{
    if (!condition) throw new Exception(message);
}

var motion = new IslandMotion();
Check(motion.PayloadOffset == 34, "The closed capsule must keep its payload below the 34-DIP clip.");
motion.Show(0, true);
motion.Advance(80);
double interrupted = motion.Expansion;
motion.Show(80, true);
Check(motion.Expansion == interrupted, "An incoming event must not jump an opening capsule.");
motion.Advance(400);
Check(motion.Expansion == 1 && !motion.Animating, "The capsule must finish opening.");
motion.Show(3000, true);
motion.Advance(6999);
Check(motion.Visible && motion.Expansion == 1, "Updates must restart the reading time.");
motion.Advance(7000);
motion.Advance(7100);
double closing = motion.Expansion;
Check(closing > 0 && closing < 1, "The capsule must retract before hiding.");
Check(motion.Closing && motion.PayloadScale < 1 && motion.PayloadOffset > 0,
    "The outgoing payload must fold together during exit.");
motion.Show(7100, true);
Check(motion.Expansion == closing, "A new event must reverse dismissal from its current size.");
Check(!motion.Closing, "Reopening must cancel the old exit before preparing new content.");
motion.Advance(7500);
Check(motion.Expansion == 1, "Interrupted dismissal must reopen fully.");
motion.Advance(11500);
motion.Advance(11720);
Check(!motion.Visible && motion.Expansion == 0, "Dismissal must finish and release the timer.");
Check(motion.PayloadOffset == 34, "Completed dismissal must move the payload below the closed clip again.");

motion.Show(12000, false);
Check(motion.Visible && motion.Expansion == 1 && !motion.Animating, "Reduced motion must show all content immediately.");
Check(motion.PayloadScale == 1 && motion.PayloadOffset == 0, "Reduced motion must leave the payload at its final layout.");
motion.Advance(16000);
Check(!motion.Visible, "Reduced motion must dismiss without a spatial animation.");
motion.Show(17000, true);
motion.Hide();
Check(!motion.Visible && motion.Expansion == 0, "Explicit hide must clear the transition.");

motion.Show(18000, true);
motion.SetAnimationsEnabled(18080, false);
Check(motion.Expansion == 1 && !motion.Animating, "Disabling animation during opening must reveal the complete message.");
motion.SetAnimationsEnabled(20000, true);
motion.SetAnimationsEnabled(21000, false);
Check(motion.DismissDelay(21000) == 1080, "Preference changes must preserve the existing dismissal deadline.");
motion.Advance(22080);
Check(!motion.Visible, "Repeated preference changes must not keep the popup open.");
motion.Show(23000, true);
motion.Advance(23420);
motion.Advance(27420);
motion.Advance(27480);
motion.SetAnimationsEnabled(27480, false);
Check(!motion.Visible, "Disabling animation during dismissal must hide immediately.");
motion.SetAnimationsEnabled(28000, true);
Check(!motion.Visible, "An animation preference change must never reopen a hidden popup.");
Console.WriteLine("Island motion checks passed.");

var timing = new IslandMotion();
timing.Show(0, true);
timing.Advance(64);
Check(Math.Abs(timing.Expansion - 0.5) < 0.00001,
    "The opening curve must match cubic-bezier(.2, 0, 0, 1).");
timing.Advance(319);
Check(timing.Expansion < 1 && timing.Animating, "The entrance must keep moving until 320 ms.");
timing.Advance(320);
Check(timing.Expansion == 1 && !timing.Animating, "The entrance must stop frame work at 320 ms.");
timing.Dismiss(320, true);
timing.Advance(364);
Check(Math.Abs(timing.Expansion - 0.5) < 0.00001, "Dismissal must use the same curve over 220 ms.");
timing.Show(364, true);
Check(Math.Abs(timing.Expansion - 0.5) < 0.00001, "Reopening must preserve the current clipped geometry.");
timing.SetAnimationsEnabled(364, false);
Check(timing.Expansion == 1 && !timing.Animating, "Reduced motion must finish an interrupted fold directly.");

var pairing = new IslandMotion(double.PositiveInfinity, openingMilliseconds: 420);
pairing.Show(0, true);
pairing.Advance(320);
Check(pairing.Animating, "The pairing card must preserve its separate 420 ms entrance.");
pairing.Advance(420);
pairing.Advance(60000);
Check(pairing.Visible && !pairing.Animating, "An interactive card must remain open while its own expiry is paused.");
pairing.Dismiss(60000, true);
Check(pairing.Expansion == 1 && pairing.Animating, "Explicit dismissal must start without jumping.");
pairing.Advance(60100);
double dismissed = pairing.Expansion;
Check(dismissed > 0 && dismissed < 1, "An interactive card must retract before hiding.");
pairing.Show(60100, true);
Check(pairing.Expansion == dismissed, "A new prompt must reverse dismissal from its current position.");
pairing.Dismiss(60100, false);
Check(!pairing.Visible && !pairing.Animating, "Reduced motion must finish explicit dismissal immediately.");

motion.Show(30000, true);
double previous = 0;
for (int t = 0; t <= 320; t += 10)
{
    motion.Advance(30000 + t);
    Check(motion.Expansion >= previous && motion.Expansion <= 1, "Capsule entrance must settle without reversing or overshooting.");
    previous = motion.Expansion;
}
var gauge = new IslandGaugeMotion();
gauge.Set(80, 0, true);
gauge.Advance(100);
double midway = gauge.Value;
gauge.Set(80, 100, true);
gauge.Advance(280);
Check(gauge.Value == 80 && !gauge.Animating, "Repeated reports must not restart the battery animation.");
gauge.Set(40, 300, true);
gauge.Advance(400);
midway = gauge.Value;
gauge.Set(100, 400, true);
Check(gauge.Value == midway, "A changed battery target must continue from the displayed position.");
gauge.Advance(680);
Check(gauge.Value == 100 && !gauge.Animating, "The ring must settle on the latest battery target.");
gauge.Set(0, 700, false);
Check(gauge.Value == 0 && !gauge.Animating, "An empty battery remains valid with reduced motion.");
gauge.Set(50, 800, true);
gauge.Advance(900);
gauge.Set(null, 900, true);
Check(gauge.Value == 0 && !gauge.Animating, "An absent reading must clear the old ring immediately.");
Console.WriteLine("Battery ring motion checks passed.");

foreach (int refreshRate in new[] { 60, 120, 144, 240 })
{
    var paced = new IslandMotion();
    paced.Show(0, true);
    for (double now = 0; now < 320; now += 1000.0 / refreshRate)
        paced.Advance(now);
    paced.Advance(320);
    Check(paced.Expansion == 1 && !paced.Animating, "Display refresh must not change the opening duration.");
    Check(paced.DismissDelay(4320) == 1, "The idle wake-up must respect the reading deadline.");
    paced.Advance(4320);
    for (double now = 4320; now < 4540; now += 1000.0 / refreshRate)
        paced.Advance(now);
    paced.Advance(4540);
    Check(!paced.Visible, "Display refresh must not change the dismissal duration.");
}
Console.WriteLine("Display-paced motion checks passed.");

var message = new IslandMessageMotion();
message.Set(new("Connected"), 0, true);
Check(!message.Animating && message.Progress == 1, "The initial status must not animate from an empty row.");
message.Set(new("Adaptive", IslandMessageIcon.Adaptive), 10, true);
message.Advance(50);
Check(Math.Abs(message.Progress - 0.5) < 0.00001, "Status rows must use the 200 ms push curve.");
double messageProgress = message.Progress;
message.Set(new("Transparency", IslandMessageIcon.Transparency), 50, true);
Check(message.Current.Text == "Adaptive" && message.Previous.Text == "Connected" && message.Progress == messageProgress,
    "An interrupt must preserve both visible text rows and their current positions.");
message.Set(new("Microphone ready", IslandMessageIcon.Microphone), 50, true);
message.Advance(210);
Check(message.Current.Text == "Microphone ready" && message.Previous.Text == "Adaptive" && message.Progress == 0,
    "The next push must start from the settled message and coalesce superseded events.");
Check(message.Current.Icon == IslandMessageIcon.Microphone && message.Previous.Icon == IslandMessageIcon.Adaptive,
    "Message icons must follow the same transition as their text.");
message.Advance(410);
Check(!message.Animating && message.Progress == 1 && message.Previous.Text == "", "The latest message must settle without a jump.");
message.Set(new("Off", IslandMessageIcon.MicrophoneOff), 420, true);
message.Set(new("Connected"), 450, true);
message.Finish();
Check(message.Current.Text == "Connected" && !message.Animating && message.Progress == 1,
    "Reduced motion must show the latest queued event immediately.");
message.Set(new("Adaptive"), 500, true);
message.Set(new("Transparency"), 510, true);
message.Set(new("Adaptive"), 520, true);
message.Advance(700);
Check(message.Current.Text == "Adaptive" && !message.Animating, "Repeating the current target must cancel a superseded queued event.");
message.Set(new("Connecting"), 710, true);
message.Set(new("Nearby"), 720, false);
Check(message.Current.Text == "Nearby" && message.Previous.Text == "" && message.Progress == 1 && !message.Animating,
    "Reduced motion must discard outgoing and queued status rows immediately.");
Console.WriteLine("Interrupted message checks passed.");

var resizing = new IslandMotion();
resizing.SetHeight(160, 0, true);
resizing.Show(0, true);
resizing.Advance(420);
double deadline = resizing.DismissDelay(500);
resizing.SetHeight(190, 500, true);
Check(resizing.Height == 160 && resizing.Resizing, "A visible content resize must start at the displayed height.");
resizing.Advance(560);
double height = resizing.Height;
Check(height > 160 && height < 190, "The island content height must interpolate.");
resizing.SetHeight(170, 560, true);
Check(resizing.Height == height, "A second content resize must preserve the displayed height.");
resizing.Advance(740);
Check(resizing.Height == 170 && !resizing.Resizing && resizing.Expansion == 1, "Content resize must settle without replaying entrance.");
Check(resizing.DismissDelay(740) == deadline - 240, "Content resize must not extend the dismissal deadline.");
resizing.SetHeight(200, 750, true);
resizing.SetAnimationsEnabled(780, false);
Check(resizing.Height == 200 && !resizing.Resizing, "Disabling animations must finish a content resize.");
resizing.Hide();
resizing.SetHeight(240, 800, true);
resizing.Show(800, true);
resizing.Advance(860);
Check(resizing.Height == 240 && !resizing.Resizing,
    "A taller new island must open at its final height without clipping the battery row.");
Console.WriteLine("Content height motion checks passed.");

var action = new IslandActionMotion();
action.Set(IslandConnectState.Ready, 0, true);
action.Advance(50);
double readyOpacity = action.Opacity(IslandConnectState.Ready);
action.Set(IslandConnectState.Connecting, 50, true);
Check(action.Opacity(IslandConnectState.Ready) == readyOpacity && !action.CanConnect,
    "Progress must disable Connect immediately without jumping the current blend.");
action.Advance(100);
action.Set(IslandConnectState.Failed, 100, true);
action.Advance(300);
Check(action.Opacity(IslandConnectState.Failed) == 1 && action.CanConnect && !action.Animating,
    "An interrupted progress change must settle on Retry.");
action.Set(IslandConnectState.Hidden, 310, true);
Check(!action.CanConnect, "A fading action must stop accepting input immediately.");
action.Set(IslandConnectState.Ready, 320, false);
Check(action.Opacity(IslandConnectState.Ready) == 1 && !action.Animating,
    "Reduced motion must render the final action directly.");

var actionTiming = new IslandActionMotion();
actionTiming.Set(IslandConnectState.Ready, 0, true);
Check(actionTiming.Visibility == 0 && !actionTiming.CanConnect,
    "An invisible action must not accept input before its first visible frame.");
actionTiming.Advance(139);
Check(actionTiming.Visibility > 0 && actionTiming.Visibility < 1 && actionTiming.CanConnect,
    "A visible Connect action must become interactive during its 140 ms entrance.");
actionTiming.Advance(140);
Check(actionTiming.Visibility == 1 && !actionTiming.Animating,
    "The action entrance must stop frame work after 140 ms.");
actionTiming.Set(IslandConnectState.Connecting, 200, true);
Check(!actionTiming.CanConnect && actionTiming.Opacity(IslandConnectState.Ready) == 1,
    "Starting a connection must disable the outgoing Connect action immediately.");
actionTiming.Advance(279);
Check(actionTiming.Opacity(IslandConnectState.Ready) > 0 && actionTiming.Opacity(IslandConnectState.Connecting) == 0,
    "The outgoing label must fade before the new label appears.");
actionTiming.Advance(280);
Check(actionTiming.Opacity(IslandConnectState.Ready) == 0 && actionTiming.Opacity(IslandConnectState.Connecting) == 0,
    "Action labels must not overlap at their 80 ms handoff.");
actionTiming.Advance(340);
Check(actionTiming.Visibility == 1 && actionTiming.Opacity(IslandConnectState.Connecting) > 0,
    "Label changes must leave the action surface fully visible.");
double progressOpacity = actionTiming.Opacity(IslandConnectState.Connecting);
actionTiming.Set(IslandConnectState.Failed, 340, true);
Check(actionTiming.Opacity(IslandConnectState.Connecting) == progressOpacity && !actionTiming.CanConnect,
    "An interrupted label change must preserve its frame and keep an invisible Retry inactive.");
actionTiming.Advance(540);
Check(actionTiming.Opacity(IslandConnectState.Failed) == 1 && actionTiming.CanConnect && !actionTiming.Animating,
    "The latest label must settle after its 80 ms exit and 120 ms entrance.");
actionTiming.Set(IslandConnectState.Hidden, 550, true);
Check(!actionTiming.CanConnect && actionTiming.Opacity(IslandConnectState.Failed) == 1,
    "Hiding must freeze the outgoing label and remove interaction immediately.");
actionTiming.Advance(649);
Check(actionTiming.Visibility > 0 && actionTiming.Animating, "The action exit must last 100 ms.");
actionTiming.Advance(650);
Check(actionTiming.Visibility == 0 && !actionTiming.Animating, "A hidden action must stop frame work.");
actionTiming.Set(IslandConnectState.Ready, 700, false);
actionTiming.Set(IslandConnectState.Connecting, 710, true);
actionTiming.Advance(750);
progressOpacity = actionTiming.Opacity(IslandConnectState.Ready);
actionTiming.Set(IslandConnectState.Hidden, 750, true);
actionTiming.Set(IslandConnectState.Ready, 770, true);
Check(actionTiming.Opacity(IslandConnectState.Ready) == progressOpacity,
    "Reversing a hide must preserve the displayed label opacity.");
actionTiming.Advance(970);
Check(actionTiming.Visibility == 1 && actionTiming.Opacity(IslandConnectState.Ready) == 1 && !actionTiming.Animating,
    "A label interrupted by hide and reopen must recover its full opacity.");
actionTiming.Set(IslandConnectState.Connecting, 1000, true);
actionTiming.Finish();
Check(actionTiming.Visibility == 1 && actionTiming.Opacity(IslandConnectState.Connecting) == 1 && !actionTiming.Animating,
    "Reduced motion must finish both visibility and label transitions.");

var pinned = new IslandMotion();
pinned.Show(0, false);
pinned.Pin(true, 10);
pinned.Advance(10000);
Check(pinned.Visible && pinned.Pinned, "Connection progress must hold the island open.");
pinned.Pin(false, 10000);
pinned.Advance(13999);
Check(pinned.Visible, "Completing the request must start a fresh reading interval.");
pinned.Advance(14000);
Check(!pinned.Visible, "Reduced motion must dismiss after the fresh interval.");
Console.WriteLine("Connect action and hold checks passed.");
