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
motion.Show(7100, true);
Check(motion.Expansion == closing, "A new event must reverse dismissal from its current size.");
motion.Advance(7500);
Check(motion.Expansion == 1, "Interrupted dismissal must reopen fully.");
motion.Advance(11500);
motion.Advance(11720);
Check(!motion.Visible && motion.Expansion == 0, "Dismissal must finish and release the timer.");

motion.Show(12000, false);
Check(motion.Visible && motion.Expansion == 1 && !motion.Animating, "Reduced motion must show all content immediately.");
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
