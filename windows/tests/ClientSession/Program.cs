using System.Diagnostics;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using NTPods.WinUI.Ipc;

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

Check(OperatingSystem.IsWindows(), "Named-pipe process identity tests require Windows.");
var name = "ntpods-test-" + Guid.NewGuid().ToString("N");
await using (var client = new DaemonClient(name + "-events", name + "-cmds", () => { }))
{
    int disconnected = 0;
    Snapshot? last = null;
    client.ConnectionChanged += connected => { if (!connected) Interlocked.Increment(ref disconnected); };
    client.SnapshotReceived += snapshot => Volatile.Write(ref last, snapshot);
    Check(!client.Send(new SetVolumeCmd { Percent = 99 }), "Offline commands must be rejected.");
    using (var first = new Fixture(name))
    {
        client.Start();
        await first.Connect();
        await first.ReadHandshake();
        Check(!client.IsReady && !client.Send(new SetVolumeCmd { Percent = 98 }), "Two connected pipes are not ready before a fresh snapshot.");
        await first.State();
        await Until(() => client.IsReady);
        Check(client.Send(new SetVolumeCmd { Percent = 30 }), "A ready connection accepts commands.");
        Check((await first.Read())!.Contains("\"percent\":30"), "The connected writer must deliver the requested command.");
        int before = Volatile.Read(ref disconnected);
        first.Commands.Dispose();
        // No user command: the idle probe must notice a closed command direction.
        await Until(() => Volatile.Read(ref disconnected) > before);
        Check(!client.IsReady && Volatile.Read(ref last) is { Connected: false }, "Command-pipe loss must invalidate the whole session.");
        Check(!client.Send(new SetVolumeCmd { Percent = 97 }), "Commands during reconnect must not be stored for later.");
    }
    using (var second = new Fixture(name))
    {
        await second.Connect();
        await second.ReadHandshake();
        Check(!client.IsReady, "Reconnecting must wait for new authoritative state.");
        await second.State();
        await Until(() => client.IsReady);
        client.Send(new SetVolumeCmd { Percent = 55 });
        Check((await second.Read())!.Contains("\"percent\":55"), "Queued commands from the old session must not replay.");
        int before = Volatile.Read(ref disconnected);
        for (int i = 0; i < 64; i++) client.Send(new SetVolumeCmd { Percent = 40 });
        second.Events.Dispose();
        await Until(() => Volatile.Read(ref disconnected) > before);
        Check(!client.IsReady, "Events-pipe loss must also disable commands.");
    }
    using (var third = new Fixture(name))
    {
        await third.Connect();
        await third.ReadHandshake();
        await third.State();
        await Until(() => client.IsReady);
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
    await using var client = new DaemonClient(mismatch + "-events", mismatch + "-cmds", () => { });
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
    await using var client = new DaemonClient(fixtureName + "-events", fixtureName + "-cmds", () => Interlocked.Increment(ref respawns));
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
Console.WriteLine("IPC session checks passed: readiness, both pipe failures, no stale replay, shutdown delivery, process exit and bounded failure.");

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
