namespace NTPods.WinUI.Popup;

internal sealed class IslandMotion
{
    private enum Phase { Hidden, Opening, Holding, Closing }
    private Phase _phase;
    private double _started;
    private double _from;
    private bool _animate;
    private double _heightFrom, _heightTarget, _heightStarted;

    public double Expansion { get; private set; }
    public double Height { get; private set; }
    public bool Resizing { get; private set; }
    public bool Visible => _phase != Phase.Hidden;
    public bool Animating => _phase is Phase.Opening or Phase.Closing || Resizing;
    public double HeaderOpacity => Smooth(Math.Clamp((Expansion - 0.08) / 0.72, 0, 1));
    public double BatteryOpacity => Smooth(Math.Clamp((Expansion - 0.32) / 0.68, 0, 1));
    public double DismissDelay(double now) => Math.Max(1, 4000 - (now - _started));

    public void Show(double now, bool animate)
    {
        Advance(now);
        _animate = animate;
        _started = now;
        _from = Expansion;
        if (!animate) Expansion = 1;
        _phase = Expansion < 1 ? Phase.Opening : Phase.Holding;
    }

    public void Advance(double now)
    {
        AdvanceHeight(now);
        double elapsed = Math.Max(0, now - _started);
        if (_phase == Phase.Holding)
        {
            if (elapsed < 4000) return;
            if (!_animate) { Hide(); return; }
            _from = Expansion;
            _started = now;
            _phase = Phase.Closing;
            return;
        }
        if (_phase is not (Phase.Opening or Phase.Closing)) return;
        bool opening = _phase == Phase.Opening;
        double duration = opening ? 420 * (1 - _from) : 220 * _from;
        double t = Math.Clamp(elapsed / Math.Max(1, duration), 0, 1);
        // Settle without overshoot at the screen edge.
        double eased = opening ? (1 - (1 + 8 * t) * Math.Exp(-8 * t)) / (1 - 9 * Math.Exp(-8))
                               : 1 - Math.Pow(1 - t, 3);
        Expansion = _from + ((opening ? 1 : 0) - _from) * eased;
        if (t < 1) return;
        _phase = opening ? Phase.Holding : Phase.Hidden;
        _started = now;
    }

    public void SetAnimationsEnabled(double now, bool animate)
    {
        Advance(now);
        _animate = animate;
        if (animate) return;
        Height = _heightTarget;
        Resizing = false;
        if (_phase == Phase.Closing) Hide();
        else if (_phase == Phase.Opening)
        {
            Expansion = 1;
            _phase = Phase.Holding;
            _started = now;
        }
    }

    public void Hide()
    {
        _phase = Phase.Hidden;
        Expansion = 0;
        Height = _heightTarget;
        Resizing = false;
    }

    public void SetHeight(double height, double now, bool animate)
    {
        if (!double.IsFinite(height) || height <= 0) return;
        AdvanceHeight(now);
        if (!animate || !Visible || Height == 0)
        {
            Height = _heightTarget = height;
            Resizing = false;
            return;
        }
        if (_heightTarget == height) return;
        _heightFrom = Height;
        _heightTarget = height;
        _heightStarted = now;
        Resizing = Height != height;
    }

    private void AdvanceHeight(double now)
    {
        if (!Resizing) return;
        double t = Math.Clamp((now - _heightStarted) / 180, 0, 1);
        Height = _heightFrom + (_heightTarget - _heightFrom) * (1 - Math.Pow(1 - t, 3));
        Resizing = t < 1;
    }

    private static double Smooth(double value) => value * value * (3 - 2 * value);
}
