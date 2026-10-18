using System.Diagnostics;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Devices.Enumeration;
using Windows.Foundation;
using Windows.Storage.Streams;

namespace NTPods.WinUI.Services;

internal sealed class BluetoothCaseInformation
{
    private static readonly SemaphoreSlim ReadGate = new(1, 1);
    private static readonly Guid Information = new("0000180a-0000-1000-8000-00805f9b34fb");
    private static readonly Guid Pnp = new("00002a50-0000-1000-8000-00805f9b34fb");
    private static readonly Guid Dult = new("15190001-12f4-c226-88ed-2ac5579f2a85");
    private static readonly Guid Control = new("8e0c0001-1d68-fb92-bf61-48377421680e");
    private static readonly Guid Cccd = new("00002902-0000-1000-8000-00805f9b34fb");
    private static readonly TimeSpan WorkDeadline = TimeSpan.FromSeconds(34), TotalDeadline = TimeSpan.FromSeconds(40);
    private sealed class ReadFailure(CaseInformationError error) : Exception { internal CaseInformationError Error { get; } = error; }
    private readonly record struct Endpoint(ulong Address, BluetoothAddressType Type);

    public async Task<CaseInformationResult> ReadAsync(ushort expectedPnpProduct, CancellationToken cancellationToken)
    {
        if (expectedPnpProduct != 0x2029 || !OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041)) return new(null, CaseInformationError.Unsupported);
        try
        {
            if (!await ReadGate.WaitAsync(0, cancellationToken).ConfigureAwait(false)) return new(null, CaseInformationError.Busy);
        }
        catch (OperationCanceledException) { return new(null, CaseInformationError.Cancelled); }
        try { return await ReadCoreAsync(expectedPnpProduct, cancellationToken).ConfigureAwait(false); }
        finally { ReadGate.Release(); }
    }

    private static async Task<Endpoint> DiscoverAsync(CancellationToken cancellation)
    {
        var watcher = new BluetoothLEAdvertisementWatcher { ScanningMode = BluetoothLEScanningMode.Passive };
        var gate = new object();
        var candidates = new Dictionary<Endpoint, long>();
        var stopped = new TaskCompletionSource<BluetoothError>(TaskCreationOptions.RunContinuationsAsynchronously);
        bool callbacksOpen = true, callbackFailed = false, started = false, stopConfirmed = false, earlyStop = false;
        TypedEventHandler<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs> received = (_, args) =>
        {
            lock (gate)
            {
                if (!callbacksOpen) return;
                try
                {
                    if (!OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041)) return;
                    var age = DateTimeOffset.UtcNow - args.Timestamp;
                    if (age > TimeSpan.FromSeconds(10) || age < -TimeSpan.FromSeconds(5) ||
                        args.AdvertisementType != BluetoothLEAdvertisementType.ConnectableUndirected ||
                        args.BluetoothAddressType is not (BluetoothAddressType.Public or BluetoothAddressType.Random) ||
                        args.BluetoothAddress is 0 or > 0xffffffffffff ||
                        !DultCaseProtocol.AdvertisesCaseService(args.Advertisement.ServiceUuids,
                            args.Advertisement.DataSections.Take(32).Where(section => section.DataType is 0x16 or 0x21 && section.Data.Length <= 512)
                                .Select(section => (section.DataType, Bytes(section.Data))))) return;
                    var endpoint = new Endpoint(args.BluetoothAddress, args.BluetoothAddressType);
                    if (candidates.ContainsKey(endpoint) || candidates.Count < 2) candidates[endpoint] = Stopwatch.GetTimestamp();
                }
                catch (Exception) { callbackFailed = true; }
            }
        };
        TypedEventHandler<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementWatcherStoppedEventArgs> onStopped = (_, args) => stopped.TrySetResult(args.Error);
        watcher.Received += received;
        watcher.Stopped += onStopped;
        try
        {
            cancellation.ThrowIfCancellationRequested();
            watcher.Start();
            started = true;
            var duration = Task.Delay(TimeSpan.FromSeconds(8), cancellation);
            earlyStop = await Task.WhenAny(duration, stopped.Task).ConfigureAwait(false) == stopped.Task;
            if (!earlyStop) await duration.ConfigureAwait(false);
        }
        finally
        {
            lock (gate) callbacksOpen = false;
            try
            {
                if (started || watcher.Status == BluetoothLEAdvertisementWatcherStatus.Started)
                {
                    if (watcher.Status == BluetoothLEAdvertisementWatcherStatus.Started) watcher.Stop();
                    stopConfirmed = await stopped.Task.WaitAsync(TimeSpan.FromSeconds(2)).ConfigureAwait(false) == BluetoothError.Success;
                }
            }
            catch (Exception) { stopConfirmed = false; }
            finally { watcher.Received -= received; watcher.Stopped -= onStopped; }
        }
        cancellation.ThrowIfCancellationRequested();
        if (!stopConfirmed || earlyStop || callbackFailed) throw new ReadFailure(CaseInformationError.Unreachable);
        var error = DultCaseProtocol.CandidateError(candidates.Count);
        if (error != CaseInformationError.None) throw new ReadFailure(error);
        var candidate = candidates.Single();
        if (Stopwatch.GetElapsedTime(candidate.Value) > TimeSpan.FromSeconds(8)) throw new ReadFailure(CaseInformationError.NotFound);
        return candidate.Key;
    }

    private static async Task<T> Step<T>(Func<IAsyncOperation<T>> create, Stopwatch clock, TimeSpan deadline, CancellationToken cancellation = default, Action? unsettledWrite = null)
    {
        cancellation.ThrowIfCancellationRequested();
        var remaining = deadline - clock.Elapsed;
        if (remaining <= TimeSpan.Zero) throw new TimeoutException();
        var operation = create();
        var pending = operation.AsTask();
        try { return await pending.WaitAsync(remaining < TimeSpan.FromSeconds(3) ? remaining : TimeSpan.FromSeconds(3), cancellation).ConfigureAwait(false); }
        catch (Exception)
        {
            try { operation.Cancel(); } catch (Exception) { }
            if (!pending.IsCompleted) unsettledWrite?.Invoke();
            _ = pending.ContinueWith(completed =>
            {
                try
                {
                    if (!completed.IsCompletedSuccessfully) { _ = completed.Exception; return; }
                    if (completed.Result is BluetoothLEDevice device) device.Dispose();
                    if (completed.Result is GattDeviceServicesResult services)
                        foreach (var service in services.Services) { try { service.Dispose(); } catch (Exception) { } }
                }
                catch (Exception) { }
            }, CancellationToken.None, TaskContinuationOptions.ExecuteSynchronously, TaskScheduler.Default);
            throw;
        }
    }

    private static byte[] Bytes(IBuffer buffer)
    {
        if (buffer.Length > 512) throw new ReadFailure(CaseInformationError.InvalidResponse);
        using var reader = DataReader.FromBuffer(buffer);
        var bytes = new byte[buffer.Length];
        reader.ReadBytes(bytes);
        return bytes;
    }
    private static IBuffer Buffer(byte[] bytes)
    {
        using var writer = new DataWriter();
        writer.WriteBytes(bytes);
        return writer.DetachBuffer();
    }
    private static GattDeviceService TakeService(GattDeviceServicesResult result, Guid expected)
    {
        if (result.Status == GattCommunicationStatus.Success && result.Services.Count == 1 && result.Services[0].Uuid == expected)
            return result.Services[0];
        foreach (var service in result.Services) { try { service.Dispose(); } catch (Exception) { } }
        throw new ReadFailure(result.Status != GattCommunicationStatus.Success ? CaseInformationError.Unreachable : result.Services.Count == 0 ? CaseInformationError.Unsupported : CaseInformationError.InvalidResponse);
    }
    private static GattCharacteristic TakeCharacteristic(GattCharacteristicsResult result, Guid expected, GattCharacteristicProperties properties)
    {
        if (result.Status != GattCommunicationStatus.Success) throw new ReadFailure(CaseInformationError.Unreachable);
        if (result.Characteristics.Count == 0) throw new ReadFailure(CaseInformationError.Unsupported);
        if (result.Characteristics.Count != 1 || result.Characteristics[0].Uuid != expected || !result.Characteristics[0].CharacteristicProperties.HasFlag(properties))
            throw new ReadFailure(CaseInformationError.InvalidResponse);
        return result.Characteristics[0];
    }
    private static void RequireUnpaired(DeviceInformation info)
    {
        if (info.Pairing.IsPaired) throw new ReadFailure(CaseInformationError.Paired);
        if (info.Kind != DeviceInformationKind.AssociationEndpoint) throw new ReadFailure(CaseInformationError.InvalidResponse);
    }

    private static async Task<CaseInformationResult> ReadCoreAsync(ushort product, CancellationToken parentCancellation)
    {
        var clock = Stopwatch.StartNew();
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(parentCancellation);
        BluetoothLEDevice? device = null;
        GattDeviceService? information = null, service = null;
        GattCharacteristic? control = null;
        GattDescriptor? descriptor = null;
        var gate = new object();
        DultResponsePhase? phase = null;
        bool callbacksOpen = true, callbackFailed = false, disconnected = false, sawConnected = false;
        bool valueAttached = false, statusAttached = false, restoreRequired = false, cleanupFailed = false, unsettledSubscription = false;
        ushort originalCccd = 0;
        int notifications = 0;
        CaseMetadata? metadata = null;
        var error = CaseInformationError.None;
        void CancelRead() { try { cancellation.Cancel(); } catch (Exception) { callbackFailed = true; } }
        void RequireLive()
        {
            cancellation.Token.ThrowIfCancellationRequested();
            if (callbackFailed || disconnected || device?.ConnectionStatus != BluetoothConnectionStatus.Connected) throw new ReadFailure(CaseInformationError.Unreachable);
            RequireUnpaired(device.DeviceInformation);
            sawConnected = true;
        }
        void Release(Action dispose) { try { dispose(); } catch (Exception) { cleanupFailed = true; } }
        TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs> onValue = (_, args) =>
        {
            lock (gate)
            {
                if (!callbacksOpen) return;
                try
                {
                    if (++notifications > 128) throw new ReadFailure(CaseInformationError.InvalidResponse);
                    phase?.Accept(Bytes(args.CharacteristicValue));
                }
                catch (Exception) { callbackFailed = true; CancelRead(); }
            }
        };
        TypedEventHandler<BluetoothLEDevice, object> onStatus = (sender, _) =>
        {
            lock (gate)
            {
                if (!callbacksOpen) return;
                try
                {
                    if (sender.ConnectionStatus == BluetoothConnectionStatus.Connected) sawConnected = true;
                    else if (sawConnected) { disconnected = true; CancelRead(); }
                }
                catch (Exception) { callbackFailed = true; CancelRead(); }
            }
        };
        try
        {
            var endpoint = await DiscoverAsync(cancellation.Token).ConfigureAwait(false);
            device = await Step(() => BluetoothLEDevice.FromBluetoothAddressAsync(endpoint.Address, endpoint.Type), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (device is null || device.BluetoothAddress != endpoint.Address || device.BluetoothAddressType != endpoint.Type) throw new ReadFailure(CaseInformationError.Unreachable);
            RequireUnpaired(device.DeviceInformation);
            // The device object can retain an unpaired flag after Windows has saved a bond.
            var beforeDiscovery = await Step(() => DeviceInformation.CreateFromIdAsync(device.DeviceInformation.Id, Array.Empty<string>(), device.DeviceInformation.Kind), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (beforeDiscovery is null || beforeDiscovery.Id != device.DeviceInformation.Id) throw new ReadFailure(CaseInformationError.InvalidResponse);
            RequireUnpaired(beforeDiscovery);
            sawConnected = device.ConnectionStatus == BluetoothConnectionStatus.Connected;
            device.ConnectionStatusChanged += onStatus; statusAttached = true;
            information = TakeService(await Step(() => device.GetGattServicesForUuidAsync(Information, BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false), Information);
            var identity = TakeCharacteristic(await Step(() => information.GetCharacteristicsForUuidAsync(Pnp, BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false), Pnp, GattCharacteristicProperties.Read);
            var pnp = await Step(() => identity.ReadValueAsync(BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (pnp.Status != GattCommunicationStatus.Success) throw new ReadFailure(CaseInformationError.Unreachable);
            if (!DultCaseProtocol.MatchesIdentity(Bytes(pnp.Value), product)) throw new ReadFailure(CaseInformationError.Unsupported);
            RequireLive();
            service = TakeService(await Step(() => device.GetGattServicesForUuidAsync(Dult, BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false), Dult);
            control = TakeCharacteristic(await Step(() => service.GetCharacteristicsForUuidAsync(Control, BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false), Control, GattCharacteristicProperties.Write | GattCharacteristicProperties.Indicate);
            var descriptors = await Step(() => control.GetDescriptorsForUuidAsync(Cccd, BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (descriptors.Status != GattCommunicationStatus.Success) throw new ReadFailure(CaseInformationError.Unreachable);
            if (descriptors.Descriptors.Count != 1 || descriptors.Descriptors[0].Uuid != Cccd) throw new ReadFailure(CaseInformationError.InvalidResponse);
            descriptor = descriptors.Descriptors[0];
            var initial = await Step(() => descriptor.ReadValueAsync(BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (initial.Status != GattCommunicationStatus.Success || !DultCaseProtocol.TryParseCccd(Bytes(initial.Value), out originalCccd)) throw new ReadFailure(CaseInformationError.InvalidResponse);
            var fresh = await Step(() => DeviceInformation.CreateFromIdAsync(device.DeviceInformation.Id, Array.Empty<string>(), device.DeviceInformation.Kind), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (fresh is null || fresh.Id != device.DeviceInformation.Id) throw new ReadFailure(CaseInformationError.InvalidResponse);
            RequireUnpaired(fresh);
            RequireLive();
            control.ValueChanged += onValue; valueAttached = true;
            restoreRequired = true;
            var enabled = await Step(() => control.WriteClientCharacteristicConfigurationDescriptorWithResultAsync(GattClientCharacteristicConfigurationDescriptorValue.Indicate), clock, WorkDeadline, cancellation.Token,
                unsettledWrite: () => unsettledSubscription = true).ConfigureAwait(false);
            if (enabled.Status != GattCommunicationStatus.Success) throw new ReadFailure(CaseInformationError.Unreachable);
            var active = await Step(() => descriptor.ReadValueAsync(BluetoothCacheMode.Uncached), clock, WorkDeadline, cancellation.Token).ConfigureAwait(false);
            if (active.Status != GattCommunicationStatus.Success || !DultCaseProtocol.TryParseCccd(Bytes(active.Value), out var current) || current != 2) throw new ReadFailure(CaseInformationError.InvalidResponse);
            var values = new Dictionary<ushort, uint>();
            foreach (ushort opcode in new ushort[] { DultCaseProtocol.ProtocolVersion, DultCaseProtocol.Capabilities, DultCaseProtocol.FirmwareVersion })
            {
                RequireLive();
                var deadline = clock.Elapsed + TimeSpan.FromSeconds(3);
                if (deadline > WorkDeadline) deadline = WorkDeadline;
                var request = new DultResponsePhase(opcode);
                lock (gate) phase = request;
                var written = await Step(() =>
                {
                    RequireLive();
                    lock (gate) request.Issue();
                    return control.WriteValueWithResultAsync(Buffer(DultCaseProtocol.Request(opcode)), GattWriteOption.WriteWithResponse);
                }, clock, deadline, cancellation.Token).ConfigureAwait(false);
                if (written.Status != GattCommunicationStatus.Success) throw new ReadFailure(CaseInformationError.Unreachable);
                var remaining = deadline - clock.Elapsed;
                if (remaining <= TimeSpan.Zero) throw new TimeoutException();
                var response = await request.Pending.WaitAsync(remaining, cancellation.Token).ConfigureAwait(false);
                RequireLive();
                lock (gate) phase = null;
                if (response.Value is not uint value) throw new ReadFailure(CaseInformationError.Unsupported);
                values.Add(opcode, value);
            }
            metadata = new(values[DultCaseProtocol.ProtocolVersion], values[DultCaseProtocol.Capabilities], values[DultCaseProtocol.FirmwareVersion]);
        }
        catch (ReadFailure failure) { error = failure.Error; }
        catch (OperationCanceledException) { error = parentCancellation.IsCancellationRequested ? CaseInformationError.Cancelled : CaseInformationError.Unreachable; }
        catch (Exception) { error = CaseInformationError.Unreachable; }
        finally
        {
            lock (gate) phase = null;
            bool restored = await DultCaseProtocol.RestoreAsync(restoreRequired, async () =>
            {
                var result = await Step(() => control!.WriteClientCharacteristicConfigurationDescriptorWithResultAsync((GattClientCharacteristicConfigurationDescriptorValue)originalCccd), clock, TotalDeadline).ConfigureAwait(false);
                return result.Status == GattCommunicationStatus.Success;
            }, async () =>
            {
                var result = await Step(() => descriptor!.ReadValueAsync(BluetoothCacheMode.Uncached), clock, TotalDeadline).ConfigureAwait(false);
                return result.Status == GattCommunicationStatus.Success && DultCaseProtocol.TryParseCccd(Bytes(result.Value), out var value) && value == originalCccd;
            }).ConfigureAwait(false);
            lock (gate) callbacksOpen = false;
            if (valueAttached) Release(() => control!.ValueChanged -= onValue);
            if (statusAttached) Release(() => device!.ConnectionStatusChanged -= onStatus);
            Release(() => service?.Dispose());
            Release(() => information?.Dispose());
            Release(() => device?.Dispose());
            if (!restored || cleanupFailed || unsettledSubscription) error = CaseInformationError.RestoreUncertain;
        }
        if (error == CaseInformationError.None && (parentCancellation.IsCancellationRequested || disconnected || callbackFailed || clock.Elapsed >= TotalDeadline))
            error = parentCancellation.IsCancellationRequested ? CaseInformationError.Cancelled : CaseInformationError.Unreachable;
        return new(error == CaseInformationError.None ? metadata : null, error);
    }
}
