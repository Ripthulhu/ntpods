using System.Diagnostics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.Foundation;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Services;

/// Reveals natural-sized content while the surrounding layout follows its visible height.
public sealed class ExpanderRevealPanel : Panel
{
    private readonly UISettings _settings = new();
    private Expander? _owner;
    private ToggleButton? _header;
    private Border? _body;
    private double _naturalHeight, _height, _from, _target, _started, _duration;
    private bool _rendering, _initial;
    private static double Now => Stopwatch.GetTimestamp() * 1000d / Stopwatch.Frequency;

    public ExpanderRevealPanel()
    {
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    private void OnLoaded(object sender, RoutedEventArgs args)
    {
        if (_owner is not null) return;
        for (DependencyObject? ancestor = VisualTreeHelper.GetParent(this); ancestor is not null;
             ancestor = VisualTreeHelper.GetParent(ancestor))
            if (ancestor is Expander expander) { _owner = expander; break; }
        if (_owner is null || Children.FirstOrDefault() is not Border body) return;
        _body = body;
        _body.GettingFocus += BodyGettingFocus;
        if (VisualTreeHelper.GetChild(_owner, 0) is Grid root)
            _header = root.Children.OfType<ToggleButton>().FirstOrDefault();
        _owner.Expanding += Expanding;
        _owner.Collapsed += Collapsed;
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _settings.AnimationsEnabledChanged += AnimationsChanged;
        _initial = true;
        _body.Visibility = _owner.IsExpanded ? Visibility.Visible : Visibility.Collapsed;
        _body.IsHitTestVisible = _owner.IsExpanded;
        InvalidateMeasure();
    }

    private void OnUnloaded(object sender, RoutedEventArgs args)
    {
        StopRendering();
        if (_owner is not null)
        {
            _owner.Expanding -= Expanding;
            _owner.Collapsed -= Collapsed;
        }
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _settings.AnimationsEnabledChanged -= AnimationsChanged;
        if (_body is not null) _body.GettingFocus -= BodyGettingFocus;
        _owner = null;
        _header = null;
        _body = null;
    }

    protected override Size MeasureOverride(Size availableSize)
    {
        if (Children.FirstOrDefault() is not FrameworkElement body) return default;
        body.Measure(new Size(availableSize.Width, double.PositiveInfinity));
        _naturalHeight = body.DesiredSize.Height;
        if (_owner is not null)
        {
            double target = _owner.IsExpanded ? _naturalHeight : 0;
            if (_initial || target != _target) Retarget(target, !_initial);
            _initial = false;
        }
        return new Size(body.DesiredSize.Width, _height);
    }

    protected override Size ArrangeOverride(Size finalSize)
    {
        if (Children.FirstOrDefault() is FrameworkElement body)
            body.Arrange(new Rect(0, 0, finalSize.Width, _naturalHeight));
        return finalSize;
    }

    private void Expanding(Expander sender, ExpanderExpandingEventArgs args)
    {
        if (_body is null) return;
        _body.Visibility = Visibility.Visible;
        _body.IsHitTestVisible = true;
        InvalidateMeasure();
    }

    private void Collapsed(Expander sender, ExpanderCollapsedEventArgs args)
    {
        if (_body is null) return;
        _body.IsHitTestVisible = false;
        if (XamlRoot is not null)
            for (DependencyObject? focused = FocusManager.GetFocusedElement(XamlRoot) as DependencyObject;
                 focused is not null; focused = VisualTreeHelper.GetParent(focused))
                if (ReferenceEquals(focused, _body)) { _header?.Focus(FocusState.Programmatic); break; }
        Retarget(0, true);
        InvalidateMeasure();
    }

    private void BodyGettingFocus(UIElement sender, GettingFocusEventArgs args)
    {
        if (_owner?.IsExpanded != false) return;
        if (_header is null || !args.TrySetNewFocusedElement(_header)) args.TryCancel();
    }

    private void Retarget(double target, bool animate)
    {
        Advance(Now);
        _from = _height;
        _target = target;
        _started = Now;
        _duration = target > _height ? 240 : 180;
        if (!animate || !_settings.AnimationsEnabled || Math.Abs(_target - _height) < 0.1)
        {
            _height = _target;
            Finish();
        }
        else if (!_rendering)
        {
            CompositionTarget.Rendering += Frame;
            _rendering = true;
        }
    }

    private void Advance(double now)
    {
        if (!_rendering) return;
        double progress = Math.Clamp((now - _started) / _duration, 0, 1);
        _height = _from + (_target - _from) * (1 - Math.Pow(1 - progress, 3));
        if (progress >= 1) Finish();
    }

    private void Frame(object? sender, object args)
    {
        Advance(Now);
        InvalidateMeasure();
    }

    private void Finish()
    {
        _height = _target;
        StopRendering();
        if (_body is not null && _target == 0 && _owner?.IsExpanded == false)
            _body.Visibility = Visibility.Collapsed;
    }

    private void StopRendering()
    {
        if (!_rendering) return;
        CompositionTarget.Rendering -= Frame;
        _rendering = false;
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_owner is null || sender.AnimationsEnabled) return;
            Finish();
            InvalidateMeasure();
        });
}
