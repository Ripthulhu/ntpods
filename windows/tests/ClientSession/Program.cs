using System.Diagnostics;
using System.Collections.Concurrent;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Controls;

static void Check(bool value, string message)
{
    if (!value) throw new Exception(message);
}

static async Task Until(Func<bool> predicate)
{
    using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
    while (!predicate()) await Task.Delay(10, timeout.Token);
}

if (args is ["--commands-only", var commandName])
{
    using var commands = new NamedPipeServerStream(commandName + "-cmds", PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
    Task connected = commands.WaitForConnectionAsync();
    Console.WriteLine("commands-ready");
    await Console.Out.FlushAsync();
    await connected.WaitAsync(TimeSpan.FromSeconds(10));
    using var reader = new StreamReader(commands);
    Check(await reader.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10)) is null, "Mismatched servers must not receive a handshake or command.");
    return;
}

if (args is ["--fixture", var childName, var holdText])
{
    using var fixture = new Fixture(childName);
    await fixture.Connect();
    await fixture.ReadHandshake();
    await fixture.State();
    while (await fixture.Read() is { } message)
    {
        if (JsonDocument.Parse(message).RootElement.GetProperty("cmd").GetString() != "shutdown") continue;
        // Stand in for the backend's asynchronous audio recovery before process exit.
        await Task.Delay(int.Parse(holdText));
        return;
    }
    return;
}

if (args is ["--starting"])
{
    await Task.Delay(TimeSpan.FromSeconds(30));
    return;
}

if (args is ["--recovering", var recoveringChildName])
{
    using (var fixture = new Fixture(recoveringChildName))
    {
        await fixture.Connect();
        await fixture.ReadHandshake();
        await fixture.State();
        await fixture.Read();
    }
    await Task.Delay(TimeSpan.FromSeconds(30));
    return;
}

