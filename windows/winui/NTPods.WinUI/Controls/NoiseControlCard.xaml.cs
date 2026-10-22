using System;
using System.Diagnostics;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Media;
using Windows.System;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Controls;

/// Shared listening-mode thumb, with local gesture previews and confirmed device state.
public sealed partial class NoiseControlCard : UserControl
{
    public DaemonClient? Client { get; set; }
    public Func<object, bool>? SendCommand { get; set; }

    private const byte ModeId = 0x0D;
    private const byte StrengthId = ControlId.AdaptiveNoiseStrength;
    private readonly ToggleButton[] _buttons;
    private readonly DispatcherTimer _requestTimeout = new() { Interval = TimeSpan.FromSeconds(12) };
    private readonly DispatcherTimer _strengthEdit = new() { Interval = TimeSpan.FromMilliseconds(350) };
    private readonly DispatcherTimer _gestureFlush = new() { Interval = TimeSpan.FromMilliseconds(100) };
    private Snapshot _snapshot = new();
    private bool _applying;
    private bool _dragging;
    private bool _editingStrength;
    private bool _strengthKeyHeld;
    private uint _previewMode;
    private uint _previewStrength;
    private double _dragLeft;
    private readonly ListeningModeRequest _request = new();
    private readonly ListeningModeMotion _motion = new();
    private readonly UISettings _uiSettings = new();
    private readonly AccessibilitySettings _accessibility = new();
    private readonly ListeningModeGlass _glass = new();
    private readonly Dictionary<UIElement, float> _iconOpacity = new();
    private double _velocityAt;
    private double _velocityDistance;
    private uint _focusedMode;
    private ListeningModeLayout _paintedLayout;
    private uint _paintedMode = uint.MaxValue;
    private bool _renderingMotion;
    private bool _gestureRenderPending;
    private bool _timedOut;

    public NoiseControlCard()
    {
        InitializeComponent();
        // The PNGs have transparent padding. Keep atlas-edge pixels outside the
        // visible icon while fractional translations and opacity fades are active.
        foreach (var icon in new[] { OffIcon, TransparencyIcon, AdaptiveIcon, AncIcon,
            SelectedTransparencyIcon, SelectedAdaptiveIcon, SelectedAncIcon })
            icon.Clip = new RectangleGeometry { Rect = new Windows.Foundation.Rect(1, 1, 26, 26) };
        GlassHost.Children.Add(_glass);
        _glass.ReadinessChanged += (_, _) => RefreshGlassFallback();
        ActualThemeChanged += (_, _) => { UpdateMaterial(); PaintMotion(); };
        _buttons = [AncOffBtn, AncNcBtn, AncTransBtn, AncAdaptiveBtn];
        StrengthSlider.AddHandler(KeyDownEvent, new KeyEventHandler(Strength_KeyDown), true);
        StrengthSlider.AddHandler(KeyUpEvent, new KeyEventHandler(Strength_KeyUp), true);
        foreach (var button in _buttons)
        {
            ToolTipService.SetToolTip(button, ModeName(Convert.ToUInt32((string)button.Tag)));
            button.GotFocus += (_, _) =>
            {
                _focusedMode = button.FocusState == FocusState.Keyboard ? Convert.ToUInt32((string)button.Tag) : 0;
                UpdateModeFocus();
            };
            button.LostFocus += (_, _) => { _focusedMode = 0; UpdateModeFocus(); };
        }
        _requestTimeout.Tick += (_, _) =>
        {
            CancelInteraction();
            _timedOut = true;
            Render();
        };
        _strengthEdit.Tick += (_, _) => CompleteStrengthEdit();
        _gestureFlush.Tick += (_, _) => FlushGesture();
        Loaded += (_, _) =>
        {
            _uiSettings.ColorValuesChanged += Colors_Changed;
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _uiSettings.AnimationsEnabledChanged += AnimationsEnabled_Changed;
            UpdateMaterial();
            Render();
        };
        Unloaded += (_, _) =>
        {
            CancelInteraction();
            _uiSettings.ColorValuesChanged -= Colors_Changed;
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _uiSettings.AnimationsEnabledChanged -= AnimationsEnabled_Changed;
            _motion.Finish();
            FinishIconFades();
            StopMotion();
        };
        Update(_snapshot);
    }

