namespace NTPods.WinUI.Popup;

internal enum IslandMessageIcon { None, Transparency, Adaptive, NoiseCancellation, Microphone, MicrophoneOff }
internal readonly record struct IslandMessage(string Text, IslandMessageIcon Icon = IslandMessageIcon.None);

internal sealed class IslandMessageMotion
{
    private const double Duration = 180;
    private double _started;
    private IslandMessage? _pending;
    public IslandMessage Current { get; private set; } = new("");
    public IslandMessage Previous { get; private set; } = new("");
    public IslandMessage Latest => _pending ?? Current;
    public double Opacity { get; private set; } = 1;
    public bool Animating { get; private set; }

    public void Set(IslandMessage message, double now, bool animate)
    {
        Advance(now);
        if (!animate)
        {
            Current = message;
            _pending = null;
            Finish();
        }
        else if (Animating)
        {
            // Finish the visible blend before fading to the latest event.
            _pending = message == Current ? null : message;
        }
        else Begin(message, now);
    }

    private void Begin(IslandMessage message, double now)
    {
        if (Current == message) return;
        Previous = Current;
        Current = message;
        _started = now;
        Animating = Previous.Text.Length > 0;
        Opacity = Animating ? 0 : 1;
    }

    public void Advance(double now)
    {
        if (!Animating) return;
        double t = Math.Clamp((now - _started) / Duration, 0, 1);
        Opacity = 1 - Math.Pow(1 - t, 3);
        if (t < 1) return;
        Animating = false;
        Previous = new("");
        if (_pending is not { } next) return;
        _pending = null;
        Begin(next, _started + Duration);
        Advance(now);
    }

    public void Finish()
    {
        Current = _pending ?? Current;
        _pending = null;
        Previous = new("");
        Opacity = 1;
        Animating = false;
    }
}
