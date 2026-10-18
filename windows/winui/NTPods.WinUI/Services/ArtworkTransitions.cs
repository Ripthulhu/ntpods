using Microsoft.UI.Composition;
using System.Runtime.CompilerServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Services;

/// Native opacity transitions keep image changes off the UI render loop.
internal sealed class ArtworkTransitions
{
    private readonly FrameworkElement _owner;
    private readonly FrameworkElement[] _artwork;
    private readonly UISettings _settings = new();
    private sealed class SourceState
    {
        public string? CurrentUri;
        public SourceRequest? Pending;
    }
    private sealed record SourceRequest(string Uri, BitmapImage Bitmap);
    private static readonly ConditionalWeakTable<Image, SourceState> Sources = new();
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
    }

    public static void SetSource(Image image, string? uri)
    {
        var state = Sources.GetValue(image, _ => new SourceState());
        if (uri is null)
        {
            state.Pending = null;
            // Retain the bitmap until the compositor has finished its hide animation.
            image.Visibility = Visibility.Collapsed;
            return;
        }
        if ((state.CurrentUri == uri && image.Source is not null) || (image.Source as BitmapImage)?.UriSource?.AbsoluteUri == uri)
        {
            state.Pending = null;
            image.Visibility = Visibility.Visible;
            return;
        }
        if (state.Pending?.Uri == uri) return;

        // Decode before replacement so visible artwork never becomes an empty image.
        // A newer request (including hiding the image) owns the final result.
        var request = new SourceRequest(uri, new BitmapImage());
        state.Pending = request;
        _ = LoadSourceAsync(image, state, request);
    }

    public static void CancelPendingSource(Image image)
    {
        if (Sources.TryGetValue(image, out var state)) state.Pending = null;
    }

    private static async Task LoadSourceAsync(Image image, SourceState state, SourceRequest request)
    {
        try
        {
            // Bundled ms-appx artwork lives beside the unpackaged executable. UriSource
            // alone does not decode an unattached BitmapImage; the stream forces it.
            string relativePath = Uri.UnescapeDataString(new Uri(request.Uri).AbsolutePath).TrimStart('/');
            string path = System.IO.Path.GetFullPath(System.IO.Path.Combine(AppContext.BaseDirectory, relativePath));
            var file = await StorageFile.GetFileFromPathAsync(path);
            using var stream = await file.OpenReadAsync();
            if (!ReferenceEquals(state.Pending, request)) return;
            await request.Bitmap.SetSourceAsync(stream);
            if (!ReferenceEquals(state.Pending, request)) return;
            image.Source = request.Bitmap;
            state.CurrentUri = request.Uri;
            state.Pending = null;
            image.Visibility = Visibility.Visible;
        }
        catch (Exception)
        {
            // A missing or undecodable replacement leaves known artwork intact.
            if (ReferenceEquals(state.Pending, request)) state.Pending = null;
        }
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
                _show ??= Fade(visual.Compositor, 1, 180);
                hide = Fade(visual.Compositor, 0, 120);
            }
            ElementCompositionPreview.SetImplicitShowAnimation(image, _show);
            ElementCompositionPreview.SetImplicitHideAnimation(image, hide);
            visual.StopAnimation("Opacity");
            visual.Opacity = animate && image.Visibility == Visibility.Collapsed ? 0 : 1;
        }
    }

    private static ScalarKeyFrameAnimation Fade(Compositor compositor, float to, int milliseconds)
    {
        var animation = compositor.CreateScalarKeyFrameAnimation();
        animation.Target = "Opacity";
        animation.InsertExpressionKeyFrame(0, "this.StartingValue");
        animation.InsertKeyFrame(1, to);
        animation.Duration = TimeSpan.FromMilliseconds(milliseconds);
        return animation;
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        _owner.DispatcherQueue.TryEnqueue(() => { if (_loaded) Configure(); });
}