    private uint ConfirmedMode => _snapshot.DeviceControls.TryGetValue(ModeId, out var mode) ? mode : 0;
    private uint DisplayedStrength => Math.Min(DeviceSettingStatus.DisplayValue(_snapshot, StrengthId) ?? 0, 100u);
    private bool AllowOff => DeviceModels.OffAvailable(_snapshot);
    private bool GestureAvailable => ListeningModePolicy.CanContinueGesture(_snapshot);
    private bool ModeAvailable => ListeningModePolicy.CanChooseMode(_snapshot,
        _dragging || _editingStrength || _request.IsPending || _request.HasTarget);
    private bool StrengthAvailable => GestureAvailable && !_snapshot.AncUnconfirmed && ConfirmedMode == 4 &&
        _snapshot.DeviceControls.ContainsKey(StrengthId);
    private static double MotionTime => Stopwatch.GetTimestamp() * 1000d / Stopwatch.Frequency;
    private bool Animate => IsLoaded && GestureAvailable && _uiSettings.AnimationsEnabled;

    public void Update(Snapshot snapshot)
    {
        bool newError = !string.IsNullOrWhiteSpace(snapshot.DeviceSettingsError) &&
            snapshot.DeviceSettingsError != _snapshot.DeviceSettingsError;
        bool changedDevice = snapshot.DeviceId != _snapshot.DeviceId;
        if (changedDevice) CancelInteraction();
        _snapshot = snapshot;
        _request.Update(snapshot, newError);
        if (!_request.IsPending) _requestTimeout.Stop();
        if (!GestureAvailable || newError) CancelInteraction();
        Render();
    }

    private void Render()
    {
        if (_buttons is null) return;
        _gestureRenderPending = false;
        _applying = true;
        try
        {
            uint mode = _dragging ? _previewMode : _request.DisplayMode(ConfirmedMode);
            uint strength = _dragging || _editingStrength ? _previewStrength : _request.DisplayStrength(DisplayedStrength);
            bool adaptive = DeviceModels.Adaptive(_snapshot);
            var layout = ListeningModeLayout.Create(Track.ActualWidth, mode, AllowOff, adaptive);
            bool expanded = mode == 4;
            AncAdaptiveBtn.Visibility = !adaptive || expanded ? Visibility.Collapsed : Visibility.Visible;
            AdaptiveLabel.Visibility = adaptive ? Visibility.Visible : Visibility.Collapsed;
            AdaptiveRail.Visibility = expanded ? Visibility.Visible : Visibility.Collapsed;
            foreach (var button in _buttons)
            {
                uint value = Convert.ToUInt32((string)button.Tag);
                button.IsChecked = value == ConfirmedMode && !_snapshot.AncUnconfirmed;
                button.IsEnabled = ModeAvailable && ListeningModePolicy.ModeSupported(_snapshot, value) && (value != 1 || AllowOff);
            }
            StrengthSlider.IsEnabled = StrengthAvailable;
            StrengthSlider.Value = strength;
            SelectionThumb.IsEnabled = GestureAvailable;
            Selection.Visibility = ListeningModePolicy.CanShowSelection(mode, AllowOff) && layout.Width > 0
                ? Visibility.Visible : Visibility.Collapsed;
            Selection.Opacity = GestureAvailable ? 1 : 0.55;
            if (_paintedMode != mode)
            {
                _paintedMode = mode;
                FadeIcon(SelectedTransparencyIcon, mode == 3);
                FadeIcon(SelectedAdaptiveIcon, mode == 4);
                FadeIcon(SelectedAncIcon, mode is 1 or 2);
                FadeIcon(OffIcon, mode != 1);
                FadeIcon(TransparencyIcon, mode != 3);
                FadeIcon(AdaptiveIcon, mode != 4);
                FadeIcon(AncIcon, mode != 2);
            }
            _motion.SetTarget(layout, _dragging ? _dragLeft : layout.LeftFor(mode, strength), MotionTime,
                _dragging, Animate);
            PaintMotion();
            UpdateModeFocus();
            if (!Animate) FinishIconFades();
            if (_motion.IsMoving) StartMotion();
            else StopMotion();
            AncModeLabel.Text = ModeName(ConfirmedMode);
            RenderStatus();
        }
        finally { _applying = false; }
    }

