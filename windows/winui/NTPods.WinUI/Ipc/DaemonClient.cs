using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Channels;
using Microsoft.Win32.SafeHandles;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Ipc;

/// Both pipes and their command queue belong to one connection. Commands never survive reconnecting.
public sealed class DaemonClient : IDisposable
{
    public event Action<Snapshot>? SnapshotReceived;
    public event Action<string, string>? OverlayReceived;
    public event Action<string>? ConnectPromptReceived;
    public event Action<ushort, bool>? PairingNearbyReceived;
    public event Action<DaemonEvent.PairedLid>? PairedLidReceived;
    public event Action<DaemonEvent.CommandResult>? CommandResultReceived;
    public event Action<CompanionStatus>? CompanionStatusReceived;
    public event Action<CompanionPreferences>? CompanionPreferencesReceived;
    public event Action<DaemonEvent.HeadTracking>? HeadTrackingReceived;
    public event Action<bool>? ConnectionChanged;

    private sealed record Outgoing(byte[] Bytes, TaskCompletionSource<bool>? Delivered = null, CancellationToken Expires = default);

    private sealed class Session(NamedPipeClientStream events, NamedPipeClientStream commands, CancellationTokenSource cancellation, int processId)
    {
        public readonly NamedPipeClientStream Events = events;
        public readonly NamedPipeClientStream Commands = commands;
        public readonly CancellationTokenSource Cancellation = cancellation;
        public readonly int ProcessId = processId;
        public readonly Channel<Outgoing> Outgoing = Channel.CreateBounded<Outgoing>(256);
        public bool Ready;
    }

    private readonly object _gate = new();
    private readonly CancellationTokenSource _cts = new();
    private readonly string _eventsPipe;
    private readonly string _commandsPipe;
    private readonly Func<Process?> _spawnDaemon;
    private readonly Func<bool> _daemonIsRunning;
    private Process? _spawnedDaemon;
    private Process? _connectedDaemon;
    private Session? _session;
    private ulong _sessionEpoch;
    private TaskCompletionSource<Session> _nextReady = NewReady();
    private Task? _run;
    private bool _stopping;

    public DaemonClient(bool startDaemon = true) : this("NTPods-events", "NTPods-cmds",
        startDaemon ? TrySpawnDaemon : () => null, DaemonIsRunning) { }

    internal DaemonClient(string eventsPipe, string commandsPipe, Func<Process?> spawnDaemon, Func<bool>? daemonIsRunning = null)
    {
        _eventsPipe = eventsPipe;
        _commandsPipe = commandsPipe;
        _spawnDaemon = spawnDaemon;
        _daemonIsRunning = daemonIsRunning ?? (() => false);
    }

    private static TaskCompletionSource<Session> NewReady() => new(TaskCreationOptions.RunContinuationsAsynchronously);
    public bool IsReady { get { lock (_gate) return !_stopping && _session is { Ready: true }; } }
    public ulong SessionEpoch { get { lock (_gate) return _sessionEpoch; } }

    public void Start()
    {
        if (_run is null) _run = RunAsync(_cts.Token);
    }

    public bool Send(object command)
    {
        Session? stalled = null;
        lock (_gate)
        {
            if (_stopping || _session is not { Ready: true } session) return false;
            if (session.Outgoing.Writer.TryWrite(new Outgoing(Encode(command)))) return true;
            stalled = session;
        }
        // A blocked command pipe must not build an ever-growing backlog behind a healthy events pipe.
        try { stalled.Cancellation.Cancel(); } catch (ObjectDisposedException) { }
        return false;
    }

