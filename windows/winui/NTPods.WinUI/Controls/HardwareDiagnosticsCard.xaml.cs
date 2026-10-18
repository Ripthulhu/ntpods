using System.ComponentModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class HardwareDiagnosticsCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private Snapshot _snapshot = new();
    private bool _requesting;
    private string? _localError;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(15) };

    public HardwareDiagnosticsCard()
    {
        InitializeComponent();
        _timeout.Tick += (_, _) =>
        {
            _timeout.Stop();
            _requesting = false;
            _localError = Localize.Get("HardwareChecks.TimedOut");
            Client?.RequestState();
            Render();
        };
        Loaded += (_, _) => Loc.Instance.PropertyChanged += LanguageChanged;
        Unloaded += (_, _) =>
        {
            Loc.Instance.PropertyChanged -= LanguageChanged;
            _timeout.Stop();
            _requesting = false;
        };
        Render();
    }

    private void LanguageChanged(object? sender, PropertyChangedEventArgs args) => DispatcherQueue.TryEnqueue(() =>
    {
        if (IsLoaded) Render();
    });

    public void Update(Snapshot s)
    {
        if (s.DeviceId != _snapshot.DeviceId || !s.Connected || s.LinkStale || s.HardwareDiagnosticsPending ||
            s.HardwareDiagnostics != _snapshot.HardwareDiagnostics || s.HardwareDiagnosticsError != _snapshot.HardwareDiagnosticsError)
        {
            _requesting = false;
            _localError = null;
            _timeout.Stop();
        }
        _snapshot = s;
        Render();
    }

    private void Render()
    {
        bool busy = _requesting || _snapshot.HardwareDiagnosticsPending;
        ReadButton.IsEnabled = !_requesting && HardwareDiagnosticsPresentation.CanRead(_snapshot, Client?.IsReady == true);
        string help = Localize.Get(busy || _snapshot.HearingAidPending || _snapshot.TransparencyPending || _snapshot.LoudSoundReductionPending ? "DeviceSetting.Pending" :
            Client?.IsReady != true || !_snapshot.Connected || _snapshot.LinkStale
            ? "DeviceSetting.Disconnected" : !ReadButton.IsEnabled ? "HardwareChecks.PauseMic" : "HardwareChecks.Description");
        ToolTipService.SetToolTip(ReadButton, help);
        AutomationProperties.SetHelpText(ReadButton, help);
        var record = _snapshot.HardwareDiagnostics;
        Results.Visibility = record is null ? Visibility.Collapsed : Visibility.Visible;
        EmptyText.Visibility = record is null ? Visibility.Visible : Visibility.Collapsed;
        if (record is not null)
        {
            ShowEar(record.Left, LeftResult, LeftFlags, LeftTime);
            ShowEar(record.Right, RightResult, RightFlags, RightTime);
            ReadTime.Text = HardwareDiagnosticsPresentation.Date(record.ReadAtUnixSeconds) is { } readAt
                ? Localize.Get("HardwareChecks.ReadAt", readAt.ToLocalTime().ToString("g")) : "";
            ReadTime.Visibility = ReadTime.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        }
        ErrorText.Text = _localError ?? _snapshot.HardwareDiagnosticsError ?? "";
        ErrorText.Visibility = !busy && ErrorText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private static void ShowEar(HardwareDiagnosticEar ear, TextBlock result, TextBlock flags, TextBlock time)
    {
        result.Text = Localize.Get(HardwareDiagnosticsPresentation.ResultKey(ear));
        var names = HardwareDiagnosticsPresentation.FailureKeys(ear).Select(key => Localize.Get(key)).ToArray();
        flags.Text = names.Length == 0 ? "" : Localize.Get("HardwareChecks.Reported", string.Join(", ", names));
        flags.Visibility = flags.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        time.Text = HardwareDiagnosticsPresentation.Date(ear.MeasuredAtUnixSeconds) is { } measured
            ? Localize.Get("HardwareChecks.DeviceTime", measured.ToLocalTime().ToString("g")) : "";
        time.Visibility = time.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void Read_Click(object sender, RoutedEventArgs e)
    {
        if (!ReadButton.IsEnabled || Client?.Send(new GetHardwareDiagnosticsCmd()) != true) return;
        _requesting = true;
        _localError = null;
        _timeout.Start();
        Render();
    }
}