    private void PaintMotion()
    {
        var layout = _motion.Layout;
        if (_paintedLayout != layout)
        {
            _paintedLayout = layout;
            OffColumn.Width = new GridLength(layout.OffWidth);
            TransparencyColumn.Width = new GridLength(layout.Segment);
            AdaptiveColumn.Width = new GridLength(layout.AdaptiveWidth);
            AncColumn.Width = new GridLength(layout.Segment);
            AncOffBtn.Visibility = layout.OffWidth > 0 ? Visibility.Visible : Visibility.Collapsed;
            // Measure wrapped labels at a stable width; animation only moves their centers.
            double labelSlot = layout.Width / (layout.HasAdaptive ? 4 : 3);
            double collapsedSlot = layout.Width / (layout.HasAdaptive ? 5 : 3);
            double labelWidth = Math.Max(0, labelSlot - 4);
            if (OffLabel.Width != labelWidth)
                OffLabel.Width = TransparencyLabel.Width = AdaptiveLabel.Width = AncLabel.Width = labelWidth;
            OffLabelTransform.X = (layout.OffWidth - labelSlot) / 2;
            TransparencyLabelTransform.X = layout.OffWidth + (layout.Segment - labelSlot) / 2;
            AdaptiveLabelTransform.X = layout.AdaptiveStart + (layout.AdaptiveWidth - labelSlot) / 2;
            AncLabelTransform.X = layout.AncStart + (layout.Segment - labelSlot) / 2;
            OffLabel.Opacity = !layout.HasAdaptive ? (layout.OffWidth > 0 ? 1 : 0) : layout.Width > 0
                ? Math.Clamp((layout.OffWidth - collapsedSlot) / (layout.Width / 4 - collapsedSlot), 0, 1) : 0;
            Selection.Width = layout.Segment;
        }
        SelectionTransform.X = layout.ClampLeft(_motion.Left);
        SelectionScale.ScaleX = _motion.ScaleX;
        SelectionScale.ScaleY = _motion.ScaleY;
        MinimumMarker.Opacity = layout.MarkerOpacity(layout.AdaptiveStart + 22.5, SelectionTransform.X, _motion.ScaleX);
        MaximumMarker.Opacity = layout.MarkerOpacity(layout.AncStart - 22.5, SelectionTransform.X, _motion.ScaleX);
        _glass.Visibility = Selection.Visibility;
        _glass.Opacity = Selection.Opacity;
        _glass.Update(layout, SelectionTransform.X, _paintedMode, _motion.ScaleX, _motion.ScaleY,
            ActualTheme == ElementTheme.Dark, _accessibility.HighContrast);
        RefreshGlassFallback();
    }

    private void Motion_Rendering(object? sender, object e)
    {
        if (_gestureRenderPending) { Render(); return; }
        _motion.Tick(MotionTime);
        PaintMotion();
        if (!_motion.IsMoving) StopMotion();
    }

    private void StartMotion()
    {
        if (_renderingMotion) return;
        CompositionTarget.Rendering += Motion_Rendering;
        _renderingMotion = true;
    }

    private void StopMotion()
    {
        if (!_renderingMotion) return;
        CompositionTarget.Rendering -= Motion_Rendering;
        _renderingMotion = false;
    }

