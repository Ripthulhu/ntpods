using System.Numerics;
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Effects;
using Microsoft.Graphics.Canvas.Geometry;
using Microsoft.Graphics.Canvas.UI;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Foundation;
using Windows.Graphics.DirectX;
using Windows.UI;

namespace NTPods.WinUI.Controls;

/// GPU lens artwork only. NoiseControlCard retains input, focus and device state.
internal sealed class ListeningModeGlass : Grid
{
    private const float Overflow = 176; // Covers 1.8x stretch plus the shadow at the 640-DIP track maximum.
    private CanvasControl? _canvas;
    private CanvasBitmap[]? _icons;
    private ColorMatrixEffect[]? _tintedIcons;
    private CanvasStrokeStyle? _markerStroke;
    private bool? _iconTintDark;
    private CanvasRenderTarget? _source;
    private readonly List<IDisposable> _effects = [];
    private readonly List<IDisposable> _mapResources = [];
    private Transform2DEffect? _sourcePosition;
    private Transform2DEffect[][]? _maps;
    private Transform2DEffect[]? _highlights;
    private ArithmeticCompositeEffect[]? _mapMix;
    private ArithmeticCompositeEffect? _highlightMix;
    private ICanvasImage? _lens;
    private GaussianBlurEffect? _highlightBlur;
    private ShadowEffect? _shadow;
    private CanvasCommandList? _silhouette;
    private CanvasGeometry? _shape, _rim, _outside;
    private Snapshot _snapshot;
    private Snapshot? _sourceSnapshot;
    private float[] _mapWidths = [];
    private float _mapTrackWidth, _mapDpi, _shapeWidth;
    private float _mapSegment = float.NaN;
    private int _mapPair = -1;
    private int _generation;
    private bool _loadingMaps, _resourcesReady, _failed;

    private readonly record struct Snapshot(ListeningModeLayout Layout, double Left, uint Mode, double ScaleX, double ScaleY, bool Dark, bool HighContrast);

    internal bool IsReady { get; private set; }
    internal event EventHandler? ReadinessChanged;

    internal ListeningModeGlass()
    {
        IsHitTestVisible = false;
        Height = ListeningModeLens.Height;
        Loaded += (_, _) => Attach();
        Unloaded += (_, _) => Detach();
    }

    internal void Update(ListeningModeLayout layout, double left, uint mode, double scaleX, double scaleY, bool darkTheme, bool highContrast)
    {
        var next = new Snapshot(layout, left, mode, scaleX, scaleY, darkTheme, highContrast);
        if (_snapshot == next) return;
        _snapshot = next;
        if (highContrast) SetReady(false);
        _canvas?.Invalidate();
    }

    private void Attach()
    {
        if (_canvas != null) return;
        _failed = false;
        _canvas = new CanvasControl { IsHitTestVisible = false, ClearColor = Colors.Transparent, Margin = new Thickness(-Overflow) };
        _canvas.CreateResources += CreateResources;
        _canvas.Draw += Draw;
        Children.Add(_canvas);
    }

    private void Detach()
    {
        _generation++;
        SetReady(false);
        if (_canvas != null)
        {
            _canvas.CreateResources -= CreateResources;
            _canvas.Draw -= Draw;
            Children.Remove(_canvas);
            _canvas.RemoveFromVisualTree();
            _canvas = null;
        }
        DisposeResources();
    }

    private void SetReady(bool ready)
    {
        if (IsReady == ready) return;
        IsReady = ready;
        ReadinessChanged?.Invoke(this, EventArgs.Empty);
    }