Check(OperatingSystem.IsWindows(), "Named-pipe process identity tests require Windows.");
var companionName = "ntpods-companion-test-" + Guid.NewGuid().ToString("N");
await using (var client = new DaemonClient(companionName + "-events", companionName + "-cmds", () => null))
using (var fixture = new Fixture(companionName))
{
    var controls = new CompanionControls();
    var callbacks = new ConcurrentQueue<Action>();
    var completedIds = new List<ulong>();
    // Exercise the window's queued delivery boundaries, rather than calling Complete directly.
    client.SnapshotReceived += snapshot => callbacks.Enqueue(() => controls.Update(snapshot));
    client.ConnectionChanged += connected =>
    {
        ulong epoch = client.SessionEpoch;
        callbacks.Enqueue(() =>
        {
            if (epoch == client.SessionEpoch) controls.SetSession(epoch, connected && client.IsReady);
        });
    };
    client.CompanionStatusReceived += status =>
    {
        ulong epoch = client.SessionEpoch;
        callbacks.Enqueue(() =>
        {
            if (epoch == client.SessionEpoch && client.IsReady) controls.Apply(status);
        });
    };
    client.CommandResultReceived += result =>
    {
        if (result.Command != "companion_switch") return;
        ulong epoch = client.SessionEpoch;
        callbacks.Enqueue(() =>
        {
            if (epoch != client.SessionEpoch || !client.IsReady) return;
            completedIds.Add(result.RequestId);
            Check(controls.Complete(result, epoch), "A current matching terminal result must reach the pending control.");
        });
    };
    Task DrainUntil(Func<bool> predicate) => Until(() =>
    {
        while (callbacks.TryDequeue(out var callback)) callback();
        return predicate();
    });
    Task Status(string phase) => fixture.Event(JsonSerializer.Serialize(new
    {
        @event = "companion_status",
        status = new { linked = true, connected = true, device_id = "001122334455", phase }
    }));
    client.Start();
    await fixture.Connect();
    await fixture.ReadHandshake();
    await fixture.Event("{\"event\":\"state\",\"device_id\":\"001122334455\",\"connected\":true}");
    await Status("idle");
    await DrainUntil(() => controls.CanSwitch);
    foreach (var (target, id) in new[] { ("android", ulong.MaxValue), ("pc", 9_007_199_254_740_993UL) })
    {
        var command = controls.Begin(target, id);
        Check(command is not null && client.Send(command), "The pending control must send its exact request through the client.");
        using var sent = JsonDocument.Parse((await fixture.Read())!);
        ulong sentId = sent.RootElement.GetProperty("request_id").GetUInt64();
        Check(sentId == id && sent.RootElement.GetProperty("target").GetString() == target,
            "Neither full-width IDs nor odd IDs above 2^53 may change on the command pipe.");
        await Status("acquire_" + target);
        await fixture.Event("{\"event\":\"state\",\"device_id\":\"001122334455\",\"connected\":false}");
        await DrainUntil(() => controls.HelpKey == "Companion.Pending");
        Check(controls.Pending == id && !controls.CanSwitch,
            "A disconnected audio snapshot and repeated ready callback must preserve the in-flight request.");
        await fixture.Event(JsonSerializer.Serialize(new
        {
            @event = "command_result", command = "companion_switch", request_id = sentId,
            ok = true, message = "The destination audio route is connected."
        }));
        await Status("idle");
        await DrainUntil(() => controls.Pending is null && controls.CanSwitch);
        Check(completedIds[^1] == id && controls.HelpKey == "Companion.Help" && controls.ErrorKey is null,
            "Parsed terminal result followed by idle status must clear Switching for both large request IDs.");
    }
    Console.WriteLine("Companion completion pipe-to-queued-control checks passed.");
}
var name = "ntpods-test-" + Guid.NewGuid().ToString("N");
await using (var client = new DaemonClient(name + "-events", name + "-cmds", () => null))
{
    int disconnected = 0;
    ulong firstEpoch = 0, secondEpoch = 0;
    Snapshot? last = null;
    DaemonEvent.PairedLid? lastLid = null;
    CompanionStatus? lastCompanion = null;
    CompanionPreferences? lastPreferences = null;
    client.ConnectionChanged += connected => { if (!connected) Interlocked.Increment(ref disconnected); };
    client.SnapshotReceived += snapshot => Volatile.Write(ref last, snapshot);
    client.PairedLidReceived += lid => Volatile.Write(ref lastLid, lid);
    client.CompanionStatusReceived += status => Volatile.Write(ref lastCompanion, status);
    client.CompanionPreferencesReceived += preferences => Volatile.Write(ref lastPreferences, preferences);
    Check(!client.Send(new SetVolumeCmd { Percent = 99 }), "Offline commands must be rejected.");
    Check(!client.RequestCompanionStatus(), "An offline companion query must not be queued for a later session.");
    using (var first = new Fixture(name))
    {
        client.Start();
        await first.Connect();
        await first.ReadHandshake();
        Check(!client.IsReady && !client.Send(new SetVolumeCmd { Percent = 98 }), "Two connected pipes are not ready before a fresh snapshot.");
        Check(!client.RequestCompanionStatus(), "A companion query requires authoritative state readiness.");
        Check(!client.SetCompanionPreference("theme", "dark"), "Offline preference edits must not enter a reconnecting command queue.");
        await first.State();
        await Until(() => client.IsReady);
        await first.Event("{\"event\":\"companion_preferences\",\"preferences\":{\"device_id\":\"001122334455\",\"records\":[{\"key\":\"call_takeover_enabled\",\"rev\":\"ffffffffffffffff\",\"origin\":\"android\",\"value\":true}],\"local_call_consent\":false,\"peer_prefs\":true,\"peer_calls\":true,\"error\":null}}");
        await Until(() => Volatile.Read(ref lastPreferences) is { Records.Length: 1 });
        Check(Volatile.Read(ref lastPreferences) is { LocalCallTakeoverEnabled: false, PeerCalls: true },
            "A remote enabled call preference reaches the app without granting local consent.");
        Check(client.Send(new PingCmd()), "Receipt-only projection must leave the command queue idle.");
        Check((await first.Read()) == "{\"cmd\":\"ping\"}", "Receiving a preference must not echo it as an edit.");
        Check(client.SetCompanionPreference("conversation_policy", new ConversationPolicy
            { Mode = "relative", First = "43.25", Second = "19.8", PauseMedia = true }, "001122334455", ulong.MaxValue),
            "An explicit selected-pair preference can enter the current ready session.");
        using (var preferenceCommand = JsonDocument.Parse((await first.Read())!))
        {
            var command = preferenceCommand.RootElement;
            Check(command.GetProperty("cmd").GetString() == "companion_set_preference" &&
                command.GetProperty("device_id").GetString() == "001122334455" &&
                command.GetProperty("request_id").GetUInt64() == ulong.MaxValue &&
                command.GetProperty("value").GetProperty("first").GetString() == "43.25",
                "The real writer must preserve canonical strings, selected pair and full request correlation.");
        }
        firstEpoch = client.SessionEpoch;
        Check(firstEpoch > 0, "The first pipe session needs an identity for queued UI callbacks.");
        Check(client.RequestCompanionStatus(), "A ready session can request its companion status.");
        using (var query = JsonDocument.Parse((await first.Read())!))
            Check(query.RootElement.GetProperty("cmd").GetString() == "companion_status",
                "The companion status helper must send the typed query command.");
        await first.Event("{\"event\":\"companion_status\",\"status\":{\"linked\":true,\"connected\":true,\"device_id\":\"001122334455\",\"peer_address\":\"AA:BB:CC:DD:EE:FF\",\"held_away\":true,\"phase\":\"idle\",\"error\":null}}");
        await Until(() => Volatile.Read(ref lastCompanion) is { Linked: true });
        Check(Volatile.Read(ref lastCompanion) is { Linked: true, Connected: true, DeviceId: "001122334455",
            PeerAddress: "AA:BB:CC:DD:EE:FF", HeldAway: true, Phase: "idle", Error: null },
            "Companion status must reach subscribers with its exact owner, link and hold state.");
        foreach (string target in new[] { "pc", "android" })
        {
            Check(client.Send(new CompanionSwitchCmd { DeviceId = "001122334455", Target = target, RequestId = ulong.MaxValue }),
                "Both explicit companion destinations can be sent through a ready session.");
            using var sent = JsonDocument.Parse((await first.Read())!);
            var root = sent.RootElement;
            Check(root.GetProperty("cmd").GetString() == "companion_switch" &&
                root.GetProperty("device_id").GetString() == "001122334455" &&
                root.GetProperty("target").GetString() == target &&
                root.GetProperty("request_id").GetUInt64() == ulong.MaxValue,
                "The companion writer must preserve selected pair, destination and full-width request ID.");
        }
        await first.Event("{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"lid_open\":false,\"sequence\":2}");
        await Until(() => Volatile.Read(ref lastLid) is not null);
        Check(Volatile.Read(ref lastLid) is DaemonEvent.PairedLid("001122334455", false, 2),
            "A paired lid edge must reach the client with its exact owner and order.");
        Check(client.Send(new SetVolumeCmd { Percent = 30 }), "A ready connection accepts commands.");
        Check((await first.Read())!.Contains("\"percent\":30"), "The connected writer must deliver the requested command.");
        int before = Volatile.Read(ref disconnected);
        first.Commands.Dispose();
        // No user command: the idle probe must notice a closed command direction.
        await Until(() => Volatile.Read(ref disconnected) > before);
        Check(!client.IsReady && Volatile.Read(ref last) is { Connected: false }, "Command-pipe loss must invalidate the whole session.");
        Check(Volatile.Read(ref lastPreferences) is { Records.Length: 0, LocalCallTakeoverEnabled: false, PeerCalls: false },
            "Pipe loss must retire preference capabilities and local automation presentation from that session.");
        Check(Volatile.Read(ref lastCompanion) is { Linked: false, Connected: false, DeviceId: null, HeldAway: false },
            "Command-pipe loss must immediately retire the previous session's companion status.");
        Check(!client.Send(new SetVolumeCmd { Percent = 97 }), "Commands during reconnect must not be stored for later.");
    }
    using (var second = new Fixture(name))
    {
        await second.Connect();
        await second.ReadHandshake();
        Check(!client.IsReady, "Reconnecting must wait for new authoritative state.");
        await second.State();
        await Until(() => client.IsReady);
        secondEpoch = client.SessionEpoch;
        Check(secondEpoch > firstEpoch, "A replacement pipe session must retire queued callbacks from the previous one.");
        Check(Volatile.Read(ref lastCompanion) is { Linked: false },
            "A fresh snapshot must not revive companion status from the previous session.");
        await second.Event("{\"event\":\"companion_status\",\"status\":{\"linked\":true,\"connected\":false,\"device_id\":\"aabbccddeeff\",\"held_away\":false,\"phase\":\"waiting\",\"error\":\"unavailable\"}}");
        await Until(() => Volatile.Read(ref lastCompanion) is { DeviceId: "aabbccddeeff" });
        Check(Volatile.Read(ref lastCompanion) is { Connected: false, Phase: "waiting", Error: "unavailable" },
            "Replacement companion status is forwarded without inventing a connected phone.");
        client.Send(new SetVolumeCmd { Percent = 55 });
        Check((await second.Read())!.Contains("\"percent\":55"), "Queued commands from the old session must not replay.");
        int before = Volatile.Read(ref disconnected);
        for (int i = 0; i < 64; i++) client.Send(new SetVolumeCmd { Percent = 40 });
        second.Events.Dispose();
        await Until(() => Volatile.Read(ref disconnected) > before);
        Check(!client.IsReady, "Events-pipe loss must also disable commands.");
        Check(Volatile.Read(ref lastCompanion) is { Linked: false, DeviceId: null },
            "Events-pipe loss must also clear the linked companion and owner.");
    }
    using (var third = new Fixture(name))
    {
        await third.Connect();
        await third.ReadHandshake();
        await third.State();
        await Until(() => client.IsReady);
        Check(client.SessionEpoch > secondEpoch, "Each replacement pipe session needs a newer identity.");
        client.Send(new SetVolumeCmd { Percent = 56 });
        Check((await third.Read())!.Contains("\"percent\":56"), "A burst queued before events loss must not cross into a new session.");
    }
}

