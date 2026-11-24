using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class DongleCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private DongleState _state = new();
    private bool _dialogOpen;
    private bool _submitted;
    private bool _rendering;
    private bool _selectionIssue;
    private string? _localError;
    public DongleCard() { InitializeComponent(); }

    public void Update(DongleState state)
    {
        _state = state;
        _submitted = false;
        _localError = null;
        Render();
    }

    public void Disconnected() { _state = new(); _submitted = false; Render(); }

    private void Render()
    {
        bool selectionIssue = !_state.Selected || _state.Ambiguous || _state.Error == "selected_missing";
        if (selectionIssue && !_selectionIssue) DongleOptions.IsExpanded = true;
        _selectionIssue = selectionIssue;
        bool healthBusy = DongleHealthPresentation.Busy(_state.Health);
        bool ready = Client?.IsReady == true && _state.Present && !_state.Ambiguous && !_state.Pending && !healthBusy && !_submitted && !_dialogOpen;
        PendingRing.IsActive = _state.Pending || healthBusy || _submitted;
        PendingRing.Visibility = PendingRing.IsActive ? Visibility.Visible : Visibility.Collapsed;
        ConnectionRow.Visibility = !_state.Selected || PendingRing.IsActive ? Visibility.Visible : Visibility.Collapsed;
        ConnectionText.Text = Localize.Get(!DongleHealthPresentation.Attached(_state) ? "Dongle_Error" : DongleHealthPresentation.ConnectionKey(_state));
        OutputHelpText.Text = Localize.Get(_state.UsbAudioAvailable ? "Dongle_OutputHelp" : "Dongle_ControlOnlyHelp");
        OutputHelpText.Visibility = _state.Present && _state.Session != 0 && _state.Generation != 0 ? Visibility.Visible : Visibility.Collapsed;
        _rendering = true;
        try
        {
            var shown = (DevicePicker.ItemsSource as DongleDevice[]) ?? [];
            if (!shown.Select(d => (d.Serial, d.UsbAudioAvailable)).SequenceEqual(_state.AvailableDevices.Select(d => (d.Serial, d.UsbAudioAvailable))))
                DevicePicker.ItemsSource = _state.AvailableDevices;
            DevicePicker.SelectedValue = _state.Serial;
            DevicePicker.Visibility = _state.AvailableDevices.Length > 1 || _state.Ambiguous || _state.Error == "selected_missing"
                ? Visibility.Visible : Visibility.Collapsed;
            DevicePicker.IsEnabled = Client?.IsReady == true && _state.AvailableDevices.Length > 0 && !_state.Pending && !healthBusy && !_submitted && !_dialogOpen;
            bool? policy = _state.Health.ReconnectAutomatically;
            bool idle = DongleHealthPresentation.ReconnectPolicyIdle(_state);
            ReconnectRow.Visibility = policy.HasValue || _state.Health.ReconnectPolicyError ? Visibility.Visible : Visibility.Collapsed;
            ReconnectSwitch.Visibility = policy.HasValue ? Visibility.Visible : Visibility.Collapsed;
            ReconnectSwitch.IsOn = policy == true;
            ReconnectSwitch.IsEnabled = ready && idle && policy.HasValue;
            string help = Localize.Get(_state.Health.ReconnectPolicyError ? "Dongle_AutoConnectError" :
                idle ? "Dongle_AutoConnectHelp" : "Dongle_AutoConnectBusy");
            ReconnectHelp.Text = help;
            AutomationProperties.SetHelpText(ReconnectSwitch, help);
        }
        finally { _rendering = false; }
        PairButton.IsEnabled = ready && (_state.Capabilities & 1) != 0 && !_state.UsbPlayback && !_state.MicrophoneActive && !_state.PhoneConnected;
        ToolTipService.SetToolTip(PairButton, Localize.Get("Dongle_PairHelp"));
        PhoneButton.IsEnabled = ready && (_state.Capabilities & 4) != 0;
        string pairingHelp = Localize.Get("Dongle_BluetoothAudioPairHelp");
        ToolTipService.SetToolTip(PhoneButton, pairingHelp); AutomationProperties.SetHelpText(PhoneButton, pairingHelp);
        DiagnosticsSection.Visibility = AppSettings.EnableExperimental && (_state.Capabilities & 16) != 0 ? Visibility.Visible : Visibility.Collapsed;
        DiagnosticsRefresh.IsEnabled = ready && _state.Selected;
        DiagnosticValues.Visibility = _state.Health.Performance is null ? Visibility.Collapsed : Visibility.Visible;
        if (_state.Health.Performance is { } counters)
        {
            static string Number(uint? value) => value?.ToString() ?? "—";
            static string KiB(uint? value) => value is uint bytes ? (bytes / 1024.0).ToString("0.0") : "—";
            DiagnosticQueue.Text = Localize.Get("Dongle_Queue", Number(counters.PcmUsed), Number(counters.PcmCapacity), Number(counters.PcmHighWater));
            DiagnosticMemory.Text = Localize.Get("Dongle_Memory", KiB(counters.HeapRemaining), KiB(counters.HeapCapacity));
            DiagnosticEncoding.Text = Localize.Get("Dongle_Encoding", Number(counters.EncodeUs), Number(counters.EncodeMaxUs));
            DiagnosticDrops.Text = Localize.Get("Dongle_Drops", Number(counters.UnderflowFrames), Number(counters.DroppedFrames));
            DiagnosticRecovery.Text = Localize.Get("Dongle_Recovery", Number(counters.SendFailures), Number(counters.Recoveries));
        }
        string? key = _localError ?? (_state.Error is { } reason ? reason switch {
            "multiple" => "Dongle_Multiple", "selected_missing" => null, "busy" => "Dongle_Busy", "stale" => "Dongle_Stale",
            "timeout" => "Dongle_Timeout", "unsupported" or "protocol" or "malformed" => "Dongle_Unsupported",
            _ => "Dongle_Error"
        } : null);
        ErrorText.Text = key is null ? "" : Localize.Get(key);
        ErrorText.Visibility = key is null ? Visibility.Collapsed : Visibility.Visible;
    }

    private void Device_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_rendering || !DevicePicker.IsEnabled || DevicePicker.SelectedItem is not DongleDevice device || device.Serial == _state.Serial) return;
        _submitted = Client?.Send(new SelectDongleCmd { Serial = device.Serial }) == true;
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }

    private void Send(byte opcode, byte argument = 0)
    {
        if (_state.Pending || DongleHealthPresentation.Busy(_state.Health) || _submitted || Client?.IsReady != true) return;
        _submitted = Client.Send(new DongleControlCmd { Session = _state.Session, Generation = _state.Generation, Opcode = opcode, Argument = argument });
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }
    private void Phone_Click(object sender, RoutedEventArgs e) => Send(4);
    private void Reconnect_Toggled(object sender, RoutedEventArgs e)
    {
        if (_rendering || !ReconnectSwitch.IsEnabled || _state.Health.ReconnectAutomatically is not bool current ||
            current == ReconnectSwitch.IsOn || Client is null) return;
        _submitted = Client.Send(new DongleHealthCmd { Session = _state.Session, Generation = _state.Generation,
            Command = new SetDongleReconnectPolicyCmd { Automatic = ReconnectSwitch.IsOn } });
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }
    private void Diagnostics_Click(object sender, RoutedEventArgs e)
    {
        if (!DiagnosticsRefresh.IsEnabled || Client is null) return;
        _submitted = Client.Send(new DongleHealthCmd { Session = _state.Session, Generation = _state.Generation, Command = new GetDonglePerformanceCmd() });
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }
    private async void Pair_Click(object sender, RoutedEventArgs e)
    {
        ulong session = _state.Session; uint generation = _state.Generation;
        _dialogOpen = true; Render();
        var dialog = new ContentDialog {
            XamlRoot = XamlRoot, RequestedTheme = ActualTheme,
            Title = Localize.Get("Dongle_Pair"),
            Content = Localize.Get(_state.Selected ? "Dongle_ReplaceConfirm" : "Dongle_PairHelp"),
            PrimaryButtonText = Localize.Get("Dongle_PairAction"), CloseButtonText = Localize.Get("Dongle_Cancel"),
            DefaultButton = ContentDialogButton.Close
        };
        try {
            if (await dialog.ShowAsync() == ContentDialogResult.Primary && session == _state.Session && generation == _state.Generation) Send(2);
        } catch (Exception) { _localError = "Dongle_Error"; }
        finally { _dialogOpen = false; Render(); }
    }
}
