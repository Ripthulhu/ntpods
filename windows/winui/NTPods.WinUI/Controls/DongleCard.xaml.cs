using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class DongleCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private DongleState _state = new();
    private bool _dialogOpen;
    private bool _submitted;
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
        Visibility = _state.Present || _state.Ambiguous ? Visibility.Visible : Visibility.Collapsed;
        bool healthBusy = DongleHealthPresentation.Busy(_state.Health);
        bool ready = Client?.IsReady == true && _state.Present && !_state.Ambiguous && !_state.Pending && !healthBusy && !_submitted && !_dialogOpen;
        PendingRing.IsActive = _state.Pending || healthBusy || _submitted;
        PendingRing.Visibility = PendingRing.IsActive ? Visibility.Visible : Visibility.Collapsed;
        ConnectionText.Text = Localize.Get(_state.PhonePairing ? "Dongle_PhoneWaiting" : _state.Connected ? "Dongle_Connected" : "Dongle_NotConnected");
        string help = Localize.Get("Dongle_NativeHelp");
        ToolTipService.SetToolTip(this, help); AutomationProperties.SetHelpText(this, help);
        ReconnectButton.IsEnabled = ready && _state.Selected && (_state.Capabilities & 1) != 0;
        PairButton.IsEnabled = ready && (_state.Capabilities & 1) != 0 && !_state.UsbPlayback && !_state.MicrophoneActive && !_state.PhoneConnected;
        ToolTipService.SetToolTip(PairButton, Localize.Get("Dongle_PairHelp"));
        PhoneButton.IsEnabled = ready && (_state.Capabilities & 4) != 0;
        DiagnosticsExpander.Visibility = (_state.Capabilities & 16) != 0 ? Visibility.Visible : Visibility.Collapsed;
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
        Modes.Visibility = (_state.Capabilities & 2) != 0 && !((_state.Capabilities & 16) != 0 && _state.Health.Available)
            ? Visibility.Visible : Visibility.Collapsed;
        AncButton.IsEnabled = TransparencyButton.IsEnabled = ready && _state.ControlsReady;
        AncButton.IsChecked = _state.Anc == 2; TransparencyButton.IsChecked = _state.Anc == 3;
        byte?[] levels = [_state.Battery.Left, _state.Battery.Right, _state.Battery.Case];
        TextBlock[] labels = [LeftBattery, RightBattery, CaseBattery];
        for (int i = 0; i < labels.Length; i++)
        {
            labels[i].Text = levels[i] is byte level ? $"{level}%" : "—";
            uint? age = i < _state.BatteryAgeMs.Length ? _state.BatteryAgeMs[i] : null;
            labels[i].Style = age is <= 30000 ? null : (Style)Resources["StaleBatteryReading"];
            ToolTipService.SetToolTip(labels[i], age is uint ms
                ? string.Format(Localize.Get("Dongle_BatteryAge"), ms / 1000) : Localize.Get("Dongle_Unknown"));
        }
        string? key = _localError ?? (_state.Error is { } reason ? reason switch {
            "multiple" => "Dongle_Multiple", "busy" => "Dongle_Busy", "stale" => "Dongle_Stale",
            "timeout" => "Dongle_Timeout", "unsupported" or "protocol" or "malformed" => "Dongle_Unsupported",
            _ => "Dongle_Error"
        } : null);
        ErrorText.Text = key is null ? "" : Localize.Get(key);
        ErrorText.Visibility = key is null ? Visibility.Collapsed : Visibility.Visible;
    }

    private void Send(byte opcode, byte argument = 0)
    {
        if (_state.Pending || DongleHealthPresentation.Busy(_state.Health) || _submitted || Client?.IsReady != true) return;
        _submitted = Client.Send(new DongleControlCmd { Session = _state.Session, Generation = _state.Generation, Opcode = opcode, Argument = argument });
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }
    private void Reconnect_Click(object sender, RoutedEventArgs e) => Send(1);
    private void Phone_Click(object sender, RoutedEventArgs e) => Send(4);
    private void Diagnostics_Click(object sender, RoutedEventArgs e)
    {
        if (!DiagnosticsRefresh.IsEnabled || Client is null) return;
        _submitted = Client.Send(new DongleHealthCmd { Session = _state.Session, Generation = _state.Generation, Command = new GetDonglePerformanceCmd() });
        if (!_submitted) _localError = "Dongle_Error";
        Render();
    }
    private void Mode_Click(object sender, RoutedEventArgs e)
    {
        if (sender is ToggleButton { Tag: string tag } && byte.TryParse(tag, out byte mode)) Send(3, mode);
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