for (int attempt = 0; attempt < 5; attempt++)
{
    string mismatch = "ntpods-pid-test-" + Guid.NewGuid().ToString("N");
    using var events = new NamedPipeServerStream(mismatch + "-events", PipeDirection.Out, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
    var start = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardError = true, RedirectStandardOutput = true };
    start.ArgumentList.Add("--commands-only");
    start.ArgumentList.Add(mismatch);
    using var process = Process.Start(start)!;
    Task<string> childError = process.StandardError.ReadToEndAsync();
    // Arm the child server's accept before the client can connect and reject it.
    // Otherwise Windows can report ERROR_NO_DATA from WaitForConnectionAsync:
    // the correct PID rejection already closed the pipe before accept began.
    Check(await process.StandardOutput.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10)) == "commands-ready",
        "The command fixture must be accepting connections before the mismatch test starts.");
    await using var client = new DaemonClient(mismatch + "-events", mismatch + "-cmds", () => null);
    int ready = 0;
    client.ConnectionChanged += connected => { if (connected) Interlocked.Increment(ref ready); };
    client.Start();
    await events.WaitForConnectionAsync().WaitAsync(TimeSpan.FromSeconds(10));
    await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(10));
    Check(process.ExitCode == 0 && ready == 0 && !client.IsReady,
        $"Pipes from two backend processes must never form a ready session. Exit={process.ExitCode}, ready={ready}, IsReady={client.IsReady}. {await childError}");
}

