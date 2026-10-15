using System.Collections.ObjectModel;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Windows.Devices.Bluetooth;
using Windows.Devices.Enumeration;
using Windows.Foundation;

namespace NTPods.WinUI.Services;

internal sealed class BluetoothPairingDiscovery(DispatcherQueue dispatcher) : IDisposable
{
    private readonly PairingDiscoveryState _state = new();
    private readonly Dictionary<string, DeviceInformation> _devices = new(StringComparer.Ordinal);
    private readonly DispatcherTimer _timeout = new();
    private DeviceWatcher? _watcher;
    private Action? _unsubscribe;
    private EventHandler<object>? _timeoutHandler;
    private bool _disposed;

    public ObservableCollection<PairingCandidate> Candidates => _state.Candidates;
    public bool IsScanning => _state.Scanning;
    public string? ErrorKey { get; private set; }
    public event Action? Changed;

    public void Start(TimeSpan duration)
    {
        if (_disposed) return;
        Stop();
        ErrorKey = null;
        _devices.Clear();
        int generation = _state.Begin();
        try
        {
            var watcher = DeviceInformation.CreateWatcher(
                BluetoothDevice.GetDeviceSelectorFromPairingState(false),
                new[] { "System.Devices.Aep.IsPresent", "System.Devices.Aep.DeviceAddress",
                    "System.Devices.Aep.Bluetooth.Cod.Major", "System.Devices.Aep.Bluetooth.Cod.Minor" },
                DeviceInformationKind.AssociationEndpoint);
            _watcher = watcher;
            void Dispatch(Action action) => dispatcher.TryEnqueue(() =>
            {
                if (_disposed || !_state.Accepts(generation)) return;
                try { action(); }
                catch (Exception)
                {
                    ErrorKey = "Pairing.ScanFailed";
                    Stop();
                }
            });
            TypedEventHandler<DeviceWatcher, DeviceInformation> added = (_, info) => Dispatch(() =>
            {
                if (_devices.Count >= 128 && !_devices.ContainsKey(info.Id)) return;
                _devices[info.Id] = info;
                Update(generation, info);
            });
            TypedEventHandler<DeviceWatcher, DeviceInformationUpdate> updated = (_, update) => Dispatch(() =>
            {
                if (!_devices.TryGetValue(update.Id, out var info)) return;
                info.Update(update);
                Update(generation, info);
            });
            TypedEventHandler<DeviceWatcher, DeviceInformationUpdate> removed = (_, update) => Dispatch(() =>
            {
                _devices.Remove(update.Id);
                _state.Remove(generation, update.Id);
                Changed?.Invoke();
            });
            TypedEventHandler<DeviceWatcher, object> stopped = (_, _) => Dispatch(() =>
            {
                ErrorKey = "Pairing.ScanFailed";
                Stop();
            });
            watcher.Added += added;
            watcher.Updated += updated;
            watcher.Removed += removed;
            watcher.Stopped += stopped;
            _unsubscribe = () =>
            {
                watcher.Added -= added;
                watcher.Updated -= updated;
                watcher.Removed -= removed;
                watcher.Stopped -= stopped;
            };
            _timeout.Interval = duration;
            _timeoutHandler = (_, _) => { if (_state.Accepts(generation)) Stop(); };
            _timeout.Tick += _timeoutHandler;
            watcher.Start();
            _timeout.Start();
        }
        catch (Exception)
        {
            ErrorKey = "Pairing.ScanFailed";
            Stop();
        }
        Changed?.Invoke();
    }

    private void Update(int generation, DeviceInformation info)
    {
        bool present = !info.Properties.TryGetValue("System.Devices.Aep.IsPresent", out var value) || value is not false;
        ushort? major = info.Properties.TryGetValue("System.Devices.Aep.Bluetooth.Cod.Major", out var majorValue)
            ? majorValue as ushort? : null;
        ushort? minor = info.Properties.TryGetValue("System.Devices.Aep.Bluetooth.Cod.Minor", out var minorValue)
            ? minorValue as ushort? : null;
        _state.Update(generation, info.Id, info.Name, info.Pairing.IsPaired, info.Pairing.CanPair, present, major, minor);
        Changed?.Invoke();
    }

