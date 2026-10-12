using Microsoft.UI.Composition;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Services;

/// Native opacity transitions keep image changes off the UI render loop.
internal sealed class ArtworkTransitions
{
    private readonly FrameworkElement _owner;
    private readonly FrameworkElement[] _artwork;
    private readonly UISettings _settings = new();
    private ScalarKeyFrameAnimation? _show;
    private bool _loaded;

    public ArtworkTransitions(FrameworkElement owner, params FrameworkElement[] artwork)
    {
        _owner = owner;
        _artwork = artwork;
        owner.Loaded += (_, _) =>
        {
            if (_loaded) return;
            _loaded = true;
            Configure();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _settings.AnimationsEnabledChanged += AnimationsChanged;
        };
        owner.Unloaded += (_, _) =>
        {
            _loaded = false;
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _settings.AnimationsEnabledChanged -= AnimationsChanged;
            Configure();
        };
        foreach (var image in artwork.OfType<Image>())
            image.ImageOpened += (_, _) =>
            {
                if (_show is not null && image.Visibility == Visibility.Visible)
                    ElementCompositionPreview.GetElementVisual(image).StartAnimation("Opacity", _show);
            };
    }

    public static void SetSource(Image image, string? uri)
    {
        if (uri is null)
        {
            // Retain the bitmap until the compositor has finished its hide animation.
            image.Visibility = Visibility.Collapsed;
            return;
        }
        if ((image.Source as BitmapImage)?.UriSource?.AbsoluteUri != uri)
            image.Source = new BitmapImage(new Uri(uri));
        image.Visibility = Visibility.Visible;
    }

    private void Configure()
    {
        bool animate = _loaded && _settings.AnimationsEnabled;
        _show = null;
        foreach (var image in _artwork)
        {
            var visual = ElementCompositionPreview.GetElementVisual(image);
            ScalarKeyFrameAnimation? hide = null;
            if (animate)
            {
                _show ??= Fade(visual.Compositor, 0, 1, 180);
                hide = Fade(visual.Compositor, 1, 0, 120);
            }
            ElementCompositionPreview.SetImplicitShowAnimation(image, _show);
            ElementCompositionPreview.SetImplicitHideAnimation(image, hide);
            if (!animate) { visual.StopAnimation("Opacity"); visual.Opacity = 1; }
        }
    }

    private static ScalarKeyFrameAnimation Fade(Compositor compositor, float from, float to, int milliseconds)
    {
        var animation = compositor.CreateScalarKeyFrameAnimation();
        animation.Target = "Opacity";
        animation.InsertKeyFrame(0, from);
        animation.InsertKeyFrame(1, to);
        animation.Duration = TimeSpan.FromMilliseconds(milliseconds);
        return animation;
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        _owner.DispatcherQueue.TryEnqueue(() => { if (_loaded) Configure(); });
}
