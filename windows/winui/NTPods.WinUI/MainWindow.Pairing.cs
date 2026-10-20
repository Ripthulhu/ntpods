using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;
using Windows.Devices.Enumeration;

namespace NTPods.WinUI;

public sealed partial class MainWindow
{
    private BluetoothPairingDiscovery? _pairingDiscovery;
    private readonly PairingInvitationPolicy _pairingInvitations = new();
    private readonly PairingLidState _pairingLid = new();
    private readonly DispatcherTimer _pairingRetry = new() { Interval = TimeSpan.FromSeconds(45) };
    private readonly DispatcherTimer _pairingBatteryExpiry = new();
    private PairAirPodsDialog? _pairingDialog;
    private string? _pairedPromptAddress;
    private ulong _pairedPromptAttemptStartedAtMs;
    private bool _pairedPromptHeadphone;
    private bool _pairingPromptActive;
    private bool _pairingEligible;
    private bool _pairingShuttingDown;
    internal event Action<string>? NearbyPairingSucceeded;

    private bool PairingWindowVisible => !_closed && !_pairingShuttingDown && RootGrid.IsLoaded && AppWindow.IsVisible &&
        AppWindow.Presenter is not OverlappedPresenter { State: OverlappedPresenterState.Minimized };

    private bool CanDiscoverPairing => !_closed && !_pairingShuttingDown && _pairingDialog is null &&
        _connectPrompt is not { PromptVisible: true } && _lastSnapshot is not { MicCaptureActive: true } &&
        _lastSnapshot is not { MicUplinkActive: true };

    private void InitializePairing()
    {
        _pairingDiscovery = new BluetoothPairingDiscovery(DispatcherQueue);
        _pairingDiscovery.Changed += RefreshPairingInvitation;
        _client.PairingNearbyReceived += OnPairingNearby;
        _pairingBatteryExpiry.Tick += (_, _) =>
        {
            _pairingBatteryExpiry.Stop();
            if (_lastSnapshot is { } snapshot) UpdatePairingSnapshot(snapshot);
        };
        _pairingRetry.Tick += (_, _) =>
        {
            _pairingRetry.Stop();
            if (CanDiscoverPairing && !_pairingDiscovery.IsScanning)
                _pairingDiscovery.Start(TimeSpan.FromSeconds(15));
        };
        DevicePageView.PairingRequested += ShowPairingDialog;
        AppWindow.Changed += (_, args) =>
        {
            if ((args.DidVisibilityChange || args.DidPresenterChange) && !PairingWindowVisible)
                _pairingDialog?.Cancel();
        };
        Closed += (_, _) =>
        {
            StopPairing();
            _client.PairingNearbyReceived -= OnPairingNearby;
            _pairingDiscovery.Dispose();
            _connectPrompt?.Close();
        };
        // MainWindow also exists for --tray launches; discovery does not require Loaded or focus.
        DispatcherQueue.TryEnqueue(UpdatePairingAvailability);
    }

    private ConnectPromptWindow GetConnectPrompt()
    {
        if (_connectPrompt is not null) return _connectPrompt;
        _connectPrompt = new ConnectPromptWindow();
        _connectPrompt.BusyChanged += UpdatePairingAvailability;
        _connectPrompt.Dismissed += () =>
        {
            _pairingPromptActive = false;
            _pairedPromptAddress = null;
            _pairedPromptHeadphone = false;
            _pairingBatteryExpiry.Stop();
            if (_pairingLid.Dismissed() is { } model) ShowLidInvitation(model);
            UpdatePairingAvailability();
        };
        _connectPrompt.Closed += (_, _) =>
        {
            _connectPrompt = null;
            _pairingPromptActive = false;
            _pairedPromptAddress = null;
            _pairedPromptHeadphone = false;
            _pairingBatteryExpiry.Stop();
            _pairingLid.Clear();
            UpdatePairingAvailability();
        };
        return _connectPrompt;
    }

