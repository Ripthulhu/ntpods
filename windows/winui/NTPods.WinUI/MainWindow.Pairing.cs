using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using NTPods.WinUI.Controls;
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
    private PairAirPodsDialog? _pairingDialog;
    private bool _pairingPromptActive;
    private bool _pairingEligible;
    private bool _pairingShuttingDown;

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
            if (_pairingLid.Dismissed() is { } model) ShowLidInvitation(model);
            UpdatePairingAvailability();
        };
        _connectPrompt.Closed += (_, _) =>
        {
            _connectPrompt = null;
            _pairingPromptActive = false;
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
        if (blocked)
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
            prompt.ShowPrompt(candidate.Name, token => PairDiscoveredDevice(discovery, candidate.Id, token),
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
        else _pairingLid.QueueOpen(model);
    });

    private void ShowLidInvitation(ushort model)
    {
        if (!CanDiscoverPairing || PairingInvitationPolicy.NearbyName(model) is not { } name) return;
        try
        {
            _pairingLid.Show(model);
            _pairingPromptActive = true;
            var prompt = GetConnectPrompt();
            prompt.ShowPrompt(name, token => PairNearbyDevice(prompt, PairingInstructions.ForName(name), token),
                busyStatusKey: PairingInstructions.ForName(name));
            UpdatePairingAvailability();
        }
        catch (Exception)
        {
            _pairingPromptActive = false;
            _pairingLid.Clear();
        }
    }

    private async Task<string?> PairNearbyDevice(ConnectPromptWindow prompt, string instructions, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
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
            });
        }
        catch (InvalidOperationException) when (discovery.ErrorKey is not null)
        {
            return discovery.ErrorKey;
        }
        token.ThrowIfCancellationRequested();
        return await PairDiscoveredDevice(discovery, id, token);
    }

    private async Task<string?> PairDiscoveredDevice(BluetoothPairingDiscovery discovery, string id, CancellationToken token)
    {
        var status = await discovery.PairAsync(id, token);
        token.ThrowIfCancellationRequested();
        if (status is not (DevicePairingResultStatus.Paired or DevicePairingResultStatus.AlreadyPaired))
            return "Pairing.Failed";
        return discovery.AddressFor(id) is { } address && _client.PairingCompleted(address)
            ? null : "Pairing.HandoffUnavailable";
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
        _pairingDialog?.Cancel();
        _pairingDiscovery?.Stop();
        _connectPrompt?.HidePrompt();
    }
}
