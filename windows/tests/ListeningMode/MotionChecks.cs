using NTPods.WinUI.Controls;

internal static class MotionChecks
{
    public static void Run()
    {
        var compact = ListeningModeLayout.Create(600, 3, false);
        var adaptive = ListeningModeLayout.Create(600, 4, false);
        ListeningModeMotion Create()
        {
            var result = new ListeningModeMotion();
            result.SetTarget(compact, 0, 0, false, true);
            return result;
        }

        var motion = Create();
        Check(!motion.IsMoving && motion.Layout == compact, "Initial measure must not animate from zero width.");
        motion.SetTarget(adaptive, 240, 10, false, true);
        motion.Tick(60);
        Check(motion.Layout.Segment is > 120 and < 200 && motion.Left is > 0 and < 240,
            "Mode changes must spring width and position together.");
        var drag = motion.Layout.ResolveDrag(motion.Layout.LeftFor(4, 37), 0, false);
        Check(drag.Mode == 4 && drag.Strength == 37, "Hit testing must use the displayed geometry.");
        double previous = motion.Left;
        motion.SetTarget(compact, 0, 60, false, true);
        Check(motion.Left == previous, "Reversing must preserve the current position.");
        motion.Tick(60.01);
        Check(motion.Left > previous, "Reversing must retain velocity rather than stopping instantly.");
        motion.Tick(500);
        Check(!motion.IsMoving && motion.Left == 0 && motion.Layout == compact,
            "A critically damped reversal must settle and stop frame work.");

        var uninterrupted = Create();
        motion = Create();
        uninterrupted.SetTarget(adaptive, 240, 10, false, true);
        motion.SetTarget(adaptive, 240, 10, false, true);
        for (int now = 20; now <= 500; now += 10)
        {
            uninterrupted.Tick(now);
            motion.SetTarget(adaptive, 240, now, false, true);
            Check(motion.Layout == uninterrupted.Layout && motion.Left == uninterrupted.Left,
                "Repeated snapshots must not restart a spring or reset its velocity.");
        }
        Check(!motion.IsMoving, "Repeated snapshots must leave settled animation idle.");
        motion.SetTarget(compact, 137.25, 510, true, true);
        motion.Tick(550);
        Check(motion.Left == 137.25, "Pointer-owned position must remain immediate during width changes.");
        motion.SetTarget(compact, 149.75, 555, true, true);
        Check(motion.Left == 149.75, "Every pointer update must replace its position without easing.");
        motion.SetTarget(compact, 150, 560, false, true);
        motion.Tick(1000);
        Check(motion.Left == 150 && motion.Layout == compact && !motion.IsMoving,
            "Release must settle at its final target.");

        // Changing Off availability while expanding must never produce invalid GridLength values.
        motion = Create();
        for (int now = 1; now <= 1600; now++)
        {
            if (now % 31 == 1)
            {
                var target = ListeningModeLayout.Create(600, now % 2 == 0 ? 4u : 3u, now % 3 == 0);
                motion.SetTarget(target, target.LeftFor(4, 50), now, false, true);
            }
            motion.Tick(now);
            var layout = motion.Layout;
            Check(layout.Segment >= 0 && layout.OffWidth >= 0 && layout.AdaptiveWidth >= 0,
                "Interrupted column springs must remain valid layout dimensions.");
            Check(Math.Abs(layout.Segment * 2 + layout.OffWidth + layout.AdaptiveWidth - 600) < 1e-9,
                "Spring columns must fill the track even near their settling thresholds.");
        }

        motion = Create();
        motion.Press(true, 0, true);
        motion.DragVelocity(1250, 0, true);
        motion.Tick(1000);
        Check(Near(motion.ScaleX, 1.5) && Near(motion.ScaleY, 1.05) && !motion.IsMoving,
            "Velocity 1250 DIP/s must give stretch 0.5, with the upstream press/stretch scales.");
        motion.DragVelocity(-5000, 1000, true);
        motion.Tick(2000);
        Check(Near(motion.ScaleX, 1.8) && Near(motion.ScaleY, 0.9),
            "Negative velocity must stretch by its magnitude and clamp at one.");
        motion.Press(false, 2000, true);
        bool rebounded = false;
        for (int now = 2001; now <= 4000; now++)
        {
            motion.Tick(now);
            rebounded |= motion.ScaleX < 1;
            Check(double.IsFinite(motion.ScaleX) && motion.ScaleX >= 0.4 && motion.ScaleX <= 1.8 &&
                double.IsFinite(motion.ScaleY) && motion.ScaleY >= 0.7 && motion.ScaleY <= 1.5,
                "Release bounce must stay finite and bounded.");
        }
        Check(rebounded && !motion.IsMoving && motion.ScaleX == 1 && motion.ScaleY == 1,
            "Stretch release must rebound with damping 0.2, then stop at neutral.");

        motion = Create();
        motion.Press(true, 0, true);
        motion.Tick(20);
        previous = motion.ScaleX;
        motion.Press(false, 20, true);
        motion.Tick(20.01);
        Check(motion.ScaleX > previous, "An interrupted press must retain its velocity.");
        motion.SetTarget(adaptive, 240, 21, false, true);
        motion.ResetInteraction();
        Check(motion.ScaleX == 1 && motion.ScaleY == 1 && motion.IsMoving,
            "Resetting interaction must clear effects without losing the geometry target.");
        motion.Finish();
        Check(motion.Layout == adaptive && motion.Left == 240 && !motion.IsMoving,
            "Finish must complete geometry and clear all interaction effects.");

        foreach (double velocity in new[] { double.NaN, double.PositiveInfinity, double.NegativeInfinity, double.MaxValue })
        {
            motion.DragVelocity(velocity, 30, true);
            motion.Tick(5000);
            Check(double.IsFinite(motion.ScaleX) && motion.ScaleX is >= 0.4 and <= 1.8,
                "Invalid or extreme velocity must not corrupt decorative transforms.");
            motion.Finish();
        }
        motion = Create();
        motion.Press(true, 0, true);
        motion.DragVelocity(5000, 0, true);
        motion.Tick(100);
        motion.SetTarget(adaptive, 240, 100, false, false);
        motion.Press(true, 100, false);
        motion.DragVelocity(5000, 100, false);
        Check(motion.Layout == adaptive && motion.Left == 240 && motion.ScaleX == 1 && motion.ScaleY == 1 && !motion.IsMoving,
            "Reduced motion must apply geometry immediately and keep decoration neutral.");
        motion.SetTarget(ListeningModeLayout.Create(360, 4, false), 120, 101, false, true);
        Check(motion.Left == 120 && motion.Layout.Width == 360 && !motion.IsMoving,
            "Resize must discard obsolete geometry.");

        // Analytic evaluation must not depend on frame frequency or a dropped frame.
        var coarse = Create();
        var fine = Create();
        coarse.Press(true, 0, true); fine.Press(true, 0, true);
        coarse.DragVelocity(3000, 0, true); fine.DragVelocity(3000, 0, true);
        for (int now = 1; now <= 100; now++) fine.Tick(now);
        coarse.Tick(100);
        Check(Near(coarse.ScaleX, fine.ScaleX) && Near(coarse.ScaleY, fine.ScaleY),
            "Press and stretch must follow the same trajectory across frame cadences.");
        coarse.Press(false, 100, true); fine.Press(false, 100, true);
        for (int now = 101; now <= 210; now++) fine.Tick(now);
        coarse.Tick(210);
        Check(Near(coarse.ScaleX, fine.ScaleX) && Near(coarse.ScaleY, fine.ScaleY),
            "Underdamped return must also be independent of frame cadence.");
        coarse.Tick(double.NaN); coarse.Tick(double.NegativeInfinity); coarse.Tick(200);
        Check(Near(coarse.ScaleX, fine.ScaleX), "Invalid or backwards frame timestamps must not rewind motion.");
    }

    private static bool Near(double a, double b) => Math.Abs(a - b) < 1e-9;
    private static void Check(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
}
