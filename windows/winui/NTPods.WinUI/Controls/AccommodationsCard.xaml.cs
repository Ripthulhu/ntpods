using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class AccommodationsCard : UserControl
{
    private DaemonClient? _client;
    private bool _subscribed;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (ReferenceEquals(_client, value)) return;
            Detach();
            StopEditing();
            _edit.Update(new(), false);
            _snapshot = new();
            _client = value;
            _clientEpoch = value?.SessionEpoch ?? 0;
            if (IsLoaded) Attach();
            Render();
        }
    }
    public Func<object, bool>? SendCommand { get; set; }
    public event Action? TransparencyRequested;
    private Snapshot _snapshot = new();
    private readonly AccommodationsEditState _edit = new(AppSettings.HeadphoneAccommodationsDestinations,
        (deviceId, phone, media) => AppSettings.SetHeadphoneAccommodationsDestinations(deviceId, phone, media));
    private bool _applying = true;
    private ulong _requestId = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
    private ulong _clientEpoch;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(15) };
    private readonly DispatcherTimer _live = new() { Interval = TimeSpan.FromMilliseconds(150) };
    private static readonly string[] Tunings = ["balanced_tone", "vocal_range", "brightness"];
    private static readonly string[] Levels = ["slight", "moderate", "strong"];

    public AccommodationsCard()
    {
        InitializeComponent();
        _live.Tick += (_, _) => { _live.Stop(); Flush(); };
        _timeout.Tick += (_, _) => { _timeout.Stop(); _edit.Cancel(); Client?.RequestState(); Render(); };
        Loaded += (_, _) => { Attach(); Render(); };
        Unloaded += (_, _) => { StopEditing(); Detach(); };
        _applying = false;
        Render();
    }

    private void Attach()
    {
        if (_subscribed || _client is null) return;
        _client.CommandResultReceived += ApplyResult;
        _subscribed = true;
    }

    private void Detach()
    {
        if (_subscribed && _client is not null) _client.CommandResultReceived -= ApplyResult;
        _subscribed = false;
    }

    private bool Available => Client?.IsReady == true && _snapshot.Connected && !_snapshot.LinkStale;
    private bool OtherBusy => _snapshot.HearingAidPending || _snapshot.TransparencyPending ||
        _snapshot.HardwareDiagnosticsPending || _snapshot.HearingMetadataPending || _snapshot.LoudSoundReductionPending ||
        _snapshot.MicCaptureActive || _snapshot.MicUplinkActive ||
        _snapshot.MicState is not ("idle" or "unavailable") ||
        HardwareDiagnosticsPresentation.IsRunning(_snapshot.HardwareCheck);

    public void Update(Snapshot snapshot, string? preferenceDeviceId = null)
    {
        if (_clientEpoch != (Client?.SessionEpoch ?? 0))
        {
            _clientEpoch = Client?.SessionEpoch ?? 0;
            StopEditing();
            _edit.Update(new(), false);
        }
        _snapshot = snapshot;
        _edit.Update(snapshot, Available, preferenceDeviceId);
        if (!_edit.Sending) _timeout.Stop();
        Render();
        if (_edit.Dirty && !_edit.Sending) Schedule();
    }

    private void Render()
    {
        _applying = true;
        try
        {
            bool busy = _edit.Sending || _snapshot.HeadphoneAccommodations?.Pending == true;
            bool editable = Available && !OtherBusy && _edit.CanEdit;
            var view = _edit.View(_snapshot.HeadphoneAccommodations);
            var value = view.Settings;
            MasterSwitch.IsEnabled = TuningPicker.IsEnabled = PhoneBox.IsEnabled = MediaBox.IsEnabled = editable;
            AudiogramChoice.IsEnabled = editable && _snapshot.HeadphoneAccommodations?.AudiogramAvailable == true;
            if (!view.Enabled && view.Phone == false && view.Media == false) MasterSwitch.IsEnabled = false;
            MasterSwitch.IsOn = view.Enabled;
            PhoneBox.IsOn = view.Phone == true;
            MediaBox.IsOn = view.Media == true;
            TuningPicker.SelectedIndex = value?.Audiogram == true ? 0 : value?.Preset is { } preset ? Array.IndexOf(Tunings, preset.Tuning) + 1 : -1;
            OptimizationSlider.Value = value?.Preset is { } optimized ? Array.IndexOf(Levels, optimized.Optimization) : 0;
            OptimizationSlider.IsEnabled = editable && value?.Preset is not null;
            OptimizationControls.Visibility = value?.Preset is null ? Visibility.Collapsed : Visibility.Visible;
            bool receivedReadOnly = Available && value is { Phone: not null, Media: not null, Editable: false };
            TuningHelp.Text = Localize.Get(receivedReadOnly ? "Accommodations.ReadOnly" : value?.Audiogram == true ? "Accommodations.AudiogramHelp" : value?.Preset?.Tuning switch
            {
                "balanced_tone" => "Accommodations.BalancedHelp", "vocal_range" => "Accommodations.VocalHelp",
                "brightness" => "Accommodations.BrightnessHelp", _ => "Accommodations.CustomHelp"
            });
            TransparencyButton.IsEnabled = SendCommand is null && DeviceModels.Find(_snapshot.Model).CustomTransparency;
            string help = Localize.Get(!Available ? "DeviceSetting.Disconnected" : busy ? "DeviceSetting.Pending" :
                receivedReadOnly ? "Accommodations.ReadOnly" : _edit.Dirty ? "DeviceSetting.Unsent" :
                !editable ? "DeviceSetting.Unknown" : "Accommodations.Help");
            AutomationProperties.SetHelpText(this, help);
            ToolTipService.SetToolTip(MasterSwitch, editable && !MasterSwitch.IsEnabled ? Localize.Get("Accommodations.SelectDestination") : help);
            AutomationProperties.SetHelpText(MasterSwitch, editable && !MasterSwitch.IsEnabled ? Localize.Get("Accommodations.SelectDestination") : help);
            ResultText.Text = _snapshot.HeadphoneAccommodations?.Error ??
                (_snapshot.HeadphoneAccommodations?.Result == "unconfirmed" ? Localize.Get("Accommodations.Unconfirmed") : "");
            ResultText.Visibility = !busy && ResultText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
        finally { _applying = false; }
    }

    private void Master_Changed(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        _edit.Enable(MasterSwitch.IsOn);
        Render();
        Schedule();
    }

    private void Tuning_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || TuningPicker.SelectedIndex < 0 || _edit.Draft is not { } value) return;
        _edit.Edit(TuningPicker.SelectedIndex == 0 ? value with { Audiogram = true, Preset = null } :
            value with { Audiogram = false, Preset = new() { Tuning = Tunings[TuningPicker.SelectedIndex - 1], Optimization = value.Preset?.Optimization ?? "slight" } });
        Render();
        Schedule();
    }

    private void Optimization_Changed(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || _edit.Draft is not { Preset: { } preset } value) return;
        _edit.Edit(value with { Preset = preset with { Optimization = Levels[(int)Math.Round(OptimizationSlider.Value)] } });
        Render();
        Schedule();
    }

    private void Destination_Changed(object sender, RoutedEventArgs e)
    {
        if (_applying || _edit.Draft is not { } value) return;
        _edit.Destinations(PhoneBox.IsOn, MediaBox.IsOn);
        Render();
        Schedule();
    }

    private void Schedule()
    {
        if (_edit.Dirty && !_live.IsEnabled) _live.Start();
    }
    private void Flush()
    {
        if (Available && _clientEpoch == Client?.SessionEpoch && !OtherBusy && _edit.Command(++_requestId) is { } command) Send(command);
    }
    private void Send(SetHeadphoneAccommodationsCmd command)
    {
        if (!(SendCommand?.Invoke(command) ?? Client?.Send(command) == true))
        {
            _edit.Cancel();
            Render();
            return;
        }
        _edit.Begin(_snapshot, command.RequestId, _clientEpoch);
        _timeout.Start();
        Render();
    }
    private void ApplyResult(DaemonEvent.CommandResult result)
    {
        if (!IsLoaded) return;
        var client = Client;
        ulong epoch = client?.SessionEpoch ?? 0;
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!IsLoaded || !ReferenceEquals(client, Client) || epoch != _clientEpoch || epoch != Client?.SessionEpoch ||
                !_edit.FailRequest(result, epoch)) return;
            _live.Stop(); _timeout.Stop();
            Client?.RequestState();
            Render();
            if (!string.IsNullOrEmpty(result.Message))
            {
                ResultText.Text = result.Message;
                ResultText.Visibility = Visibility.Visible;
            }
        });
    }
    public void StopEditing() { _live.Stop(); _timeout.Stop(); _edit.Cancel(); }
    private void Transparency_Click(object sender, RoutedEventArgs e) => TransparencyRequested?.Invoke();

}
