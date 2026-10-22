using System.Text;
using System.Text.Json;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

// Opt-in receipt metadata only: never serialize a command, status or raw pipe payload here.
internal static class CompanionUiTrace
{
    private const int Limit = 64 * 1024;
    private static readonly bool Enabled = Array.IndexOf(Environment.GetCommandLineArgs(), "--companion-trace") >= 0;
    private static readonly object Gate = new();
    private static readonly Queue<string> Lines = new();
    private static int _bytes;

    public static void Write(string stage, object fields)
    {
        if (!Enabled) return;
        try
        {
            string line = JsonSerializer.Serialize(new { time = DateTimeOffset.UtcNow, stage, fields }) + "\n";
            int bytes = Encoding.UTF8.GetByteCount(line);
            if (bytes > Limit) return;
            lock (Gate)
            {
                Lines.Enqueue(line);
                _bytes += bytes;
                while (_bytes > Limit) _bytes -= Encoding.UTF8.GetByteCount(Lines.Dequeue());
                string folder = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "NTPods");
                Directory.CreateDirectory(folder);
                File.WriteAllText(Path.Combine(folder, "companion-ui.log"), string.Concat(Lines), new UTF8Encoding(false));
            }
        }
        catch (Exception) { }
    }

    public static string Phase(string? phase) => phase is "idle" or "waiting" or "prepare" or
        "release_android" or "release_pc" or "acquire_android" or "acquire_pc" ? phase : "unknown";

    public static void Wire(string line, DaemonEvent? parsed, ulong epoch)
    {
        if (!Enabled) return;
        try
        {
            using var document = JsonDocument.Parse(line);
            var root = document.RootElement;
            if (root.TryGetProperty("event", out var kind) && kind.GetString() == "command_result")
            {
                string command = root.TryGetProperty("command", out var cmd) && cmd.GetString() == "companion_switch"
                    ? "companion_switch" : "other";
                ulong? requestId = root.TryGetProperty("request_id", out var id) && id.TryGetUInt64(out ulong value) ? value : null;
                Write("wire_result", new { epoch, command, request_id = requestId, typed = parsed is DaemonEvent.CommandResult });
            }
            else if (parsed is null) Write("wire_unparsed", new { epoch });
        }
        catch (Exception) { Write("wire_unparsed", new { epoch }); }
    }
}
