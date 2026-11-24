using System.Diagnostics;
using System.Numerics;
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Effects;
using Microsoft.Graphics.Canvas.Geometry;
using Microsoft.Graphics.Canvas.UI;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics.DirectX;
using Windows.UI;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Controls;

/// Draws the CT thumb only; the native Slider owns input, focus and RangeValue automation.
public sealed class TransparencySliderThumb : Grid
{
    private const float Overflow = 16;
    public static readonly DependencyProperty PressedProperty = DependencyProperty.Register(nameof(Pressed), typeof(bool),
        typeof(TransparencySliderThumb), new PropertyMetadata(false, (owner, _) => ((TransparencySliderThumb)owner).Press()));
    public static readonly DependencyProperty MutedProperty = DependencyProperty.Register(nameof(Muted), typeof(bool),
        typeof(TransparencySliderThumb), new PropertyMetadata(false, (owner, _) => ((TransparencySliderThumb)owner).Invalidate()));
    public bool Pressed { get => (bool)GetValue(PressedProperty); set => SetValue(PressedProperty, value); }
    public bool Muted { get => (bool)GetValue(MutedProperty); set => SetValue(MutedProperty, value); }

    private readonly Border _fallback = new() { CornerRadius = new(12), Background = new SolidColorBrush(Colors.White) };
    private readonly UISettings _settings = new();
    private readonly AccessibilitySettings _accessibility = new();
    private readonly ListeningModeMotion _motion = new();
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly List<IDisposable> _resources = [];
    private CanvasControl? _canvas;
    private Slider? _slider;
    private CanvasRenderTarget? _source;
    private CanvasGeometry? _shape;
    private CanvasGeometry? _rim;
    private ICanvasImage? _lens;
    private GaussianBlurEffect? _blur;
    private readonly List<DisplacementMapEffect> _displacements = [];
    private Transform2DEffect? _highlight;
    private ShadowEffect? _shadow;
    private (bool Dark, bool Muted, int Endpoint)? _sourceState;
    private double _progress;
    private bool _rendering, _failed;
    private bool Animate => OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041) && _settings.AnimationsEnabled && !_accessibility.HighContrast;

    public TransparencySliderThumb()
    {
        Width = TransparencySliderMetrics.Width;
        Height = TransparencySliderMetrics.Height;
        IsHitTestVisible = false;
        Children.Add(_fallback);
        Loaded += (_, _) => Attach();
        Unloaded += (_, _) => Detach();
        ActualThemeChanged += (_, _) => { _sourceState = null; Invalidate(); };
    }

    private void Attach()
    {
        if (_canvas is not null) return;
        for (DependencyObject? parent = VisualTreeHelper.GetParent(this); parent is not null; parent = VisualTreeHelper.GetParent(parent))
            if (parent is Slider slider) { _slider = slider; break; }
        if (_slider is not null) _slider.ValueChanged += ValueChanged;
        _settings.ColorValuesChanged += ColorsChanged;
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041)) _settings.AnimationsEnabledChanged += AnimationsChanged;
        _failed = false;
        _canvas = new CanvasControl { Margin = new Thickness(-Overflow), IsHitTestVisible = false, ClearColor = Colors.Transparent };
        _canvas.CreateResources += CreateResources;
        _canvas.Draw += Draw;
        Children.Add(_canvas);
        Press();
    }

    private void Detach()
    {
        StopFrames();
        _motion.ResetInteraction();
        if (_slider is not null) _slider.ValueChanged -= ValueChanged;
        _slider = null;
        _settings.ColorValuesChanged -= ColorsChanged;
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041)) _settings.AnimationsEnabledChanged -= AnimationsChanged;
        if (_canvas is not null)
        {
            _canvas.CreateResources -= CreateResources;
            _canvas.Draw -= Draw;
            Children.Remove(_canvas);
            _canvas.RemoveFromVisualTree();
            _canvas = null;
        }
        DisposeResources();
        _fallback.Opacity = 1;
    }

    private void Press()
    {
        if (!IsLoaded) return;
        _motion.Press(Pressed, _clock.Elapsed.TotalMilliseconds, Animate);
        if (!Animate) { _progress = Pressed ? 1 : 0; StopFrames(); }
        else if (!_rendering && _motion.IsMoving) { _rendering = true; CompositionTarget.Rendering += Frame; }
        Invalidate();
    }

    private void Frame(object? sender, object args)
    {
        _motion.Tick(_clock.Elapsed.TotalMilliseconds);
        _progress = Math.Clamp((_motion.ScaleX - 1) / .2, 0, 1);
        Invalidate();
        if (!_motion.IsMoving) StopFrames();
    }
    private void StopFrames() { if (_rendering) CompositionTarget.Rendering -= Frame; _rendering = false; }
    private void Invalidate()
    {
        if (_fallback.Background is SolidColorBrush fallback)
            fallback.Color = _accessibility.HighContrast ? _settings.GetColorValue(UIColorType.Foreground) : Colors.White;
        _canvas?.Invalidate();
    }
    private void ValueChanged(object sender, RangeBaseValueChangedEventArgs args) => Invalidate();
    private void ColorsChanged(UISettings sender, object args) => DispatcherQueue.TryEnqueue(() => { _sourceState = null; Invalidate(); });
    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) => DispatcherQueue.TryEnqueue(Press);
    private T Keep<T>(T resource) where T : IDisposable { _resources.Add(resource); return resource; }

    private void CreateResources(CanvasControl sender, CanvasCreateResourcesEventArgs args)
    {
        DisposeResources();
        try
        {
            _source = Keep(new CanvasRenderTarget(sender, 40 + 2 * Overflow, 24 + 2 * Overflow, sender.Dpi));
            _shape = Keep(CanvasGeometry.CreateRoundedRectangle(sender, Overflow, Overflow, 40, 24, 12, 12));
            using var inner = CanvasGeometry.CreateRoundedRectangle(sender, Overflow + .75f, Overflow + .75f, 38.5f, 22.5f, 11.25f, 11.25f);
            _rim = Keep(_shape.CombineWith(inner, Matrix3x2.Identity, CanvasGeometryCombine.Exclude));
            var silhouette = Keep(new CanvasCommandList(sender));
            using (var drawing = silhouette.CreateDrawingSession()) drawing.FillGeometry(_shape, Colors.White);
            _shadow = Keep(new ShadowEffect { Source = silhouette, BlurAmount = 4 * .57735f + .5f, ShadowColor = Color.FromArgb(13, 0, 0, 0) });

            // Reuse the app's attributed AndroidLiquidGlass field at the thumb's 40-by-24 scale.
            var raster = ListeningModeLens.Create(90, 96);
            var transform = Matrix3x2.CreateScale(24 / ListeningModeLens.Height) * Matrix3x2.CreateTranslation(Overflow, Overflow);
            _blur = Keep(new GaussianBlurEffect { Source = _source, BlurAmount = 8 });
            ICanvasImage? sum = null;
            for (int sample = 0; sample < 7; sample++)
            {
                var bitmap = Keep(CanvasBitmap.CreateFromBytes(sender, raster.Maps[sample], raster.PixelWidth, raster.PixelHeight,
                    DirectXPixelFormat.R32G32B32A32Float, 96, CanvasAlphaMode.Premultiplied));
                var map = Keep(new Transform2DEffect { Source = bitmap, TransformMatrix = transform });
                var displaced = Keep(new DisplacementMapEffect { Source = _blur, Displacement = map, Amount = ListeningModeLens.MapAmount * 24 / ListeningModeLens.Height,
                    XChannelSelect = EffectChannelSelect.Red, YChannelSelect = EffectChannelSelect.Green });
                _displacements.Add(displaced);
                var weight = ListeningModeLens.Weights[sample];
                var spectral = Keep(new ColorMatrixEffect { Source = displaced, AlphaMode = CanvasAlphaMode.Straight, ClampOutput = false,
                    ColorMatrix = new Matrix5x4 { M11 = weight.X, M22 = weight.Y, M33 = weight.Z, M44 = weight.W } });
                sum = sum is null ? spectral : Keep(new ArithmeticCompositeEffect { Source1 = sum, Source2 = spectral, Source1Amount = 1, Source2Amount = 1, MultiplyAmount = 0 });
            }
            _lens = sum;
            var light = Keep(CanvasBitmap.CreateFromBytes(sender, raster.Highlight, raster.PixelWidth, raster.PixelHeight,
                DirectXPixelFormat.B8G8R8A8UIntNormalized, 96, CanvasAlphaMode.Premultiplied));
            _highlight = Keep(new Transform2DEffect { Source = light, TransformMatrix = transform });
            _failed = false;
            sender.Opacity = 1;
            Press();
        }
        catch (Exception error) { Fail(sender, error); }
    }

    private void Draw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        if (_failed || _source is null || _shape is null || _lens is null || _shadow is null || _highlight is null || _blur is null) return;
        try
        {
            bool dark = ActualTheme == ElementTheme.Dark;
            var drawing = args.DrawingSession;
            if (_accessibility.HighContrast)
            {
                drawing.FillGeometry(_shape, _settings.GetColorValue(UIColorType.Foreground));
                drawing.DrawGeometry(_shape, _settings.GetColorValue(UIColorType.Background), 1.5f);
                _fallback.Opacity = 0;
                return;
            }
            double fraction = _slider is { Maximum: var max, Minimum: var min } && max > min ? (_slider.Value - min) / (max - min) : .5;
            int endpoint = fraction <= 0 ? -1 : fraction >= 1 ? 1 : 0;
            var state = (dark, Muted, endpoint);
            if (_sourceState != state)
            {
                _sourceState = state;
                using var source = _source.CreateDrawingSession();
                source.Clear(dark ? Color.FromArgb(255, 28, 28, 30) : Colors.White);
                var track = dark ? Color.FromArgb(92, 120, 120, 128) : Color.FromArgb(51, 120, 120, 120);
                source.FillRectangle(0, Overflow + 9, 72, 6, track);
                if (!Muted && endpoint != -1) source.FillRectangle(0, Overflow + 9, endpoint == 1 ? 72 : Overflow + 20, 6,
                    dark ? Color.FromArgb(255, 0, 145, 255) : Color.FromArgb(255, 0, 136, 255));
            }
            float progress = Muted ? 0 : (float)_progress;
            _blur!.BlurAmount = 8 * (1 - progress);
            foreach (var effect in _displacements) effect.Amount = ListeningModeLens.MapAmount * 24 / ListeningModeLens.Height * progress;
            float scale = Animate ? TransparencySliderMetrics.Scale(progress) : 1;
            drawing.Transform = Matrix3x2.CreateScale(scale, new Vector2(Overflow + 20, Overflow + 12));
            drawing.DrawImage(_shadow, 0, 2);
            using (drawing.CreateLayer(progress, _shape)) drawing.DrawImage(_lens);
            drawing.FillGeometry(_shape, Muted ? (dark ? Color.FromArgb(92, 120, 120, 128) : Color.FromArgb(51, 120, 120, 120)) : Color.FromArgb((byte)(255 * (1 - progress)), 255, 255, 255));
            using (drawing.CreateLayer(progress, _rim)) drawing.DrawImage(_highlight);
            if (progress > 0) drawing.DrawGeometry(_shape, Color.FromArgb((byte)(24 * progress), 0, 0, 0), .75f);
            _fallback.Opacity = 0;
        }
        catch (Exception error) { Fail(sender, error); }
    }

    private void Fail(CanvasControl sender, Exception error)
    {
        _fallback.Opacity = 1;
        sender.Opacity = 0;
        StopFrames();
        if (sender.Device.IsDeviceLost(error.HResult)) sender.Device.RaiseDeviceLost();
        else _failed = true;
    }
    private void DisposeResources()
    {
        foreach (var resource in _resources) resource.Dispose();
        _resources.Clear();
        _displacements.Clear();
        _source = null; _shape = _rim = null; _lens = null; _blur = null; _highlight = null; _shadow = null; _sourceState = null;
    }
}