    private async void CreateResources(CanvasControl sender, CanvasCreateResourcesEventArgs args)
    {
        if (args.Reason == CanvasCreateResourcesReason.DpiChanged && _resourcesReady && !_failed)
        {
            // DPI changes keep the same device; redraw with the existing maps until replacements are ready.
            _sourceSnapshot = null;
            _shapeWidth = float.NaN;
            sender.Invalidate();
            return;
        }
        int generation = ++_generation;
        SetReady(false);
        DisposeResources();
        _failed = false;
        var loaded = new List<CanvasBitmap>();
        try
        {
            foreach (string name in new[] { "ic_transparency.png", "ic_adaptive.png", "ic_noise_cancellation.png" })
                loaded.Add(await CanvasBitmap.LoadAsync(sender, System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", name)));
            if (generation != _generation || sender != _canvas) { foreach (var item in loaded) item.Dispose(); return; }
            _icons = loaded.ToArray();
            _tintedIcons = new ColorMatrixEffect[_icons.Length];
            for (int index = 0; index < _icons.Length; index++)
                _tintedIcons[index] = new ColorMatrixEffect { Source = _icons[index], AlphaMode = CanvasAlphaMode.Straight };
            _markerStroke = new CanvasStrokeStyle { CustomDashStyle = [1, 2] };
            _resourcesReady = true;
            sender.Invalidate();
        }
        catch (Exception error)
        {
            foreach (var item in loaded) item.Dispose();
            if (generation == _generation) RecoverOrFail(sender, error);
        }
    }

    private void Draw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        var snapshot = _snapshot;
        if (_failed || !_resourcesReady || snapshot.HighContrast || snapshot.Mode is < 1 or > 4 ||
            snapshot.Layout.Width <= 0 || snapshot.Layout.Segment <= 0)
        { SetReady(false); return; }
        try
        {
            float width = (float)snapshot.Layout.Width;
            if (_maps == null || Math.Abs(width - _mapTrackWidth) > .1 || sender.Dpi != _mapDpi)
            {
                PrepareMaps(sender, width, sender.Dpi);
                // Keep drawing with the last maps while resized replacements are prepared.
                if (_maps == null) { SetReady(false); return; }
            }
            EnsureSource(sender, snapshot);
            float segment = (float)snapshot.Layout.Segment;
            EnsureShape(sender, segment);
            UpdateMapTransforms(segment);
            _sourcePosition!.TransformMatrix = Matrix3x2.CreateTranslation(-Overflow - (float)snapshot.Left, -Overflow);
            var drawing = args.DrawingSession;
            drawing.Transform = Matrix3x2.CreateScale((float)snapshot.ScaleX, (float)snapshot.ScaleY, new Vector2(segment / 2, ListeningModeLens.Height / 2)) *
                Matrix3x2.CreateTranslation(Overflow + (float)snapshot.Left, Overflow);
            using (drawing.CreateLayer(1, _outside)) drawing.DrawImage(_shadow, 0, 4);
            using (drawing.CreateLayer(1, _shape)) drawing.DrawImage(_lens);
            using (drawing.CreateLayer(1, _rim))
            {
                drawing.Blend = CanvasBlend.Add;
                drawing.DrawImage(_highlightBlur);
                drawing.Blend = CanvasBlend.SourceOver;
            }
            SetReady(true);
        }
        catch (Exception error)
        {
            if (sender.Device.IsDeviceLost(error.HResult)) throw;
            _failed = true;
            SetReady(false);
            System.Diagnostics.Debug.WriteLine($"Listening glass rendering unavailable: {error.GetType().Name} (0x{error.HResult:X8})");
        }
    }

    private async void PrepareMaps(CanvasControl sender, float width, float dpi)
    {
        if (_loadingMaps) return;
        _loadingMaps = true;
        int generation = _generation;
        try
        {
            var widths = new[] { width / 5, width / 4, width / 3 };
            var rasters = await Task.Run(() => widths.Select(value => ListeningModeLens.Create(value, dpi)).ToArray());
            if (generation != _generation || sender != _canvas) return;
            if (Math.Abs(_snapshot.Layout.Width - width) > .1 || sender.Dpi != dpi) return;
            DisposeMaps();
            _mapTrackWidth = width;
            _mapDpi = dpi;
            _mapWidths = widths;
            _maps = new Transform2DEffect[3][];
            _highlights = new Transform2DEffect[3];
            for (int size = 0; size < 3; size++)
            {
                var raster = rasters[size];
                _maps[size] = new Transform2DEffect[7];
                for (int sample = 0; sample < 7; sample++)
                {
                    var bitmap = KeepMap(CanvasBitmap.CreateFromBytes(sender, raster.Maps[sample], raster.PixelWidth, raster.PixelHeight,
                        DirectXPixelFormat.R32G32B32A32Float, dpi, CanvasAlphaMode.Premultiplied));
                    _maps[size][sample] = KeepMap(new Transform2DEffect { Source = bitmap });
                }
                var light = KeepMap(CanvasBitmap.CreateFromBytes(sender, raster.Highlight, raster.PixelWidth, raster.PixelHeight,
                    DirectXPixelFormat.B8G8R8A8UIntNormalized, dpi, CanvasAlphaMode.Premultiplied));
                _highlights[size] = KeepMap(new Transform2DEffect { Source = light });
            }
            BuildGraph();
            _sourceSnapshot = null;
        }
        catch (Exception error)
        {
            if (generation == _generation) RecoverOrFail(sender, error);
        }
        finally
        {
            if (generation == _generation)
            {
                _loadingMaps = false;
                sender.Invalidate();
            }
        }
    }

    private void RecoverOrFail(CanvasControl sender, Exception error)
    {
        SetReady(false);
        if (sender.Device.IsDeviceLost(error.HResult)) sender.Device.RaiseDeviceLost();
        else
        {
            _failed = true;
            System.Diagnostics.Debug.WriteLine($"Listening glass resources unavailable: {error.GetType().Name} (0x{error.HResult:X8})");
        }
    }

    private T KeepMap<T>(T value) where T : IDisposable { _mapResources.Add(value); return value; }
    private T KeepEffect<T>(T value) where T : IDisposable { _effects.Add(value); return value; }

    private void BuildGraph()
    {
        _sourcePosition = KeepEffect(new Transform2DEffect());
        _mapMix = new ArithmeticCompositeEffect[7];
        ICanvasImage? sum = null;
        for (int sample = 0; sample < 7; sample++)
        {
            _mapMix[sample] = KeepEffect(new ArithmeticCompositeEffect { MultiplyAmount = 0, BufferPrecision = CanvasBufferPrecision.Precision32Float });
            var displaced = KeepEffect(new DisplacementMapEffect
            {
                Source = _sourcePosition, Displacement = _mapMix[sample], Amount = ListeningModeLens.MapAmount,
                XChannelSelect = EffectChannelSelect.Red, YChannelSelect = EffectChannelSelect.Green,
                BufferPrecision = CanvasBufferPrecision.Precision16Float
            });
            var weight = ListeningModeLens.Weights[sample];
            var spectral = KeepEffect(new ColorMatrixEffect
            {
                Source = displaced, AlphaMode = CanvasAlphaMode.Straight, ClampOutput = false,
                ColorMatrix = new Matrix5x4 { M11 = weight.X, M22 = weight.Y, M33 = weight.Z, M44 = weight.W },
                BufferPrecision = CanvasBufferPrecision.Precision16Float
            });
            // Straight applies these weights directly to the premultiplied samples.
            sum = sum == null ? spectral : KeepEffect(new ArithmeticCompositeEffect
            {
                Source1 = sum, Source2 = spectral, Source1Amount = 1, Source2Amount = 1, MultiplyAmount = 0,
                ClampOutput = false, BufferPrecision = CanvasBufferPrecision.Precision16Float
            });
        }
        _lens = sum;
        _highlightMix = KeepEffect(new ArithmeticCompositeEffect { MultiplyAmount = 0 });
        _highlightBlur = KeepEffect(new GaussianBlurEffect { Source = _highlightMix, BlurAmount = .25f });
        _shadow = KeepEffect(new ShadowEffect { BlurAmount = 24 * .57735f + .5f, ShadowColor = Color.FromArgb(26, 0, 0, 0) });
    }

    private void UpdateMapTransforms(float segment)
    {
        if (_mapSegment == segment) return;
        int lower = segment <= _mapWidths[1] ? 0 : 1;
        float ratio = Math.Clamp((segment - _mapWidths[lower]) / (_mapWidths[lower + 1] - _mapWidths[lower]), 0, 1);
        // shortcut: width springs interpolate exact resting fields; use a custom shader if every intermediate shape must be exact.
        for (int size = lower; size <= lower + 1; size++)
        {
            var bitmap = (CanvasBitmap)_maps![size][0].Source;
            var transform = Matrix3x2.CreateScale(segment / (float)bitmap.Size.Width, ListeningModeLens.Height / (float)bitmap.Size.Height);
            foreach (var map in _maps[size]) map.TransformMatrix = transform;
            _highlights![size].TransformMatrix = transform;
        }
        for (int sample = 0; sample < 7; sample++)
        {
            if (_mapPair != lower)
            {
                _mapMix![sample].Source1 = _maps![lower][sample];
                _mapMix[sample].Source2 = _maps[lower + 1][sample];
            }
            _mapMix![sample].Source1Amount = 1 - ratio;
            _mapMix[sample].Source2Amount = ratio;
        }
        if (_mapPair != lower)
        {
            _highlightMix!.Source1 = _highlights![lower];
            _highlightMix.Source2 = _highlights[lower + 1];
        }
        _highlightMix!.Source1Amount = 1 - ratio;
        _highlightMix.Source2Amount = ratio;
        _mapPair = lower;
        _mapSegment = segment;
    }

    private void EnsureSource(CanvasControl sender, Snapshot snapshot)
    {
        static double MarkerOpacity(Snapshot state, bool maximum) => state.Layout.MarkerOpacity(
            maximum ? state.Layout.AncStart - 22.5 : state.Layout.AdaptiveStart + 22.5, state.Left, state.ScaleX);
        if (_sourceSnapshot is { } previous && previous.Layout == snapshot.Layout && previous.Mode == snapshot.Mode && previous.Dark == snapshot.Dark && _source != null &&
            MarkerOpacity(previous, false) == MarkerOpacity(snapshot, false) && MarkerOpacity(previous, true) == MarkerOpacity(snapshot, true))
            return;
        var layout = snapshot.Layout;
        if (_source == null || Math.Abs(_source.Size.Width - layout.Width - 2 * Overflow) > 1 || _source.Dpi != sender.Dpi)
        {
            _source?.Dispose();
            _source = new CanvasRenderTarget(sender, (float)layout.Width + 2 * Overflow, ListeningModeLens.Height + 2 * Overflow, sender.Dpi);
        }
        using var drawing = _source.CreateDrawingSession();
        drawing.Clear(Colors.Transparent);
        drawing.Transform = Matrix3x2.CreateTranslation(Overflow, Overflow);
        var foreground = snapshot.Dark ? Colors.White : Colors.Black;
        if (_iconTintDark != snapshot.Dark)
        {
            var tint = new Matrix5x4 { M41 = foreground.R / 255f, M42 = foreground.G / 255f, M43 = foreground.B / 255f, M44 = 1 };
            foreach (var icon in _tintedIcons!) icon.ColorMatrix = tint;
            _iconTintDark = snapshot.Dark;
        }
        var track = snapshot.Dark ? Color.FromArgb(255, 28, 28, 30) : Color.FromArgb(255, 227, 227, 232);
        drawing.FillRoundedRectangle(-4, -4, (float)layout.Width + 8, ListeningModeLens.Height + 8, 30, 30, track);
        if (snapshot.Mode == 4)
        {
            drawing.FillRoundedRectangle((float)layout.AdaptiveStart, 0, (float)layout.AdaptiveWidth, ListeningModeLens.Height, 27, 27,
                snapshot.Dark ? Color.FromArgb(64, 136, 136, 136) : Color.FromArgb(64, 255, 255, 255));
            var minimumColor = foreground;
            minimumColor.A = (byte)Math.Round(255 * MarkerOpacity(snapshot, false));
            var maximumColor = foreground;
            maximumColor.A = (byte)Math.Round(255 * MarkerOpacity(snapshot, true));
            drawing.DrawCircle((float)layout.AdaptiveStart + 22.5f, 27, 5.75f, minimumColor, 1.5f, _markerStroke);
            drawing.DrawCircle((float)layout.AncStart - 22.5f, 27, 5.75f, maximumColor, 1.5f);
        }
        if (layout.OffWidth > 0 && snapshot.Mode != 1) DrawIcon(drawing, 2, layout.OffWidth / 2);
        if (snapshot.Mode != 3) DrawIcon(drawing, 0, layout.OffWidth + layout.Segment / 2);
        if (layout.HasAdaptive && snapshot.Mode != 4) DrawIcon(drawing, 1, layout.AdaptiveStart + layout.AdaptiveWidth / 2);
        if (snapshot.Mode != 2) DrawIcon(drawing, 2, layout.AncStart + layout.Segment / 2);
        _sourcePosition!.Source = _source;
        _sourceSnapshot = snapshot;
    }

    private void DrawIcon(CanvasDrawingSession drawing, int index, double center)
    {
        var size = _icons![index].Size;
        drawing.DrawImage(_tintedIcons![index], new Rect(center - 14, 13, 28, 28), new Rect(0, 0, size.Width, size.Height),
            1, CanvasImageInterpolation.HighQualityCubic);
    }

    private void EnsureShape(CanvasControl sender, float width)
    {
        if (_shape != null && Math.Abs(_shapeWidth - width) < .001) return;
        DisposeShape();
        _shapeWidth = width;
        _shape = CanvasGeometry.CreateRoundedRectangle(sender, 0, 0, width, ListeningModeLens.Height, 27, 27);
        float inset = MathF.Ceiling(.5f * sender.Dpi / 96) * 96 / sender.Dpi;
        using var inside = CanvasGeometry.CreateRoundedRectangle(sender, inset, inset, Math.Max(0, width - 2 * inset), ListeningModeLens.Height - 2 * inset, 27 - inset, 27 - inset);
        _rim = _shape.CombineWith(inside, Matrix3x2.Identity, CanvasGeometryCombine.Exclude);
        using var bounds = CanvasGeometry.CreateRectangle(sender, -Overflow, -Overflow, width + 2 * Overflow, ListeningModeLens.Height + 2 * Overflow);
        _outside = bounds.CombineWith(_shape, Matrix3x2.Identity, CanvasGeometryCombine.Exclude);
        _silhouette = new CanvasCommandList(sender);
        using (var drawing = _silhouette.CreateDrawingSession()) drawing.FillGeometry(_shape, Colors.White);
        _shadow!.Source = _silhouette;
    }

    private void DisposeShape()
    {
        _shape?.Dispose(); _rim?.Dispose(); _outside?.Dispose(); _silhouette?.Dispose();
        _shape = _rim = _outside = null;
        _silhouette = null;
    }

    private void DisposeMaps()
    {
        DisposeShape();
        foreach (var resource in _effects) resource.Dispose();
        foreach (var resource in _mapResources) resource.Dispose();
        _effects.Clear(); _mapResources.Clear();
        _maps = null; _highlights = null; _lens = null;
        _mapSegment = float.NaN;
        _mapPair = -1;
    }

    private void DisposeResources()
    {
        DisposeMaps();
        _source?.Dispose(); _source = null;
        if (_tintedIcons != null) foreach (var icon in _tintedIcons) icon?.Dispose();
        _tintedIcons = null;
        _markerStroke?.Dispose(); _markerStroke = null;
        _iconTintDark = null;
        if (_icons != null) foreach (var icon in _icons) icon.Dispose();
        _icons = null;
        _sourceSnapshot = null;
        _resourcesReady = _loadingMaps = false;
    }
}
