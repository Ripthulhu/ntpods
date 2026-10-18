using System.ComponentModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class NearbyCaseCard : UserControl
{
    private readonly CaseInformationPresentation _presentation = new();
    private readonly BluetoothCaseInformation _reader = new();
    private Snapshot _snapshot = new();
    private DaemonClient? _client;
    private CancellationTokenSource? _request;
    private Task? _reading;
    private bool _daemonReady = true;
    internal bool CanRead => IsLoaded && _presentation.CanRead && _reading is not { IsCompleted: false };

    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (ReferenceEquals(_client, value)) return;
            _ = StopAsync();
            _client = value;
            Update(_snapshot);
        }
    }

    public NearbyCaseCard()
    {
        InitializeComponent();
        Loaded += (_, _) => { Loc.Instance.PropertyChanged += LanguageChanged; Render(); };
        Unloaded += async (_, _) =>
        {
            Loc.Instance.PropertyChanged -= LanguageChanged;
            await StopAsync();
        };
        Render();
    }

    private void LanguageChanged(object? sender, PropertyChangedEventArgs args) => DispatcherQueue.TryEnqueue(() =>
    {
        if (IsLoaded) Render();
    });

    public void Update(Snapshot snapshot)
    {
        _snapshot = snapshot;
        if (_presentation.Update(snapshot, _daemonReady && Client?.IsReady == true)) _request?.Cancel();
        Render();
    }

    public void SetDaemonReady(bool ready)
    {
        _daemonReady = ready;
        Update(_snapshot);
    }

    public async Task<bool> StopAsync()
    {
        _presentation.Cancel();
        _request?.Cancel();
        Task? reading = _reading;
        Render();
        if (reading is not null) await reading;
        return _presentation.CleanupConfirmed;
    }

    internal async Task ReadInformationAsync()
    {
        if (!CanRead ||
            _presentation.Product is not { } product) return;
        using var request = new CancellationTokenSource(TimeSpan.FromSeconds(40));
        _request = request;
        int generation = _presentation.Begin();
        Render();
        Task reading = ReadCoreAsync(product, generation, request.Token);
        _reading = reading;
        try { await reading; }
        finally
        {
            if (ReferenceEquals(_request, request)) _request = null;
            if (ReferenceEquals(_reading, reading)) _reading = null;
            Render();
        }
    }

    private async Task ReadCoreAsync(ushort product, int generation, CancellationToken token)
    {
        CaseInformationResult result;
        try { result = await _reader.ReadAsync(product, token); }
        catch (OperationCanceledException) { result = new(null, CaseInformationError.Cancelled); }
        catch (Exception) { result = new(null, CaseInformationError.Unreachable); }
        _presentation.Complete(generation, result);
    }

    private async void Read_Click(object sender, RoutedEventArgs args) => await ReadInformationAsync();

    private async void Panel_Collapsed(Expander sender, ExpanderCollapsedEventArgs args) => await StopAsync();

    private void Render()
    {
        bool busy = _presentation.Busy || _reading is { IsCompleted: false };
        ReadButton.IsEnabled = CanRead;
        ReadLabel.Text = Localize.Get(busy ? "NearbyCase.Reading" : "NearbyCase.Read");
        string help = Localize.Get(busy ? "DeviceSetting.Pending" :
            _snapshot.MicCaptureActive || _snapshot.MicUplinkActive || _snapshot.MicManualActive ||
            _snapshot.MicRecording || _snapshot.MicState is not ("idle" or "unavailable")
                ? "NearbyCase.PauseMic" : "NearbyCase.Description");
        ToolTipService.SetToolTip(ReadButton, help);
        AutomationProperties.SetName(ReadButton, ReadLabel.Text);
        AutomationProperties.SetHelpText(ReadButton, help);
        var metadata = _presentation.Metadata;
        Results.Visibility = metadata is null ? Visibility.Collapsed : Visibility.Visible;
        if (metadata is not null)
        {
            ProtocolValue.Text = CaseInformationPresentation.ProtocolVersion(metadata.ProtocolVersion);
            FirmwareValue.Text = CaseInformationPresentation.Raw(metadata.FirmwareVersion);
            var capabilities = CaseInformationPresentation.CapabilityKeys(metadata.Capabilities).Select(key => Localize.Get(key)).ToArray();
            CapabilitiesValue.Text = capabilities.Length > 0 ? string.Join(", ", capabilities) : Localize.Get("NearbyCase.NoneReported");
            ToolTipService.SetToolTip(ProtocolValue, CaseInformationPresentation.Raw(metadata.ProtocolVersion));
            ToolTipService.SetToolTip(FirmwareValue, CaseInformationPresentation.Raw(metadata.FirmwareVersion));
            ToolTipService.SetToolTip(CapabilitiesValue, CaseInformationPresentation.Raw(metadata.Capabilities));
        }
        string? error = busy ? null : CaseInformationPresentation.ErrorKey(_presentation.Error);
        ErrorText.Text = error is null ? "" : Localize.Get(error);
        ErrorText.Visibility = error is null ? Visibility.Collapsed : Visibility.Visible;
    }
}