    public async Task<string> WaitForAirPodsAsync(CancellationToken cancellation, Action<bool> ambiguityChanged)
    {
        int generation = _state.Generation;
        bool ambiguous = false;
        while (true)
        {
            cancellation.ThrowIfCancellationRequested();
            if (_disposed || !_state.Accepts(generation))
                throw new InvalidOperationException(ErrorKey ?? "Pairing.ScanFailed");
            string? id = _state.AutomaticCandidate(Environment.TickCount64, out bool multiple);
            if (multiple != ambiguous)
            {
                ambiguous = multiple;
                ambiguityChanged(multiple);
            }
            if (id is not null) return id;
            await Task.Delay(100, cancellation);
        }
    }

    public void Stop()
    {
        _state.Stop();
        _timeout.Stop();
        if (_timeoutHandler is not null) _timeout.Tick -= _timeoutHandler;
        _timeoutHandler = null;
        _unsubscribe?.Invoke();
        _unsubscribe = null;
        var watcher = _watcher;
        _watcher = null;
        try
        {
            if (watcher?.Status is DeviceWatcherStatus.Started or DeviceWatcherStatus.EnumerationCompleted)
                watcher.Stop();
        }
        catch (Exception) { }
        Changed?.Invoke();
    }

    public async Task<DevicePairingResultStatus> PairAsync(string id, CancellationToken cancellation)
    {
        if (_disposed || _state.Find(id) is null || !_devices.ContainsKey(id))
            return DevicePairingResultStatus.NotReadyToPair;
        Stop();
        var status = DevicePairingResultStatus.NotReadyToPair;
        await PairingRetry.WaitAsync(async token =>
        {
            // Discovery can see the case before its pairing gesture has been completed.
            var info = await DeviceInformation.CreateFromIdAsync(id,
                new[] { "System.Devices.Aep.DeviceAddress" }, DeviceInformationKind.AssociationEndpoint).AsTask(token);
            token.ThrowIfCancellationRequested();
            if (_disposed) throw new OperationCanceledException(token);
            if (info is null) return false;
            _devices[id] = info;
            if (info.Pairing.IsPaired) status = DevicePairingResultStatus.AlreadyPaired;
            else if (!info.Pairing.CanPair) status = DevicePairingResultStatus.NotReadyToPair;
            else status = await PairOnceAsync(id, info, token);
            return status is not (DevicePairingResultStatus.NotReadyToPair or
                DevicePairingResultStatus.NotPaired or DevicePairingResultStatus.ConnectionRejected or
                DevicePairingResultStatus.AuthenticationTimeout or DevicePairingResultStatus.AuthenticationFailure or
                DevicePairingResultStatus.OperationAlreadyInProgress);
        }, cancellation);
        return status;
    }

    private static async Task<DevicePairingResultStatus> PairOnceAsync(string id, DeviceInformation info, CancellationToken cancellation)
    {
        var pairing = info.Pairing.Custom;
        TypedEventHandler<DeviceInformationCustomPairing, DevicePairingRequestedEventArgs> requested = (_, args) =>
        {
            if (PairingConfirmation.Allowed(id, args.DeviceInformation.Id,
                args.PairingKind == DevicePairingKinds.ConfirmOnly, cancellation.IsCancellationRequested))
                args.Accept();
        };
        pairing.PairingRequested += requested;
        try
        {
            // Basic PairAsync requires UWP UI; desktop pairing supplies its own confirmation handler.
            return (await pairing.PairAsync(DevicePairingKinds.ConfirmOnly).AsTask(cancellation)).Status;
        }
        finally { pairing.PairingRequested -= requested; }
    }

    public string? AddressFor(string id) =>
        _devices.TryGetValue(id, out var info) &&
        info.Properties.TryGetValue("System.Devices.Aep.DeviceAddress", out var value)
            ? value as string : null;

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        Stop();
        _devices.Clear();
        _state.Close();
        Changed = null;
    }
}