    private static byte[] Encode(object command) => Encoding.UTF8.GetBytes(Wire.ToLine(command));
    public void RequestState() => Send(new GetStateCmd());
    public bool RequestCompanionStatus() => Send(new CompanionStatusCmd());
    public bool SetCompanionPreference(string key, object value, string? deviceId = null, ulong requestId = 0)
    {
        var payload = System.Text.Json.JsonSerializer.SerializeToElement(value, Wire.Json);
        return CompanionPreference.ValidValue(key, payload) && Send(new CompanionSetPreferenceCmd
            { Key = key, Value = payload, DeviceId = deviceId, RequestId = requestId });
    }
    public void SetAnc(byte mode, string? deviceId = null) => Send(new SetAncCmd { Mode = mode, DeviceId = deviceId });
    public void SetMicMode(bool auto, bool manual) => Send(new SetMicModeCmd { Auto = auto, Manual = manual });
    public void SetFeature(byte feature, bool on) => Send(new SetFeatureCmd { Feature = feature, On = on });
    public void SetControl(byte id, byte value) => Send(new SetControlCmd { Id = id, Value = value });
    public void SetDeviceControl(byte id, uint value, string? deviceId = null) => Send(new SetDeviceControlCmd { Id = id, Value = value, DeviceId = deviceId });
    public void SetEq(Equalizer eq) => Send(new SetEqCmd { Mode = eq.Mode, Low = eq.Low, Mid = eq.Mid, High = eq.High });
    public void SetAutoPause(bool on) => Send(new SetAutoPauseCmd { On = on });
    public void RefreshDeviceSettings() => Send(new RefreshDeviceSettingsCmd());
    public void StepVolume(int delta) => Send(new StepVolumeCmd { Delta = delta });
    public void ToggleMute() => Send(new ToggleMuteCmd());
    public void SetHeartRate(bool on) => Send(new SetHeartRateCmd { On = on });
    public void Connect(string? deviceId = null) => Send(new ConnectCmd { DeviceId = deviceId });
    public bool PairingCompleted(string address) => Send(new PairingCompletedCmd { Address = address });
    public void Disconnect() => Send(new DisconnectCmd());
    public void RepairConnection() => Send(new RepairConnectionCmd());
    public void RebuildAudio() => Send(new RebuildAudioCmd());
    public bool SetName(string name, ulong requestId) => Send(new SetNameCmd { Name = name, RequestId = requestId });

    public async Task ShutdownAsync(TimeSpan? recoveryTimeout = null)
    {
        Task<Session> ready;
        lock (_gate)
        {
            _stopping = true;
            ready = _session is { Ready: true } session ? Task.FromResult(session) : _nextReady.Task;
        }
        ConnectionChanged?.Invoke(false);
        try
        {
            var session = await ready.WaitAsync(TimeSpan.FromSeconds(5), _cts.Token).ConfigureAwait(false);
            using var process = Process.GetProcessById(session.ProcessId);
            var delivered = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            using var deliveryTimeout = CancellationTokenSource.CreateLinkedTokenSource(_cts.Token);
            deliveryTimeout.CancelAfter(TimeSpan.FromSeconds(5));
            await session.Outgoing.Writer.WriteAsync(new Outgoing(Encode(new ShutdownCmd()), delivered, deliveryTimeout.Token), deliveryTimeout.Token).ConfigureAwait(false);
            await delivered.Task.WaitAsync(deliveryTimeout.Token).ConfigureAwait(false);
            // The backend can spend up to 60 seconds draining capture and restoring stereo.
            await process.WaitForExitAsync(_cts.Token).WaitAsync(recoveryTimeout ?? TimeSpan.FromSeconds(70), _cts.Token).ConfigureAwait(false);
        }
        catch
        {
            // Pipe loss can mean the backend already exited, with nothing left to restore.
            bool exited = false;
            try
            {
                lock (_gate) exited = _spawnedDaemon is not { HasExited: false } &&
                    _connectedDaemon is not { HasExited: false } && !_daemonIsRunning();
            }
            catch { } // An unreadable lifetime guard is not proof that shutdown finished.
            if (exited) return;
            lock (_gate) _stopping = false;
            ConnectionChanged?.Invoke(IsReady);
            RequestState();
            throw;
        }
    }