foreach (bool timeout in new[] { false, true })
{
    string fixtureName = "ntpods-shutdown-test-" + Guid.NewGuid().ToString("N");
    var start = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
    start.ArgumentList.Add("--fixture");
    start.ArgumentList.Add(fixtureName);
    start.ArgumentList.Add("800");
    using var process = Process.Start(start)!;
    int respawns = 0;
    await using var client = new DaemonClient(fixtureName + "-events", fixtureName + "-cmds",
        () => { Interlocked.Increment(ref respawns); return null; });
    client.Start();
    await Until(() => client.IsReady);
    var elapsed = Stopwatch.StartNew();
    if (timeout)
    {
        bool failed = false;
        try { await client.ShutdownAsync(TimeSpan.FromMilliseconds(150)); }
        catch (TimeoutException) { failed = true; }
        Check(failed && !process.HasExited, "An expired shutdown wait must report failure and leave recovery running.");
    }
    else
    {
        await client.ShutdownAsync(TimeSpan.FromSeconds(4));
        Check(process.HasExited && elapsed.ElapsedMilliseconds >= 650, "Quit must wait for backend recovery and actual process exit.");
        Check(respawns == 0, "A successful shutdown must never respawn the daemon.");
    }
    await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(5));
    Check(process.ExitCode == 0, "The fake backend must exit cleanly.");
}

await using (var absent = new DaemonClient("ntpods-absent-" + Guid.NewGuid(), "ntpods-absent-" + Guid.NewGuid(),
    () => throw new Exception("Quit must not start a missing backend.")))
{
    absent.Start();
    await absent.ShutdownAsync();
    Check(!absent.IsReady, "A missing backend must allow Quit without becoming ready.");
}

