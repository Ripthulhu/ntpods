using System.Numerics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Hosting;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Services;

/// Animates visibility-driven reflow once per state change, without observing expander frames.
internal sealed class SectionTransitions
{
    private readonly FrameworkElement _owner;
    private readonly FrameworkElement[] _elements;
    private readonly (bool Visible, double Y)[] _before;
    private readonly UISettings _settings = new();

    public SectionTransitions(FrameworkElement owner, params Panel[] panels)
    {
        _owner = owner;
        _elements = panels.SelectMany(panel => panel.Children.OfType<FrameworkElement>()).ToArray();
        _before = new (bool, double)[_elements.Length];
        owner.Loaded += (_, _) =>
        {
            Configure();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _settings.AnimationsEnabledChanged += AnimationsChanged;
        };
        owner.Unloaded += (_, _) =>
        {
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _settings.AnimationsEnabledChanged -= AnimationsChanged;
            Configure();
        };
    }

    public void Capture()
    {
        for (int i = 0; i < _elements.Length; i++)
            _before[i] = (_elements[i].Visibility == Visibility.Visible, LayoutInformation.GetLayoutSlot(_elements[i]).Y);
    }

    public bool AnimateChanges(bool nestedChange = false)
    {
        bool changed = nestedChange;
        for (int i = 0; i < _elements.Length; i++)
            changed |= _before[i].Visible != (_elements[i].Visibility == Visibility.Visible);
        if (!changed || !_owner.IsLoaded || !_settings.AnimationsEnabled) return changed;

        // One layout resolves the new slots. Regular snapshots and expander frames skip this.
        _owner.UpdateLayout();
        for (int i = 0; i < _elements.Length; i++)
        {
            var element = _elements[i];
            if (!_before[i].Visible || element.Visibility != Visibility.Visible) continue;
            float shift = (float)(_before[i].Y - LayoutInformation.GetLayoutSlot(element).Y);
            if (Math.Abs(shift) < 0.1f) continue;
            var visual = ElementCompositionPreview.GetElementVisual(element);
            var move = visual.Compositor.CreateVector3KeyFrameAnimation();
            move.SetVector3Parameter("shift", new Vector3(0, shift, 0));
            move.InsertExpressionKeyFrame(0, "this.StartingValue + shift");
            move.InsertKeyFrame(1, Vector3.Zero, visual.Compositor.CreateCubicBezierEasingFunction(new(0.16f, 1), new(0.3f, 1)));
            move.Duration = TimeSpan.FromMilliseconds(180);
            visual.StartAnimation("Translation", move);
        }
        return changed;
    }

    private void Configure()
    {
        bool animate = _owner.IsLoaded && _settings.AnimationsEnabled;
        foreach (var element in _elements)
        {
            ElementCompositionPreview.SetIsTranslationEnabled(element, true);
            var visual = ElementCompositionPreview.GetElementVisual(element);
            visual.StopAnimation("Translation");
            visual.Properties.InsertVector3("Translation", Vector3.Zero);
            visual.StopAnimation("Opacity");
            visual.Opacity = animate && element.Visibility == Visibility.Collapsed ? 0 : 1;
            var show = animate ? visual.Compositor.CreateScalarKeyFrameAnimation() : null;
            var hide = animate ? visual.Compositor.CreateScalarKeyFrameAnimation() : null;
            if (show is not null && hide is not null)
            {
                show.Target = hide.Target = "Opacity";
                show.InsertExpressionKeyFrame(0, "this.StartingValue");
                show.InsertKeyFrame(1, 1);
                hide.InsertExpressionKeyFrame(0, "this.StartingValue");
                hide.InsertKeyFrame(1, 0);
                show.Duration = TimeSpan.FromMilliseconds(180);
                hide.Duration = TimeSpan.FromMilliseconds(120);
            }
            ElementCompositionPreview.SetImplicitShowAnimation(element, show);
            ElementCompositionPreview.SetImplicitHideAnimation(element, hide);
        }
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        _owner.DispatcherQueue.TryEnqueue(() => { if (_owner.IsLoaded) Configure(); });
}