    private void UpdatePairingAvailability()
    {
        if (_pairingDiscovery is null) return;
        bool blocked = _pairingShuttingDown || _closed || _pairingDialog is not null ||
            _lastSnapshot is { MicCaptureActive: true } || _lastSnapshot is { MicUplinkActive: true };
        if (blocked && (_pairedPromptAddress is null || _pairingShuttingDown || _closed || _pairingDialog is not null))
        {
            _pairingLid.Clear();
            _connectPrompt?.HidePrompt();
        }
        if (!CanDiscoverPairing)
        {
            _pairingEligible = false;
            _pairingRetry.Stop();
            if (_pairingDiscovery.IsScanning) _pairingDiscovery.Stop();
        }
        else if (!_pairingEligible)
        {
            _pairingEligible = true;
            _pairingDiscovery.Start(TimeSpan.FromSeconds(15));
        }
    }

    private void RefreshPairingInvitation()
    {
        if (_pairingDiscovery is not { } discovery || !CanDiscoverPairing) return;
        if (!discovery.IsScanning)
        {
            _pairingRetry.Start();
            return;
        }
        _pairingRetry.Stop();
        var candidate = _pairingInvitations.Next(discovery.Candidates);
        if (candidate is null) return;
        try
        {
            _pairingLid.Clear();
            _pairingPromptActive = true;
            var prompt = GetConnectPrompt();
            prompt.ShowPrompt(candidate.Name, token => PairDiscoveredDevice(discovery, candidate.Id, prompt, token,
                    (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()),
                busyStatusKey: PairingInstructions.ForName(candidate.Name));
            UpdatePairingAvailability();
        }
        catch (Exception) { _pairingPromptActive = false; }
    }

    private void OnPairingNearby(ushort model, bool lidOpen) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!lidOpen)
        {
            if (_pairingLid.Close(model)) _connectPrompt?.HidePrompt();
            return;
        }
        if (PairingInvitationPolicy.NearbyName(model) is null) return;
        if (CanDiscoverPairing) ShowLidInvitation(model);
        else if (_pairingPromptActive && _pairedPromptAddress is null && !_pairingLid.Showing &&
            _connectPrompt is { PromptVisible: true, Busy: false })
            ShowLidInvitation(model, replaceClassic: true);
        else _pairingLid.QueueOpen(model);
    });

    private void ShowLidInvitation(ushort model, bool replaceClassic = false)
    {
        if (!CanDiscoverPairing && !replaceClassic || PairingInvitationPolicy.NearbyName(model) is not { } name) return;
        try
        {
            _pairingLid.Show(model);
            _pairingPromptActive = true;
            var prompt = GetConnectPrompt();
            string instructions = PairingInstructions.ForModel(model);
            prompt.ShowPrompt(name, token => PairNearbyDevice(prompt, model, instructions, token),
                busyStatusKey: instructions, model: model);
            UpdatePairingAvailability();
        }
        catch (Exception)
        {
            _pairingPromptActive = false;
            _pairingLid.Clear();
        }
    }

    private async Task<string?> PairNearbyDevice(ConnectPromptWindow prompt, ushort model, string instructions, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        ulong attemptStartedAtMs = (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        using var discovery = new BluetoothPairingDiscovery(DispatcherQueue);
        discovery.Changed += () => _pairingInvitations.Suppress(discovery.Candidates);
        discovery.Start(TimeSpan.FromSeconds(60));
        if (discovery.ErrorKey is { } error) return error;
        string id;
        try
        {
            id = await discovery.WaitForAirPodsAsync(token, multiple =>
            {
                token.ThrowIfCancellationRequested();
                prompt.SetPairingStatus(multiple ? "Pairing.MultipleFound" : instructions);
            }, model);
        }
        catch (InvalidOperationException) when (discovery.ErrorKey is not null)
        {
            return discovery.ErrorKey;
        }
        token.ThrowIfCancellationRequested();
        return await PairDiscoveredDevice(discovery, id, prompt, token, attemptStartedAtMs, model);
    }

    private async Task<string?> PairDiscoveredDevice(BluetoothPairingDiscovery discovery, string id,
        ConnectPromptWindow prompt, CancellationToken token, ulong attemptStartedAtMs, ushort? model = null)
    {
        token.ThrowIfCancellationRequested();
        bool headphone = discovery.Candidates.FirstOrDefault(candidate => candidate.Id == id)?.Name
            .Contains("AirPods Max", StringComparison.OrdinalIgnoreCase) == true;
        prompt.SetPairingStatus("Pairing.Pairing");
        var status = await discovery.PairAsync(id, token, requireAirPods: true, model: model);
        token.ThrowIfCancellationRequested();
        if (status is not (DevicePairingResultStatus.Paired or DevicePairingResultStatus.AlreadyPaired))
            return "Pairing.Failed";
        if (discovery.AddressFor(id) is not { } address || PairingBatteryState.DeviceId(address) is not { } deviceId ||
            !_client.PairingCompleted(address))
            return "Pairing.HandoffUnavailable";
        token.ThrowIfCancellationRequested();
        _pairedPromptAddress = address;
        _pairedPromptAttemptStartedAtMs = attemptStartedAtMs;
        _pairedPromptHeadphone = headphone;
        NearbyPairingSucceeded?.Invoke(deviceId);
        prompt.MarkPaired();
        DevicePageView.QueueNearbyCaseInformation(deviceId, model);
        if (_lastSnapshot is { } snapshot) UpdatePairingSnapshot(snapshot);
        return null;
    }

    private void UpdatePairingSnapshot(Snapshot snapshot)
    {
        if (_pairedPromptAddress is not { } address || _connectPrompt is not { PromptVisible: true } prompt)
            return;
        ulong now = (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        var levels = PairingBatteryState.Read(snapshot, address, _pairedPromptAttemptStartedAtMs, now);
        if (levels is not null) _pairedPromptHeadphone |= DeviceModels.Headphone(snapshot);
        _pairingBatteryExpiry.Stop();
        prompt.UpdatePairedBattery(levels?.Left, levels?.Right, levels?.Case,
            levels?.LeftCharging ?? false, levels?.RightCharging ?? false, levels?.CaseCharging ?? false,
            _pairedPromptHeadphone, levels?.Headphone, levels?.HeadphoneCharging ?? false);
        if (levels?.NextExpiryMs is { } expiry && expiry > now)
        {
            _pairingBatteryExpiry.Interval = TimeSpan.FromMilliseconds(expiry - now + 1);
            _pairingBatteryExpiry.Start();
        }
    }

    private async void ShowPairingDialog()
    {
        if (_pairingDialog is not null || !PairingWindowVisible) return;
        _pairingLid.Clear();
        DevicePageView.CancelShortcutCapture();
        var dialog = new PairAirPodsDialog { XamlRoot = RootGrid.XamlRoot, RequestedTheme = RootGrid.ActualTheme };
        _pairingDialog = dialog;
        UpdatePairingAvailability();
        try
        {
            await dialog.ShowAsync();
            if (_closed || _pairingShuttingDown) return;
            if (dialog.Paired && (dialog.PairedAddress is not { } address || !_client.PairingCompleted(address)))
                DevicePageView.ShowPairingError("Pairing.HandoffUnavailable");
            else if (dialog.Paired && dialog.PairedAddress is { } pairedAddress &&
                PairingBatteryState.DeviceId(pairedAddress) is { } deviceId)
                DevicePageView.QueueNearbyCaseInformation(deviceId, null);
        }
        catch (Exception) { DevicePageView.ShowPairingError(); }
        finally
        {
            dialog.Cancel();
            _pairingDialog = null;
            if (PairingWindowVisible) DevicePageView.FocusActions();
            UpdatePairingAvailability();
        }
    }

    private void StopPairing()
    {
        _pairingLid.Clear();
        _pairingEligible = false;
        _pairingRetry.Stop();
        _pairingBatteryExpiry.Stop();
        _pairedPromptAddress = null;
        _pairedPromptHeadphone = false;
        _pairingDialog?.Cancel();
        _pairingDiscovery?.Stop();
        _connectPrompt?.HidePrompt();
    }
}
