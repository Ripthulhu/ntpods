using System;

namespace NTPods.WinUI.Controls;

/// Pointer motion is immediate; other values use LibrePods' mass-one, stiffness-1500 springs.
internal sealed class ListeningModeMotion
{
    private readonly Spring _segment = new(0.01);
    private readonly Spring _offWidth = new(0.01);
    private readonly Spring _left = new(0.01);
    private readonly Spring _press = new(0.0001);
    private readonly Spring _stretch = new(0.0001);
    private double _width;
    private bool _adaptive = true;
    private double _lastTick = double.NaN;

    public ListeningModeLayout Layout
    {
        get
        {
            double segment = Math.Clamp(_segment.Value, 0, _width / (_adaptive ? 3 : 2));
            if (!_adaptive) return new(_width, segment, _width - 2 * segment, 0, false);
            double off = Math.Clamp(_offWidth.Value, 0, Math.Max(0, _width - 3 * segment));
            return new(_width, segment, off, _width - 2 * segment - off);
        }
    }
    public double Left => _left.Value;
    public double ScaleX => 1 + 0.2 * Math.Clamp(_press.Value, 0, 1) + 0.6 * Math.Clamp(_stretch.Value, -1, 1);
    public double ScaleY => 1 + 0.2 * Math.Clamp(_press.Value, 0, 1) - 0.3 * Math.Clamp(_stretch.Value, -1, 1);
    public bool IsMoving => _segment.IsMoving || _offWidth.IsMoving ||
        _left.IsMoving || _press.IsMoving || _stretch.IsMoving;

    public void SetTarget(ListeningModeLayout layout, double left, double now, bool pointerOwned, bool animate)
    {
        Tick(now);
        if (!animate || _width != layout.Width || _adaptive != layout.HasAdaptive)
        {
            _width = layout.Width;
            _adaptive = layout.HasAdaptive;
            _segment.Snap(layout.Segment);
            _offWidth.Snap(layout.OffWidth);
            _left.Snap(left);
            if (!animate) ResetInteraction();
            return;
        }
        _segment.Target = layout.Segment;
        _offWidth.Target = layout.OffWidth;
        if (pointerOwned) _left.Snap(left);
        else _left.Target = left;
    }

    public void Press(bool pressed, double now, bool animate)
    {
        Tick(now);
        if (!animate) { ResetInteraction(); return; }
        _press.Target = pressed ? 1 : 0;
        if (!pressed) ReturnStretch();
    }

    public void DragVelocity(double velocity, double now, bool animate)
    {
        Tick(now);
        if (!animate) { ResetInteraction(); return; }
        double amount = double.IsFinite(velocity) ? Math.Clamp(Math.Sqrt(Math.Abs(velocity) / 50) / 10, 0, 1) : 0;
        _stretch.Damping = amount == 0 ? 0.2 : 1;
        _stretch.Target = amount;
    }

    private void ReturnStretch()
    {
        _stretch.Damping = 0.2;
        _stretch.Target = 0;
    }

    public void Tick(double now)
    {
        if (!double.IsFinite(now)) return;
        if (double.IsNaN(_lastTick)) { _lastTick = now; return; }
        if (now <= _lastTick) return;
        double seconds = (now - _lastTick) / 1000;
        _lastTick = now;
        _segment.Tick(seconds);
        _offWidth.Tick(seconds);
        _left.Tick(seconds);
        _press.Tick(seconds);
        _stretch.Tick(seconds);
    }

    public void ResetInteraction()
    {
        _press.Snap(0);
        _stretch.Snap(0);
    }

    public void Finish()
    {
        _segment.Snap(_segment.Target);
        _offWidth.Snap(_offWidth.Target);
        _left.Snap(_left.Target);
        ResetInteraction();
    }

    // Evaluate the analytic solution, so high-refresh displays and dropped frames
    // follow the same trajectory without restarting velocity at a new target.
    private sealed class Spring(double tolerance)
    {
        private const double Omega = 38.72983346207417; // sqrt(1500 / 1)
        private double _velocity;
        public double Value { get; private set; }
        public double Target { get; set; }
        public double Damping { get; set; } = 1;
        public bool IsMoving => Math.Abs(Value - Target) > tolerance || Math.Abs(_velocity) > tolerance * 10;

        public void Snap(double value)
        {
            Value = Target = value;
            _velocity = 0;
        }

        public void Tick(double seconds)
        {
            if (!IsMoving || seconds >= 10) { Snap(Target); return; }
            double offset = Value - Target;
            double decay = Math.Exp(-Damping * Omega * seconds);
            if (Damping == 1)
            {
                double coefficient = _velocity + Omega * offset;
                Value = Target + decay * (offset + coefficient * seconds);
                _velocity = decay * (_velocity - Omega * coefficient * seconds);
            }
            else
            {
                double frequency = Omega * Math.Sqrt(1 - Damping * Damping);
                double sine = Math.Sin(frequency * seconds), cosine = Math.Cos(frequency * seconds);
                Value = Target + decay * (offset * cosine + (_velocity + Damping * Omega * offset) / frequency * sine);
                _velocity = decay * (_velocity * cosine - (Damping * Omega * _velocity + Omega * Omega * offset) / frequency * sine);
            }
            if (!IsMoving) Snap(Target);
        }
    }
}
