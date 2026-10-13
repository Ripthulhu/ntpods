using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Effects;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Foundation;
using Windows.UI;

namespace NTPods.WinUI.Controls;

/// Filters the original monochrome artwork at display resolution before composition.
public sealed class ListeningModeIcon : UserControl
{
    public static readonly DependencyProperty UriSourceProperty = DependencyProperty.Register(
        nameof(UriSource), typeof(Uri), typeof(ListeningModeIcon), new PropertyMetadata(null,
            (owner, _) => ((ListeningModeIcon)owner).LoadSource()));

    public Uri? UriSource { get => (Uri?)GetValue(UriSourceProperty); set => SetValue(UriSourceProperty, value); }

    private readonly BitmapIcon _fallback = new() { ShowAsMonochrome = true };
    private readonly Image _image = new() { Stretch = Stretch.Uniform };
    private CanvasBitmap? _bitmap;
    private CanvasDevice? _device;
    private XamlRoot? _root;
    private SolidColorBrush? _brush;
    private long _brushToken;
    private int _generation;
    private (double Width, double Height, float Dpi, Color Color)? _rendered;

    public ListeningModeIcon()
    {
        Content = _fallback;
        IsHitTestVisible = false;
        IsTabStop = false;
        RegisterPropertyChangedCallback(ForegroundProperty, (_, _) => ObserveForeground());
        SizeChanged += (_, _) => Draw();
        Loaded += (_, _) =>
        {
            _root = XamlRoot;
            if (_root is not null) _root.Changed += RootChanged;
            ObserveForeground();
            LoadSource();
        };
        Unloaded += (_, _) =>
        {
            if (_root is not null) _root.Changed -= RootChanged;
            _root = null;
            StopObservingBrush();
            UseFallback();
        };
    }

    private void ObserveForeground()
    {
        _fallback.Foreground = Foreground;
        StopObservingBrush();
        if (IsLoaded && Foreground is SolidColorBrush brush)
        {
            _brush = brush;
            _brushToken = brush.RegisterPropertyChangedCallback(SolidColorBrush.ColorProperty, (_, _) => Draw());
        }
        Draw();
    }

    private void StopObservingBrush()
    {
        _brush?.UnregisterPropertyChangedCallback(SolidColorBrush.ColorProperty, _brushToken);
        _brush = null;
    }

    private async void LoadSource()
    {
        UseFallback();
        _fallback.UriSource = UriSource;
        if (!IsLoaded || UriSource is null) return;
        int generation = _generation;
        CanvasBitmap? bitmap = null;
        try
        {
            string path = System.IO.Path.Combine(AppContext.BaseDirectory, UriSource.AbsolutePath.TrimStart('/'));
            bitmap = await CanvasBitmap.LoadAsync(CanvasDevice.GetSharedDevice(), path);
            if (generation != _generation || !IsLoaded) return;
            _bitmap = bitmap;
            bitmap = null;
            _device = _bitmap.Device;
            _device.DeviceLost += DeviceLost;
            Draw();
        }
        catch (Exception) { if (generation == _generation) UseFallback(); }
        finally { bitmap?.Dispose(); }
    }

    private void Draw()
    {
        if (_bitmap is null || ActualWidth <= 0 || ActualHeight <= 0 || Foreground is not SolidColorBrush brush) return;
        // Two samples per display pixel also preserve detail when the capsule stretches.
        float dpi = (float)(_root?.RasterizationScale ?? 1) * 192;
        var state = (ActualWidth, ActualHeight, dpi, brush.Color);
        if (_rendered == state) return;
        try
        {
            var surface = new CanvasImageSource(_bitmap.Device, (float)ActualWidth, (float)ActualHeight, dpi);
            using var tint = new ColorMatrixEffect
            {
                // Direct2D Straight applies this alpha-weighted matrix without multiplying alpha twice.
                Source = _bitmap, AlphaMode = CanvasAlphaMode.Straight,
                ColorMatrix = new Matrix5x4 { M41 = brush.Color.R / 255f, M42 = brush.Color.G / 255f, M43 = brush.Color.B / 255f, M44 = 1 },
            };
            using (var drawing = surface.CreateDrawingSession(Microsoft.UI.Colors.Transparent))
                drawing.DrawImage(tint, new Rect(0, 0, ActualWidth, ActualHeight),
                    new Rect(0, 0, _bitmap.Size.Width, _bitmap.Size.Height), brush.Color.A / 255f, CanvasImageInterpolation.HighQualityCubic);
            _image.Source = surface;
            Content = _image;
            _rendered = state;
        }
        catch (Exception) { UseFallback(); }
    }

    private void UseFallback()
    {
        ++_generation;
        if (_device is not null) _device.DeviceLost -= DeviceLost;
        _device = null;
        _bitmap?.Dispose();
        _bitmap = null;
        _image.Source = null;
        _rendered = null;
        Content = _fallback;
    }

    private void RootChanged(XamlRoot sender, XamlRootChangedEventArgs args) => Draw();
    private void DeviceLost(CanvasDevice sender, object args) => DispatcherQueue.TryEnqueue(() => { if (IsLoaded && sender == _device) LoadSource(); });
}