    private void AnimationsEnabled_Changed(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!IsLoaded) return;
            if (!sender.AnimationsEnabled) Render();
        });

    private void Colors_Changed(UISettings sender, object args) =>
        DispatcherQueue.TryEnqueue(() => { if (IsLoaded) { UpdateMaterial(); PaintMotion(); } });

    private void UpdateMaterial()
    {
        AdaptiveSurface.Background = _accessibility.HighContrast
            ? (Brush)Application.Current.Resources["AppleSelectedBrush"]
            : new SolidColorBrush(ActualTheme == ElementTheme.Dark
                ? Windows.UI.Color.FromArgb(64, 136, 136, 136) : Windows.UI.Color.FromArgb(64, 255, 255, 255));
        RefreshGlassFallback();
    }

    private void RefreshGlassFallback() => SelectionFallback.Visibility = _glass.IsReady && !_accessibility.HighContrast
        ? Visibility.Collapsed : Visibility.Visible;

    private void UpdateModeFocus() => ModeFocusRing.Visibility = _focusedMode != 0 && _focusedMode == _paintedMode
        ? Visibility.Visible : Visibility.Collapsed;

    private void FadeIcon(UIElement element, bool visible)
    {
        float target = visible ? 1 : 0;
        bool known = _iconOpacity.TryGetValue(element, out float previous);
        if (known && previous == target) return;
        _iconOpacity[element] = target;
        var visual = ElementCompositionPreview.GetElementVisual(element);
        if (!Animate || !known)
        {
            visual.ImplicitAnimations = null;
            visual.StopAnimation("Opacity");
            visual.Opacity = target;
            return;
        }
        var fade = visual.Compositor.CreateScalarKeyFrameAnimation();
        fade.Target = "Opacity";
        fade.InsertExpressionKeyFrame(1, "this.FinalValue");
        fade.Duration = TimeSpan.FromMilliseconds(120);
        var animations = visual.Compositor.CreateImplicitAnimationCollection();
        animations["Opacity"] = fade;
        visual.ImplicitAnimations = animations;
        visual.Opacity = target;
    }

    private void FinishIconFades()
    {
        foreach (var (element, opacity) in _iconOpacity)
        {
            var visual = ElementCompositionPreview.GetElementVisual(element);
            visual.ImplicitAnimations = null;
            visual.StopAnimation("Opacity");
            visual.Opacity = opacity;
        }
    }

    private void RenderStatus()
    {
        string help = string.Empty;
        if (_dragging || _editingStrength)
            help = _previewMode == 4 && ConfirmedMode == 4
                ? Localize.Get("NoiseControl.StrengthPreview", _previewStrength)
                : Localize.Get("NoiseControl.GesturePreview", ModeName(_previewMode));
        else if (_request.Id == ModeId || _snapshot.DeviceControlsPending.ContainsKey(ModeId))
            help = Localize.Get("NoiseControl.Pending", ModeName(_request.Id == ModeId
                ? _request.Value : _snapshot.DeviceControlsPending[ModeId]));
        else if (_request.Id == StrengthId || _snapshot.DeviceControlsPending.ContainsKey(StrengthId))
            help = Localize.Get("DeviceSetting.Pending");
        else if (!string.IsNullOrWhiteSpace(_snapshot.DeviceSettingsError))
            help = _snapshot.DeviceSettingsError;
        else if (_timedOut)
            help = Localize.Get("NoiseControl.RequestTimeout");
        else if (_snapshot.AncUnconfirmed)
            help = Localize.Get("AdaptiveNoise.RefreshMode");
        else if (!ModeAvailable)
            help = DeviceSettingStatus.Text(_snapshot, ModeId);
        else if (ConfirmedMode == 1 && !AllowOff)
            help = Localize.Get("NoiseControl.OffUnavailable");
        else if (ConfirmedMode == 4 && !StrengthAvailable)
            help = DeviceSettingStatus.Text(_snapshot, StrengthId);
        else if (ConfirmedMode == 4 && _snapshot.AdaptiveStrengthRequested is not null)
            help = DeviceSettingStatus.Text(_snapshot, StrengthId);
        AutomationProperties.SetHelpText(SelectionThumb, help);
        AutomationProperties.SetHelpText(StrengthSlider, help);
        // Shared device errors already appear on the page; only a local timeout needs another message.
        StatusText.Visibility = _timedOut && string.IsNullOrWhiteSpace(_snapshot.DeviceSettingsError)
            ? Visibility.Visible : Visibility.Collapsed;
        if (StatusText.Visibility == Visibility.Visible) StatusText.Text = Localize.Get("NoiseControl.RequestTimeout");
    }

    private static string ModeName(uint mode) => mode switch
    {
        1 => Localize.Get("Anc_Off"),
        2 => Localize.Get("Anc_NoiseCancellation"),
        3 => Localize.Get("Anc_Transparency"),
        4 => Localize.Get("Anc_Adaptive"),
        _ => "—"
    };

    private void Send(byte id, uint value)
    {
        _timedOut = false;
        _requestTimeout.Stop();
        _requestTimeout.Start();
        var command = new SetDeviceControlCmd { Id = id, Value = value, DeviceId = _snapshot.DeviceId };
        if (SendCommand is { } send) send(command); else Client?.Send(command);
    }

    private void Request(byte id, uint value)
    {
        if (Client is not null && _request.Begin(_snapshot, id, value)) Send(id, _request.Value);
        Render();
    }

    private void QueueGesture(uint mode, uint strength)
    {
        if (Client is null || !GestureAvailable) return;
        _timedOut = false;
        _request.QueueGesture(mode, strength);
        if (!_gestureFlush.IsEnabled) _gestureFlush.Start();
    }

    private void FlushGesture()
    {
        if (Client is null) { CancelInteraction(); Render(); return; }
        if (_request.TakeNext(_snapshot) is { } command) Send(command.Id, command.Value);
        if (!_request.HasTarget && !_dragging && !_editingStrength) _gestureFlush.Stop();
        Render();
    }

    private void CancelInteraction()
    {
        _dragging = false;
        _gestureRenderPending = false;
        _editingStrength = false;
        _strengthKeyHeld = false;
        _strengthEdit.Stop();
        _gestureFlush.Stop();
        _requestTimeout.Stop();
        _request.Cancel();
        _motion.ResetInteraction();
    }

    private void Anc_Click(object sender, RoutedEventArgs e)
    {
        if (_applying || sender is not ToggleButton button || !button.IsEnabled) return;
        uint mode = Convert.ToUInt32((string)button.Tag);
        _editingStrength = false;
        _strengthKeyHeld = false;
        _strengthEdit.Stop();
        Request(ModeId, mode);
    }

    private void Track_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        // Resizing cancels a preview; layout work must never become a device write.
        CancelInteraction();
        Render();
    }

    private void Selection_DragStarted(object sender, DragStartedEventArgs e)
    {
        if (!GestureAvailable) return;
        _strengthEdit.Stop();
        _editingStrength = false;
        _previewMode = _request.DisplayMode(ConfirmedMode);
        _previewStrength = _request.DisplayStrength(DisplayedStrength);
        _velocityAt = MotionTime;
        _velocityDistance = 0;
        _motion.Tick(_velocityAt);
        _dragLeft = _motion.Layout.ClampLeft(_motion.Left);
        if (_previewMode == 4) _previewStrength = _motion.Layout.StrengthAt(_dragLeft);
        _dragging = true;
        _motion.Press(true, _velocityAt, Animate);
        Render();
    }

    private void Selection_DragDelta(object sender, DragDeltaEventArgs e)
    {
        if (!_dragging || !GestureAvailable || e.HorizontalChange == 0) return;
        double now = MotionTime;
        _velocityDistance += e.HorizontalChange;
        // Average a short input window so high-rate mice do not produce noisy stretch.
        double elapsed = now - _velocityAt;
        if (elapsed >= 16)
        {
            _motion.DragVelocity(_velocityDistance * 1000 / elapsed * (XamlRoot?.RasterizationScale ?? 1), now, Animate);
            _velocityDistance = 0;
            _velocityAt = now;
        }
        var layout = _motion.Layout;
        (_previewMode, _previewStrength, _dragLeft) = layout.ResolveDrag(
            _dragLeft + e.HorizontalChange, _previewStrength, AllowOff);
        QueueGesture(_previewMode, _previewStrength);
        // Retain every input, but update layout only once for the next display frame.
        _gestureRenderPending = true;
        StartMotion();
    }

    private void Selection_DragCompleted(object sender, DragCompletedEventArgs e)
    {
        if (!_dragging) return;
        _dragging = false;
        _motion.Press(false, MotionTime, Animate);
        uint mode = _previewMode;
        uint strength = _previewStrength;
        if (e.Canceled) { CancelInteraction(); Render(); return; }
        QueueGesture(mode, strength);
        FlushGesture();
    }

    private void AdaptiveRail_Tapped(object sender, TappedRoutedEventArgs e)
    {
        if (!StrengthAvailable) return;
        var layout = _motion.Layout;
        double localX = e.GetPosition(AdaptiveRail).X;
        QueueGesture(4, layout.StrengthAt(layout.AdaptiveStart + localX - layout.Segment / 2));
        FlushGesture();
        e.Handled = true;
    }

    private void Strength_Changed(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || !StrengthAvailable) return;
        _previewMode = 4;
        _previewStrength = (uint)Math.Clamp(Math.Round(e.NewValue), 0, 100);
        _editingStrength = true;
        QueueGesture(4, _previewStrength);
        _strengthEdit.Stop();
        if (!_strengthKeyHeld) _strengthEdit.Start();
        Render();
    }

    private void CompleteStrengthEdit()
    {
        _strengthEdit.Stop();
        if (!_editingStrength) return;
        _editingStrength = false;
        uint strength = _previewStrength;
        QueueGesture(4, strength);
        FlushGesture();
    }

    private static bool IsStrengthKey(VirtualKey key) => key is VirtualKey.Left or VirtualKey.Right or VirtualKey.Up or VirtualKey.Down or
        VirtualKey.Home or VirtualKey.End or VirtualKey.PageUp or VirtualKey.PageDown;

    private void Strength_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (IsStrengthKey(e.Key))
        {
            _strengthKeyHeld = true;
            _strengthEdit.Stop();
        }
    }

    private void Strength_KeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (IsStrengthKey(e.Key))
        {
            _strengthKeyHeld = false;
            CompleteStrengthEdit();
        }
    }

    private void Strength_GotFocus(object sender, RoutedEventArgs e) => AdaptiveFocusRing.Visibility = Visibility.Visible;

    private void Strength_LostFocus(object sender, RoutedEventArgs e)
    {
        AdaptiveFocusRing.Visibility = Visibility.Collapsed;
        _strengthKeyHeld = false;
        CompleteStrengthEdit();
    }
}
