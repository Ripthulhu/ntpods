using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

internal static class DongleChecks
{
    public static void Run(string input, string output)
    {
        var result = new List<string>();
        var lines = File.ReadAllLines(input);
        if (lines.Length != 2) throw new InvalidOperationException("Expected connected and removed dongle states.");
        foreach (string line in lines)
        {
            var state = (Wire.ParseEvent(line) as DaemonEvent.State)?.Snapshot
                ?? throw new InvalidOperationException("USB state did not parse.");
            if (!JsonNode.DeepEquals(JsonNode.Parse(line)!["dongle"], JsonSerializer.SerializeToNode(state.Dongle, Wire.Json)))
                throw new InvalidOperationException("USB fields changed across IPC.");
            var roundtrip = JsonSerializer.SerializeToNode(state, Wire.Json)!.AsObject();
            roundtrip["event"] = "state";
            result.Add(roundtrip.ToJsonString());
        }
        var legacy = JsonNode.Parse(lines[1])!.AsObject(); legacy.Remove("dongle");
        if ((Wire.ParseEvent(legacy.ToJsonString()) as DaemonEvent.State)?.Snapshot.Dongle is not { Present: false, Session: 0 })
            throw new InvalidOperationException("Older daemons must not invent a dongle.");
        result.Add(Wire.ToLine(new DongleControlCmd { Session = ulong.MaxValue, Generation = uint.MaxValue, Opcode = 3, Argument = 2 }).TrimEnd('\n'));
        object[] healthCommands = [
            new SetHearingAidModeCmd { RequestId = ulong.MaxValue, On = false },
            new GetHearingMetadataCmd { DeviceId = $"dongle:{ulong.MaxValue}:{uint.MaxValue}" },
            new StartHardwareCheckCmd { RequestId = ulong.MaxValue, DeviceId = $"dongle:{ulong.MaxValue}:{uint.MaxValue}", BudsInCaseConfirmed = true },
            new SetDongleAudioProfileCmd { Codec = 3, Variable = true, Bitrate = 174000 },
            new GetDonglePerformanceCmd(),
        ];
        foreach (object command in healthCommands)
            result.Add(Wire.ToLine(new DongleHealthCmd { Session = ulong.MaxValue, Generation = uint.MaxValue, Command = command }).TrimEnd('\n'));
        foreach (string field in new[] { "health", "hearing_aid_settings", "hardware_diagnostics", "hearing_metadata" })
        {
            var broken = JsonNode.Parse(lines[0])!.AsObject();
            if (field == "health") broken["dongle"]!["health"] = null;
            else broken["dongle"]!["health"]![field] = new JsonObject();
            if (Wire.ParseEvent(broken.ToJsonString()) is not null)
                throw new InvalidOperationException("Malformed USB health data must not become editable settings.");
        }
        foreach (string field in new[] { "audio", "performance", "device_controls", "nearby_finder" })
        {
            var broken = JsonNode.Parse(lines[0])!.AsObject();
            if (field is "device_controls" or "nearby_finder") broken["dongle"]!["health"]![field] = null;
            else broken["dongle"]!["health"]![field]![field == "audio" ? "priming_ms" : "input"] = 255;
            if (Wire.ParseEvent(broken.ToJsonString()) is not null)
                throw new InvalidOperationException("Malformed dongle feature data must not authorize controls.");
        }
        File.WriteAllLines(output, result);
    }
}
