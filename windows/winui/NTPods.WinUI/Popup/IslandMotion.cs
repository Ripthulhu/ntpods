namespace NTPods.WinUI.Popup;

public enum IslandConnectState { Hidden, Ready, Connecting, Failed }

internal sealed class IslandMotion(double holdMilliseconds = 4000, double openingMilliseconds = 320)
{
    private enum Phase { Hidden, Opening, Holding, Closing }
    private Phase _phase;
    private double _started;
    private double _from;
    private bool _animate;
    private bool _pinned;
    private double _heightFrom, _heightTarget, _heightStarted;

    public double Expansion { get; private set; }
    public double Height { get; private set; }
    public bool Resizing { get; private set; }
    public bool Visible => _phase != Phase.Hidden;
    public bool Closing => _phase == Phase.Closing;
    public bool Pinned => _pinned;
    public bool Animating => _phase is Phase.Opening or Phase.Closing || Resizing;
    public double PayloadScale => 0.96 + 0.04 * Expansion;
    public double PayloadOffset => 34 * (1 - Expansion);
    public double DismissDelay(double now) => Math.Max(1, holdMilliseconds - (now - _started));

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
            if (_pinned || elapsed < holdMilliseconds) return;
            if (!_animate) { Hide(); return; }
            _from = Expansion;
            _started = now;
            _phase = Phase.Closing;
            return;
        }
        if (_phase is not (Phase.Opening or Phase.Closing)) return;
        bool opening = _phase == Phase.Opening;
        double duration = opening ? openingMilliseconds * (1 - _from) : 220 * _from;
        double t = Math.Clamp(elapsed / Math.Max(1, duration), 0, 1);
        Expansion = _from + ((opening ? 1 : 0) - _from) * Ease(t);
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
        _pinned = false;
        Expansion = 0;
        Height = _heightTarget;
        Resizing = false;
    }

    public void Dismiss(double now, bool animate)
    {
        Advance(now);
        if (!Visible) return;
        if (!animate) { Hide(); return; }
        _from = Expansion;
        _started = now;
        _phase = Phase.Closing;
    }

    public void Pin(bool pinned, double now)
    {
        if (_pinned == pinned) return;
        _pinned = pinned;
        if (!pinned && _phase == Phase.Holding) _started = now;
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

    internal static double Ease(double progress)
    {
        if (progress <= 0) return 0;
        if (progress >= 1) return 1;
        double low = 0, high = 1;
        // Invert the x coordinate of cubic-bezier(.2, 0, 0, 1).
        for (int i = 0; i < 20; i++)
        {
            double t = (low + high) / 2;
            double x = 0.6 * t * (1 - t) * (1 - t) + t * t * t;
            if (x < progress) low = t;
            else high = t;
        }
        double position = (low + high) / 2;
        return position * position * (3 - 2 * position);
    }
}

internal sealed class IslandActionMotion
{
    private readonly double[] _from = new double[4];
    private readonly double[] _opacity = [0, 1, 0, 0];
    private double _started;
    private double _visibilityFrom, _visibilityStarted;
    private bool _visibilityAnimating, _labelAnimating;
    private IslandConnectState _labelState = IslandConnectState.Ready;
    public IslandConnectState State { get; private set; }
    public double Visibility { get; private set; }
    public bool Animating => _visibilityAnimating || _labelAnimating;
    public bool CanConnect => (State is IslandConnectState.Ready or IslandConnectState.Failed)
        && Visibility > 0 && Opacity(State) > 0;
    public double Opacity(IslandConnectState state) => state == IslandConnectState.Hidden
        ? 1 - Visibility : _opacity[(int)state];

    public void Set(IslandConnectState state, double now, bool animate)
    {
        Advance(now);
        if (state == State) { if (!animate) Finish(); return; }
        bool visibilityChanged = (state == IslandConnectState.Hidden) != (State == IslandConnectState.Hidden);
        State = state;
        if (visibilityChanged)
        {
            _visibilityFrom = Visibility;
            _visibilityStarted = now;
            _visibilityAnimating = animate;
        }
        if (state == IslandConnectState.Hidden)
            _labelAnimating = false;
        else if (_labelState != state || Opacity(state) != 1 || Visibility == 0)
        {
            _labelState = state;
            _started = now;
            Array.Copy(_opacity, _from, _opacity.Length);
            _labelAnimating = animate && Visibility > 0;
            if (!_labelAnimating) FinishLabel();
        }
        if (!animate) Finish();
    }

    public void Advance(double now)
    {
        if (_visibilityAnimating)
        {
            bool showing = State != IslandConnectState.Hidden;
            double t = Math.Clamp((now - _visibilityStarted) / (showing ? 140 : 100), 0, 1);
            Visibility = _visibilityFrom + ((showing ? 1 : 0) - _visibilityFrom) * IslandMotion.Ease(t);
            _visibilityAnimating = t < 1;
        }
        if (!_labelAnimating) return;
        double elapsed = Math.Max(0, now - _started);
        double outgoing = 1 - IslandMotion.Ease(Math.Clamp(elapsed / 80, 0, 1));
        double incoming = IslandMotion.Ease(Math.Clamp((elapsed - 80) / 120, 0, 1));
        for (int i = 1; i < _opacity.Length; i++)
            _opacity[i] = _from[i] * outgoing + (i == (int)_labelState ? incoming : 0);
        if (elapsed >= 200) FinishLabel();
    }

    public void Finish()
    {
        Visibility = State == IslandConnectState.Hidden ? 0 : 1;
        _visibilityAnimating = false;
        FinishLabel();
    }

    private void FinishLabel()
    {
        for (int i = 1; i < _opacity.Length; i++) _opacity[i] = i == (int)_labelState ? 1 : 0;
        _labelAnimating = false;
    }
}
