using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace NTPods.WinUI.Controls;

/// Keep the user's volume edit visible until the daemon confirms it.
public sealed partial class VolumeCard : UserControl
{
    public DaemonClient? Client { get; set; }

    private bool _applying;
    private bool _muted;
    private string _deviceId = "";
    private readonly VolumeEditState _edit = new();
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(5) };

    public VolumeCard()
    {
        InitializeComponent();
        VolumeSlider.AddHandler(PointerPressedEvent, new PointerEventHandler((_, _) => BeginInteraction()), true);
        foreach (var routedEvent in new[] { PointerReleasedEvent, PointerCaptureLostEvent, PointerCanceledEvent })
            VolumeSlider.AddHandler(routedEvent, new PointerEventHandler((_, _) => EndInteraction()), true);
        VolumeSlider.AddHandler(KeyDownEvent, new KeyEventHandler((_, e) => { if (IsVolumeKey(e.Key)) BeginInteraction(); }), true);
        VolumeSlider.AddHandler(KeyUpEvent, new KeyEventHandler((_, e) => { if (IsVolumeKey(e.Key)) EndInteraction(); }), true);
        VolumeSlider.LostFocus += (_, _) => EndInteraction();
        _timeout.Tick += (_, _) =>
        {
            _timeout.Stop();
            _edit.Cancel();
            Render();
            Client?.RequestState();
        };
        Unloaded += (_, _) => { _timeout.Stop(); _edit.Cancel(); };
    }

    /// Render the slider / mute state from a daemon Snapshot.
    public void Update(Snapshot s)
    {
        if (_deviceId != s.DeviceId) { _deviceId = s.DeviceId; _edit.Cancel(); }
        _muted = s.Muted;
        _edit.Update(s.Volume, s.Connected && Client?.IsReady == true);
        if (_edit.Requested is null) _timeout.Stop();
        Render();
    }

    private void Render()
    {
        _applying = true;
        try
        {
            VolumeSlider.Value = _edit.Displayed;
            VolumeText.Text = _muted ? Localize.Get("Volume_Muted") : $"{_edit.Displayed}%";
            MuteToggle.IsChecked = _muted;
        }
        finally
        {
            _applying = false;
        }
    }

    private void Volume_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying) return;
        byte value = (byte)Math.Clamp(Math.Round(e.NewValue), 0, 100);
        _edit.Request(value);
        if (Client?.Send(new SetVolumeCmd { Percent = value }) != true) _edit.Cancel();
        ArmTimeout();
        Render();
    }

    private void BeginInteraction() { _edit.BeginInteraction(); _timeout.Stop(); }

    private void EndInteraction()
    {
        _edit.EndInteraction();
        ArmTimeout();
        Render();
    }

    private void ArmTimeout()
    {
        _timeout.Stop();
        if (!_edit.Interacting && _edit.Requested is not null) _timeout.Start();
    }

    private static bool IsVolumeKey(VirtualKey key) => key is VirtualKey.Left or VirtualKey.Right or VirtualKey.Up or VirtualKey.Down
        or VirtualKey.Home or VirtualKey.End or VirtualKey.PageUp or VirtualKey.PageDown;

    private void Mute_Click(object sender, RoutedEventArgs e) => Client?.ToggleMute();
}