    private async Task RunAsync(CancellationToken ct)
    {
        SnapshotReceived?.Invoke(new Snapshot());
        ConnectionChanged?.Invoke(false);
        while (!ct.IsCancellationRequested)
        {
            using var events = new NamedPipeClientStream(".", _eventsPipe, PipeDirection.In, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
            using var commands = new NamedPipeClientStream(".", _commandsPipe, PipeDirection.Out, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
            using var connection = CancellationTokenSource.CreateLinkedTokenSource(ct);
            Session? session = null;
            Task? reader = null, writer = null;
            try
            {
                await Task.WhenAll(events.ConnectAsync(500, ct), commands.ConnectAsync(500, ct)).ConfigureAwait(false);
                if (!GetNamedPipeServerProcessId(commands.SafePipeHandle, out uint processId) ||
                    !GetNamedPipeServerProcessId(events.SafePipeHandle, out uint eventsProcessId) || processId != eventsProcessId)
                    throw new IOException("Could not identify the background service.");
                var process = Process.GetProcessById(checked((int)processId));
                try { _ = process.SafeHandle; } // Retain this process identity even after its pipes close.
                catch { process.Dispose(); throw; }
                session = new Session(events, commands, connection, checked((int)processId));
                lock (_gate)
                {
                    _connectedDaemon?.Dispose();
                    _connectedDaemon = process;
                    _session = session;
                    _sessionEpoch++;
                }
                reader = ReadSessionAsync(session);
                writer = WriteSessionAsync(session);
                await await Task.WhenAny(reader, writer).ConfigureAwait(false);
            }
            catch (TimeoutException)
            {
                lock (_gate)
                {
                    if (!_stopping && !ct.IsCancellationRequested && _spawnedDaemon is not { HasExited: false })
                    {
                        _spawnedDaemon?.Dispose();
                        _spawnedDaemon = _spawnDaemon();
                    }
                }
            }
            catch (OperationCanceledException) { }
            catch (Exception) { }
            finally
            {
                lock (_gate)
                {
                    if (ReferenceEquals(_session, session)) _session = null;
                    if (_nextReady.Task.IsCompleted) _nextReady = NewReady();
                }
                connection.Cancel();
                events.Dispose();
                commands.Dispose();
                if (session is not null)
                {
                    session.Outgoing.Writer.TryComplete();
                    while (session.Outgoing.Reader.TryRead(out var pending))
                        pending.Delivered?.TrySetException(new IOException("The background service disconnected."));
                    SnapshotReceived?.Invoke(new Snapshot());
                    CompanionStatusReceived?.Invoke(new CompanionStatus());
                    CompanionPreferencesReceived?.Invoke(new CompanionPreferences());
                    ConnectionChanged?.Invoke(false);
                }
                try { if (reader is not null && writer is not null) await Task.WhenAll(reader, writer).ConfigureAwait(false); }
                catch (Exception) { }
            }
            try { await Task.Delay(500, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
        }
    }

    private async Task ReadSessionAsync(Session session)
    {
        using var reader = new StreamReader(session.Events, Encoding.UTF8, detectEncodingFromByteOrderMarks: false, leaveOpen: true);
        while (await reader.ReadLineAsync(session.Cancellation.Token).ConfigureAwait(false) is { } line)
        {
            var parsed = Wire.ParseEvent(line);
            CompanionUiTrace.Wire(line, parsed, SessionEpoch);
            switch (parsed)
            {
                case DaemonEvent.State state:
                    lock (_gate)
                    {
                        if (!ReferenceEquals(_session, session)) return;
                        session.Ready = true;
                        _nextReady.TrySetResult(session);
                    }
                    SnapshotReceived?.Invoke(state.Snapshot);
                    ConnectionChanged?.Invoke(IsReady);
                    break;
                case DaemonEvent.Overlay overlay:
                    OverlayReceived?.Invoke(overlay.Title, Services.OverlayText.Resolve(overlay.Body));
                    break;
                case DaemonEvent.ConnectPrompt prompt:
                    ConnectPromptReceived?.Invoke(prompt.Name);
                    break;
                case DaemonEvent.PairingNearby nearby:
                    PairingNearbyReceived?.Invoke(nearby.Model, nearby.LidOpen);
                    break;
                case DaemonEvent.PairedLid lid:
                    PairedLidReceived?.Invoke(lid);
                    break;
                case DaemonEvent.CommandResult result:
                    CompanionUiTrace.Write("typed_result", new { epoch = SessionEpoch,
                        command = result.Command == "companion_switch" ? "companion_switch" : "other", request_id = result.RequestId });
                    CommandResultReceived?.Invoke(result);
                    break;
                case DaemonEvent.HeadTracking movement:
                    HeadTrackingReceived?.Invoke(movement);
                    break;
                case DaemonEvent.Preferences preferences:
                    lock (_gate) { if (!ReferenceEquals(_session, session)) return; }
                    CompanionPreferencesReceived?.Invoke(preferences.Value);
                    break;
                case DaemonEvent.Companion companion:
                    lock (_gate) { if (!ReferenceEquals(_session, session)) return; }
                    CompanionStatusReceived?.Invoke(companion.Status);
                    break;
            }
        }
    }

    private async Task WriteSessionAsync(Session session)
    {
        var ct = session.Cancellation.Token;
        await session.Commands.WriteAsync(Encode(new HelloCmd()), ct).ConfigureAwait(false);
        await session.Commands.WriteAsync(Encode(new GetStateCmd()), ct).ConfigureAwait(false);
        await session.Commands.FlushAsync(ct).ConfigureAwait(false);
        while (!ct.IsCancellationRequested)
        {
            Outgoing message;
            using (var idle = CancellationTokenSource.CreateLinkedTokenSource(ct))
            {
                idle.CancelAfter(TimeSpan.FromSeconds(2));
                try { message = await session.Outgoing.Reader.ReadAsync(idle.Token).ConfigureAwait(false); }
                catch (OperationCanceledException) when (!ct.IsCancellationRequested)
                {
                    // Detect a lost command pipe even if events continue and the user is idle.
                    message = new Outgoing(Encode(new PingCmd()));
                }
            }
            lock (_gate) { if (_stopping && message.Delivered is null) continue; }
            if (message.Expires.IsCancellationRequested)
            {
                message.Delivered?.TrySetCanceled(message.Expires);
                continue;
            }
            try
            {
                using var write = CancellationTokenSource.CreateLinkedTokenSource(ct, message.Expires);
                await session.Commands.WriteAsync(message.Bytes, write.Token).ConfigureAwait(false);
                await session.Commands.FlushAsync(write.Token).ConfigureAwait(false);
                message.Delivered?.TrySetResult(true);
            }
            catch (Exception error)
            {
                message.Delivered?.TrySetException(error);
                throw;
            }
        }
    }

    private static Process? TrySpawnDaemon()
    {
        try
        {
            var path = FindDaemon();
            if (path is null) return null;
            return Process.Start(new ProcessStartInfo { FileName = path, UseShellExecute = false, CreateNoWindow = true, WorkingDirectory = Path.GetDirectoryName(path)! });
        }
        catch { }
        return null;
    }

    private static bool DaemonIsRunning()
    {
        try
        {
            bool exists = Mutex.TryOpenExisting(@"Local\NTPodsDaemonSingleton", out var mutex);
            mutex?.Dispose();
            return exists;
        }
        catch (UnauthorizedAccessException) { return true; }
    }

    private static string? FindDaemon()
    {
        string[] candidates =
        [
            Path.Combine(AppContext.BaseDirectory, "ntpodsd.exe"),
            Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "ntpodsd.exe")),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "NTPods", "ntpodsd.exe"),
        ];
        return candidates.FirstOrDefault(File.Exists);
    }

    public void Dispose()
    {
        try { _cts.Cancel(); } catch (ObjectDisposedException) { }
        // RunAsync owns the pipe handles and finishes cancellation on its background continuation.
    }

    public async ValueTask DisposeAsync()
    {
        Dispose();
        if (_run is not null) await _run.ConfigureAwait(false);
        _spawnedDaemon?.Dispose();
        _connectedDaemon?.Dispose();
        _cts.Dispose();
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint serverProcessId);
}
