using System.ComponentModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class HardwareDiagnosticsCard : UserControl
{
    public Func<object, bool>? SendCommand { get; set; }
    private bool Send(object command) => SendCommand?.Invoke(command) ?? Client?.Send(command) == true;
    private DaemonClient? _client;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (_client is not null)
            {
                _client.CommandResultReceived -= ApplyResult;
                _client.ConnectionChanged -= ConnectionChanged;
            }
            _client = value;
            ClearRequest();
            if (_client is not null)
            {
                _client.CommandResultReceived += ApplyResult;
                _client.ConnectionChanged += ConnectionChanged;
            }
        }
    }
    private Snapshot _snapshot = new();
    private string _requesting = "";
    private string _requestTarget = "";
    private ulong? _requestId;
    private bool _cancelling;
    private string? _localError;
    private string? _metadataLocalError;
    private long _selectionRevision;
    private ContentDialog? _confirmation;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(15) };

    public HardwareDiagnosticsCard()
    {
        InitializeComponent();
        _timeout.Tick += (_, _) =>
        {
            _timeout.Stop();
            if (_requesting.Length == 0) return;
            if (_requesting == "metadata") _metadataLocalError = Localize.Get("HardwareChecks.DeviceDataTimedOut");
            else _localError = Localize.Get(_requesting == "check" ? "HardwareChecks.RequestTimedOut" : "HardwareChecks.TimedOut");
            ClearRequest();
            Client?.RequestState();
            Render();
        };
        Loaded += (_, _) => Loc.Instance.PropertyChanged += LanguageChanged;
        Unloaded += (_, _) =>
        {
            Loc.Instance.PropertyChanged -= LanguageChanged;
            _timeout.Stop();
            ClearRequest();
            _selectionRevision++;
            _confirmation?.Hide();
        };
        Render();
    }

    private void LanguageChanged(object? sender, PropertyChangedEventArgs args) => DispatcherQueue.TryEnqueue(() =>
    {
        if (IsLoaded) Render();
    });

    public void Update(Snapshot s)
    {
        if (s.DeviceId != _snapshot.DeviceId)
        {
            _selectionRevision++;
            _confirmation?.Hide();
            CurveDetails.IsExpanded = false;
            ClearRequest();
            _localError = null;
            _metadataLocalError = null;
        }
        if (!s.Connected || s.LinkStale)
        {
            _selectionRevision++;
            _confirmation?.Hide();
            if (_requesting != "check") ClearRequest();
        }
        if (_requestTarget == s.DeviceId && (_requesting switch
        {
            "diagnostics" => s.HardwareDiagnosticsPending || s.HardwareDiagnostics != _snapshot.HardwareDiagnostics ||
                s.HardwareDiagnosticsError != _snapshot.HardwareDiagnosticsError,
            "metadata" => s.HearingMetadataPending || s.HearingMetadata?.SameValues(_snapshot.HearingMetadata) == false ||
                s.HearingMetadataError != _snapshot.HearingMetadataError,
            "check" => s.HardwareCheck?.RequestId == _requestId && (!_cancelling || !HardwareDiagnosticsPresentation.IsRunning(s.HardwareCheck)),
            _ => false,
        }))
        {
            ClearRequest();
            _localError = null;
            _metadataLocalError = null;
        }
        _snapshot = s;
        Render();
    }

    private void Render()
    {
        bool ready = Client?.IsReady == true;
        bool busy = _requesting.Length > 0 || _snapshot.HardwareDiagnosticsPending || _snapshot.HearingMetadataPending ||
            HardwareDiagnosticsPresentation.IsRunning(_snapshot.HardwareCheck);
        ReadButton.IsEnabled = _requesting.Length == 0 && HardwareDiagnosticsPresentation.CanRead(_snapshot, ready);
        MetadataReadButton.IsEnabled = ReadButton.IsEnabled;
        string help = Localize.Get(busy || _snapshot.HearingAidPending || _snapshot.TransparencyPending || _snapshot.LoudSoundReductionPending ? "DeviceSetting.Pending" :
            Client?.IsReady != true || !_snapshot.Connected || _snapshot.LinkStale
            ? "DeviceSetting.Disconnected" : !ReadButton.IsEnabled ? "HardwareChecks.PauseMic" : "HardwareChecks.Description");
        Help(ReadButton, help);
        Help(MetadataReadButton, ReadButton.IsEnabled ? Localize.Get("HardwareChecks.DeviceDataHelp") : help);
        var record = _snapshot.HardwareDiagnostics;
        Results.Visibility = record is null ? Visibility.Collapsed : Visibility.Visible;
        EmptyText.Visibility = record is null ? Visibility.Visible : Visibility.Collapsed;
        if (record is not null)
        {
            ShowEar(record.Left, LeftResult, LeftFlags, LeftTime, LeftFirstUse);
            ShowEar(record.Right, RightResult, RightFlags, RightTime, RightFirstUse);
            ReadTime.Text = HardwareDiagnosticsPresentation.Date(record.ReadAtUnixSeconds) is { } readAt
                ? Localize.Get("HardwareChecks.ReadAt", readAt.ToLocalTime().ToString("g")) : "";
            ReadTime.Visibility = ReadTime.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        }
        var check = _snapshot.HardwareCheck;
        bool running = HardwareDiagnosticsPresentation.IsRunning(check);
        RunButton.Visibility = HardwareDiagnosticsPresentation.SupportsRun(_snapshot) || check is not null
            ? Visibility.Visible : Visibility.Collapsed;
        RunLabel.Text = Localize.Get(running ? "HardwareChecks.StopWaiting" : "HardwareChecks.Run");
        RunButton.IsEnabled = _confirmation is null && _requesting.Length == 0 && (running
            ? ready && _snapshot.DeviceId.Length > 0 : HardwareDiagnosticsPresentation.CanRun(_snapshot, ready));
        Help(RunButton, running ? Localize.Get("HardwareChecks.StopWaitingHelp") : busy ? Localize.Get("DeviceSetting.Pending") :
            _snapshot.EarbudsInCase != true ? Localize.Get("HardwareChecks.PutInCase") : help);
        bool preparing = check?.Phase == "waiting_case_close" && _snapshot.HardwareDiagnosticsPending;
        string step = check is null ? "" : HardwareDiagnosticsPresentation.CheckKey(check, preparing);
        CheckSteps.Visibility = step.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        CheckInstruction.Text = step.Length == 0 ? "" : Localize.Get(step);
        CheckProgress.Text = check is null ? "" : Localize.Get(HardwareDiagnosticsPresentation.ProgressKey(check));
        CheckProgress.Visibility = preparing ? Visibility.Collapsed : Visibility.Visible;
        ErrorText.Text = _localError ?? check?.Error ?? _snapshot.HardwareDiagnosticsError ?? "";
        ErrorText.Visibility = ErrorText.Text.Length > 0 && _requesting.Length == 0 && !_snapshot.HardwareDiagnosticsPending
            ? Visibility.Visible : Visibility.Collapsed;

        var data = _snapshot.HearingMetadata;
        MetadataResults.Visibility = data is null ? Visibility.Collapsed : Visibility.Visible;
        MetadataEmpty.Visibility = data is null ? Visibility.Visible : Visibility.Collapsed;
        if (data is not null)
        {
            ModeValue.Text = Localize.Get("HardwareChecks.TestModeRaw", data.TestModeRaw?.ToString() ?? "—");
            SensitivityValue.Text = Localize.Get("HardwareChecks.SensitivityRaw", HardwareDiagnosticsPresentation.RawNumber(data.DosimetrySensitivityRaw));
            string curve = HardwareDiagnosticsPresentation.RawCurve(data.DosimetryVolumeCurveRaw);
            CurveDetails.Visibility = curve.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
            CurveValues.Text = curve;
            MetadataReadTime.Text = HardwareDiagnosticsPresentation.Date(data.ReadAtUnixSeconds) is { } read
                ? Localize.Get("HardwareChecks.ReadAt", read.ToLocalTime().ToString("g")) : "";
            MetadataReadTime.Visibility = MetadataReadTime.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        }
        MetadataError.Text = _metadataLocalError ?? _snapshot.HearingMetadataError ?? "";
        MetadataError.Visibility = MetadataError.Text.Length > 0 && _requesting != "metadata" && !_snapshot.HearingMetadataPending
            ? Visibility.Visible : Visibility.Collapsed;
    }

    private static void ShowEar(HardwareDiagnosticEar ear, TextBlock result, TextBlock flags, TextBlock time, TextBlock firstUse)
    {
        result.Text = Localize.Get(HardwareDiagnosticsPresentation.ResultKey(ear));
        var names = HardwareDiagnosticsPresentation.FailureKeys(ear).Select(key => Localize.Get(key)).ToArray();
        flags.Text = names.Length == 0 ? "" : Localize.Get("HardwareChecks.Reported", string.Join(", ", names));
        flags.Visibility = flags.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        time.Text = HardwareDiagnosticsPresentation.Date(ear.MeasuredAtUnixSeconds) is { } measured
            ? Localize.Get("HardwareChecks.DeviceTime", measured.ToLocalTime().ToString("g")) : "";
        time.Visibility = time.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        firstUse.Text = HardwareDiagnosticsPresentation.Date(ear.FirstUseUnixSeconds ?? 0) is { } first
            ? Localize.Get("HardwareChecks.FirstUse", first.ToLocalTime().ToString("g")) : "";
        firstUse.Visibility = firstUse.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void Read_Click(object sender, RoutedEventArgs e)
    {
        if (ReadButton.IsEnabled) BeginRequest("diagnostics", () => Send(new GetHardwareDiagnosticsCmd()));
    }

    private void ReadMetadata_Click(object sender, RoutedEventArgs e)
    {
        if (MetadataReadButton.IsEnabled) BeginRequest("metadata",
            () => Send(new GetHearingMetadataCmd { DeviceId = _snapshot.DeviceId }));
    }

    private async void Run_Click(object sender, RoutedEventArgs e)
    {
        if (!RunButton.IsEnabled) return;
        string target = _snapshot.DeviceId;
        if (_snapshot.HardwareCheck is { } check && HardwareDiagnosticsPresentation.IsRunning(check))
        {
            BeginRequest("check", () => Send(new CancelHardwareCheckCmd { RequestId = check.RequestId, DeviceId = target }),
                check.RequestId, cancelling: true);
            return;
        }
        long selection = _selectionRevision;
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            RequestedTheme = ActualTheme,
            Title = Localize.Get("HardwareChecks.ConfirmTitle"),
            Content = new TextBlock { Text = Localize.Get("HardwareChecks.ConfirmBody"), TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = Localize.Get("HardwareChecks.Run"),
            CloseButtonText = Localize.Get("Action_Cancel"),
            DefaultButton = ContentDialogButton.Close,
        };
        _confirmation = dialog;
        Render();
        ContentDialogResult result;
        try { result = await dialog.ShowAsync(); }
        catch (System.Runtime.InteropServices.COMException) { return; }
        finally { _confirmation = null; Render(); }
        if (result != ContentDialogResult.Primary || !IsLoaded ||
            !HardwareDiagnosticsPresentation.CanConfirmRun(_snapshot, Client?.IsReady == true, target, selection, _selectionRevision)) return;
        ulong id = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
        BeginRequest("check", () => Send(new StartHardwareCheckCmd
        {
            RequestId = id, DeviceId = target, BudsInCaseConfirmed = true,
        }), id);
    }

    private void BeginRequest(string kind, Func<bool> send, ulong? requestId = null, bool cancelling = false)
    {
        if (!send()) return;
        _requesting = kind;
        _requestTarget = _snapshot.DeviceId;
        _requestId = requestId;
        _cancelling = cancelling;
        if (kind == "metadata") _metadataLocalError = null;
        else _localError = null;
        _timeout.Start();
        Render();
    }

    private void ClearRequest()
    {
        _requesting = _requestTarget = "";
        _requestId = null;
        _cancelling = false;
        _timeout.Stop();
    }

    private static void Help(Control control, string text)
    {
        ToolTipService.SetToolTip(control, text);
        AutomationProperties.SetHelpText(control, text);
    }

    private void ApplyResult(DaemonEvent.CommandResult result) => DispatcherQueue.TryEnqueue(() =>
    {
        if (_requesting != "check" || result.RequestId != _requestId || _requestTarget != _snapshot.DeviceId ||
            result.Command != (_cancelling ? "cancel_hardware_check" : "start_hardware_check") || result.Ok) return;
        ClearRequest();
        _localError = result.Message;
        Render();
    });

    private void ConnectionChanged(bool ready) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!ready)
        {
            _selectionRevision++;
            _confirmation?.Hide();
            ClearRequest();
        }
        Render();
    });
}
