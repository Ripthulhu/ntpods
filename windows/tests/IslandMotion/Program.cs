using NTPods.WinUI.Popup;

static void Check(bool condition, string message)
{
    if (!condition) throw new Exception(message);
}

var motion = new IslandMotion();
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
Check(motion.Closing && motion.PayloadScale < 1 && motion.PayloadOffset < 0,
    "The outgoing payload must fold together during exit.");
motion.Show(7100, true);
Check(motion.Expansion == closing, "A new event must reverse dismissal from its current size.");
Check(!motion.Closing, "Reopening must cancel the old exit before preparing new content.");
motion.Advance(7500);
Check(motion.Expansion == 1, "Interrupted dismissal must reopen fully.");
motion.Advance(11500);
motion.Advance(11720);
Check(!motion.Visible && motion.Expansion == 0, "Dismissal must finish and release the timer.");

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

var pairing = new IslandMotion(double.PositiveInfinity);
pairing.Show(0, true);
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
for (int t = 0; t <= 420; t += 10)
{
    motion.Advance(30000 + t);
    Check(motion.Expansion >= previous && motion.Expansion <= 1, "Capsule entrance must settle without reversing or overshooting.");
    Check(motion.HeaderOpacity >= motion.BatteryOpacity, "The header must appear before the battery details.");
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
    for (double now = 0; now < 420; now += 1000.0 / refreshRate)
        paced.Advance(now);
    paced.Advance(420);
    Check(paced.Expansion == 1 && !paced.Animating, "Display refresh must not change the opening duration.");
    Check(paced.DismissDelay(4420) == 1, "The idle wake-up must respect the reading deadline.");
    paced.Advance(4420);
    for (double now = 4420; now < 4640; now += 1000.0 / refreshRate)
        paced.Advance(now);
    paced.Advance(4640);
    Check(!paced.Visible, "Display refresh must not change the dismissal duration.");
}
Console.WriteLine("Display-paced motion checks passed.");

var message = new IslandMessageMotion();
message.Set(new("Connected"), 0, true);
message.Set(new("Adaptive", IslandMessageIcon.Adaptive), 10, true);
message.Advance(40);
double messageOpacity = message.Opacity;
message.Set(new("Transparency", IslandMessageIcon.Transparency), 40, true);
Check(message.Current.Text == "Adaptive" && message.Previous.Text == "Connected" && message.Opacity == messageOpacity,
    "An interrupt must preserve both visible text layers and their current blend.");
message.Set(new("Microphone ready", IslandMessageIcon.Microphone), 50, true);
message.Advance(190);
Check(message.Current.Text == "Microphone ready" && message.Previous.Text == "Adaptive" && message.Opacity == 0,
    "The next fade must start from the settled message and coalesce superseded events.");
Check(message.Current.Icon == IslandMessageIcon.Microphone && message.Previous.Icon == IslandMessageIcon.Adaptive,
    "Message icons must follow the same transition as their text.");
message.Advance(370);
Check(!message.Animating && message.Opacity == 1 && message.Previous.Text == "", "The latest message must settle without a jump.");
message.Set(new("Off", IslandMessageIcon.MicrophoneOff), 400, true);
message.Set(new("Connected"), 430, true);
message.Finish();
Check(message.Current.Text == "Connected" && !message.Animating && message.Opacity == 1,
    "Reduced motion must show the latest queued event immediately.");
message.Set(new("Adaptive"), 500, true);
message.Set(new("Transparency"), 510, true);
message.Set(new("Adaptive"), 520, true);
message.Advance(680);
Check(message.Current.Text == "Adaptive" && !message.Animating, "Repeating the current target must cancel a superseded queued event.");
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
action.Advance(240);
Check(action.Opacity(IslandConnectState.Failed) == 1 && action.CanConnect && !action.Animating,
    "An interrupted progress change must settle on Retry.");
action.Set(IslandConnectState.Hidden, 250, true);
Check(!action.CanConnect, "A fading action must stop accepting input immediately.");
action.Set(IslandConnectState.Ready, 260, false);
Check(action.Opacity(IslandConnectState.Ready) == 1 && !action.Animating,
    "Reduced motion must render the final action directly.");

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