string crashedName = "ntpods-crashed-" + Guid.NewGuid().ToString("N");
var crashedStart = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
crashedStart.ArgumentList.Add("--fixture");
crashedStart.ArgumentList.Add(crashedName);
crashedStart.ArgumentList.Add("100");
using (var process = Process.Start(crashedStart)!)
await using (var client = new DaemonClient(crashedName + "-events", crashedName + "-cmds", () => null, () => !process.HasExited))
{
    client.Start();
    await Until(() => client.IsReady);
    process.Kill();
    await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(5));
    await Until(() => !client.IsReady);
    await client.ShutdownAsync();
    Check(process.HasExited, "An exited former backend must not prevent Quit.");
}

string recoveringName = "ntpods-recovering-" + Guid.NewGuid().ToString("N");
var recoveringStart = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
recoveringStart.ArgumentList.Add("--recovering");
recoveringStart.ArgumentList.Add(recoveringName);
using (var process = Process.Start(recoveringStart)!)
await using (var client = new DaemonClient(recoveringName + "-events", recoveringName + "-cmds", () => null))
{
    try
    {
        client.Start();
        await Until(() => client.IsReady);
        Check(client.Send(new PingCmd()), "The live fixture must receive its disconnect trigger.");
        await Until(() => !client.IsReady);
        bool failed = false;
        try { await client.ShutdownAsync(); }
        catch (TimeoutException) { failed = true; }
        Check(failed && !process.HasExited,
            "An authenticated backend must stay protected after pipe loss, even without a lifetime mutex.");
    }
    finally
    {
        if (!process.HasExited) process.Kill();
        await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(5));
    }
}

foreach (bool spawnedHere in new[] { false, true })
{
    string startingName = "ntpods-starting-" + Guid.NewGuid().ToString("N");
    var start = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
    start.ArgumentList.Add("--starting");
    Process? process = spawnedHere ? null : Process.Start(start)!;
    await using var client = new DaemonClient(startingName + "-events", startingName + "-cmds",
        () => spawnedHere ? process = Process.Start(start)! : null,
        () => !spawnedHere && process is { HasExited: false });
    try
    {
        client.Start();
        await Until(() => process is not null);
        bool failed = false;
        try { await client.ShutdownAsync(); }
        catch (TimeoutException) { failed = true; }
        Check(failed && process is { HasExited: false },
            spawnedHere ? "Quit must wait for a spawned backend that has not created its singleton or pipes yet."
                : "A live backend with unavailable pipes must not be mistaken for an absent backend.");
    }
    finally
    {
        if (process is { HasExited: false }) process.Kill();
        if (process is not null) await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(5));
        if (!spawnedHere) process?.Dispose();
    }
}
Console.WriteLine("IPC session checks passed: readiness, companion forwarding and loss, both pipe failures, no stale replay, shutdown delivery, absent/exited backends, startup guards and bounded recovery failure.");

sealed class Fixture : IDisposable
{
    public NamedPipeServerStream Events { get; }
    public NamedPipeServerStream Commands { get; }
    private readonly StreamReader _reader;
    private readonly StreamWriter _writer;

    public Fixture(string name)
    {
        Events = new NamedPipeServerStream(name + "-events", PipeDirection.Out, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        Commands = new NamedPipeServerStream(name + "-cmds", PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        _reader = new StreamReader(Commands, Encoding.UTF8, false, leaveOpen: true);
        _writer = new StreamWriter(Events, new UTF8Encoding(false), leaveOpen: true);
    }
    public async Task Connect()
    {
        await Task.WhenAll(Events.WaitForConnectionAsync(), Commands.WaitForConnectionAsync()).WaitAsync(TimeSpan.FromSeconds(10));
        _writer.AutoFlush = true;
    }
    public async Task ReadHandshake()
    {
        if (!(await Read())!.Contains("\"cmd\":\"hello\"") || !(await Read())!.Contains("\"cmd\":\"get_state\""))
            throw new Exception("Each new pipe session must begin with its own handshake.");
    }
    public Task State() => _writer.WriteLineAsync("{\"event\":\"state\",\"connected\":true,\"mic_available\":true}");
    public Task Event(string line) => _writer.WriteLineAsync(line);
    public async Task<string?> Read() => await _reader.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10));
    public void Dispose()
    {
        _reader.Dispose();
        try { _writer.Dispose(); } catch (ObjectDisposedException) { }
        Events.Dispose();
        Commands.Dispose();
    }
}

namespace NTPods.WinUI.Services
{
    internal static class OverlayText { public static string Resolve(string body) => body; }
}
