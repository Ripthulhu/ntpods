using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Services;

/// Fades native scrollbar visuals without changing their hit areas or scroll behavior.
internal sealed class AutoHideScrollbars
{
    private readonly ScrollViewer _viewer;
    private readonly UISettings _settings = new();
    private readonly DispatcherTimer _idle = new() { Interval = TimeSpan.FromMilliseconds(400) };
    private readonly List<ScrollBar> _bars = [];
    private readonly Dictionary<ScrollBar, long> _indicatorTokens = [];
    private readonly HashSet<ScrollBar> _hovered = [];
    private readonly HashSet<uint> _pressed = [];
    private readonly PointerEventHandler _pressHandler;
    private readonly PointerEventHandler _releaseHandler;
    private Storyboard? _fade;
    private bool _loaded;
    private bool _shown;

    public AutoHideScrollbars(ScrollViewer viewer)
    {
        _viewer = viewer;
        _pressHandler = PointerPressed;
        _releaseHandler = PointerReleased;
        _idle.Tick += (_, _) => Hide();
        viewer.Loaded += Loaded;
        viewer.Unloaded += Unloaded;
        if (viewer.IsLoaded) Loaded(viewer, new RoutedEventArgs());
    }

    private void Loaded(object sender, RoutedEventArgs e)
    {
        if (_loaded) return;
        _loaded = true;
        _viewer.ApplyTemplate();
        FindScrollbars(_viewer);
        foreach (var bar in _bars)
        {
            bar.PointerEntered += PointerEntered;
            bar.PointerExited += PointerExited;
            bar.AddHandler(UIElement.PointerPressedEvent, _pressHandler, true);
            bar.AddHandler(UIElement.PointerReleasedEvent, _releaseHandler, true);
            bar.AddHandler(UIElement.PointerCaptureLostEvent, _releaseHandler, true);
            bar.AddHandler(UIElement.PointerCanceledEvent, _releaseHandler, true);
            bar.GotFocus += FocusChanged;
            bar.LostFocus += FocusChanged;
            _indicatorTokens[bar] = bar.RegisterPropertyChangedCallback(ScrollBar.IndicatorModeProperty, IndicatorChanged);
            bar.Opacity = 0;
        }
        _viewer.ViewChanging += ViewChanging;
        _viewer.ViewChanged += ViewChanged;
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _settings.AnimationsEnabledChanged += AnimationsChanged;
    }

    private void FindScrollbars(DependencyObject parent)
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is ScrollBar bar) _bars.Add(bar);
            // Content may contain its own scrolling controls; leave those alone.
            else if (child is not ScrollContentPresenter && child is not ScrollViewer) FindScrollbars(child);
        }
    }

    private void Unloaded(object sender, RoutedEventArgs e)
    {
        if (!_loaded) return;
        _loaded = false;
        _idle.Stop();
        _fade?.Stop();
        _fade = null;
        _viewer.ViewChanging -= ViewChanging;
        _viewer.ViewChanged -= ViewChanged;
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _settings.AnimationsEnabledChanged -= AnimationsChanged;
        foreach (var bar in _bars)
        {
            bar.PointerEntered -= PointerEntered;
            bar.PointerExited -= PointerExited;
            bar.RemoveHandler(UIElement.PointerPressedEvent, _pressHandler);
            bar.RemoveHandler(UIElement.PointerReleasedEvent, _releaseHandler);
            bar.RemoveHandler(UIElement.PointerCaptureLostEvent, _releaseHandler);
            bar.RemoveHandler(UIElement.PointerCanceledEvent, _releaseHandler);
            bar.GotFocus -= FocusChanged;
            bar.LostFocus -= FocusChanged;
            bar.UnregisterPropertyChangedCallback(ScrollBar.IndicatorModeProperty, _indicatorTokens[bar]);
            bar.ClearValue(UIElement.OpacityProperty);
        }
        _bars.Clear();
        _indicatorTokens.Clear();
        _hovered.Clear();
        _pressed.Clear();
        _shown = false;
    }

    private bool HasKeyboardFocus()
    {
        if (_viewer.XamlRoot is null || FocusManager.GetFocusedElement(_viewer.XamlRoot) is not Control focused ||
            focused.FocusState is not (FocusState.Keyboard or FocusState.Programmatic)) return false;
        for (DependencyObject? element = focused; element is not null; element = VisualTreeHelper.GetParent(element))
            if (element is ScrollBar bar && _bars.Contains(bar)) return true;
        return false;
    }

    private bool IsInteracting => _hovered.Count > 0 || _pressed.Count > 0 || HasKeyboardFocus();

    private void Reveal()
    {
        if (!_loaded) return;
        _idle.Stop();
        _fade?.Stop();
        _fade = null;
        _shown = true;
        foreach (var bar in _bars)
        {
            bar.Opacity = 1;
            if (bar.IndicatorMode == ScrollingIndicatorMode.None)
                VisualStateManager.GoToState(bar, "MouseIndicator", false);
        }
        if (!IsInteracting) _idle.Start();
    }

    private void Hide()
    {
        _idle.Stop();
        if (!_loaded || IsInteracting) return;
        _shown = false;
        _fade?.Stop();
        _fade = null;
        if (!_settings.AnimationsEnabled)
        {
            foreach (var bar in _bars) bar.Opacity = 0;
            return;
        }
        _fade = new Storyboard();
        foreach (var bar in _bars)
        {
            var animation = new DoubleAnimation
            {
                To = 0,
                Duration = new Duration(TimeSpan.FromMilliseconds(120)),
                EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }
            };
            Storyboard.SetTarget(animation, bar);
            Storyboard.SetTargetProperty(animation, "Opacity");
            _fade.Children.Add(animation);
        }
        _fade.Begin();
    }

    private void ViewChanging(object? sender, ScrollViewerViewChangingEventArgs e) => Reveal();
    private void ViewChanged(object? sender, ScrollViewerViewChangedEventArgs e) => Reveal();

    private void PointerEntered(object sender, PointerRoutedEventArgs e)
    {
        _hovered.Add((ScrollBar)sender);
        Reveal();
    }

    private void PointerExited(object sender, PointerRoutedEventArgs e)
    {
        _hovered.Remove((ScrollBar)sender);
        Reveal();
    }

    private void PointerPressed(object sender, PointerRoutedEventArgs e)
    {
        _pressed.Add(e.Pointer.PointerId);
        Reveal();
    }

    private void PointerReleased(object sender, PointerRoutedEventArgs e)
    {
        _pressed.Remove(e.Pointer.PointerId);
        Reveal();
    }

    private void FocusChanged(object sender, RoutedEventArgs e)
    {
        Reveal();
        // LostFocus can precede the final focused element; recheck after focus settles.
        _viewer.DispatcherQueue.TryEnqueue(Reveal);
    }

    private void IndicatorChanged(DependencyObject sender, DependencyProperty property)
    {
        // The native idle transition must not conceal a keyboard-focused scrollbar.
        if (_shown && IsInteracting)
            _viewer.DispatcherQueue.TryEnqueue(() => { if (_loaded && _shown && IsInteracting) Reveal(); });
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args)
    {
        _viewer.DispatcherQueue.TryEnqueue(() =>
        {
            if (!_loaded || _settings.AnimationsEnabled) return;
            _fade?.Stop();
            _fade = null;
            foreach (var bar in _bars) bar.Opacity = _shown ? 1 : 0;
        });
    }
}
