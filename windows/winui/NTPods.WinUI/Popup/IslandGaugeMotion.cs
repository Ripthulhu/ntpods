namespace NTPods.WinUI.Popup;

internal sealed class IslandGaugeMotion
{
    private double _from;
    private double _started;
    private byte? _target;
    public double Value { get; private set; }
    public bool Animating { get; private set; }

    public void Set(byte? target, double now, bool animate)
    {
        Advance(now);
        target = target is <= 100 ? target : null;
        if (_target == target) { if (!animate) Finish(); return; }
        _target = target;
        _from = Value;
        _started = now;
        Animating = animate && target.HasValue && Value != target.Value;
        if (!Animating) Finish();
    }

    public void Advance(double now)
    {
        if (!Animating) return;
        double t = Math.Clamp((now - _started) / 280, 0, 1);
        Value = _from + ((_target ?? 0) - _from) * (1 - Math.Pow(1 - t, 3));
        if (t == 1) Finish();
    }

    public void Finish()
    {
        Value = _target ?? 0;
        Animating = false;
    }
}
