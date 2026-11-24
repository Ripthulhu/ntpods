using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

internal static class AccommodationsChecks
{
    public static void Run(string input, string output)
    {
        static void Check(bool value, string message) { if (!value) throw new InvalidOperationException(message); }
        var lines = File.ReadAllLines(input);
        Check(lines.Length == 7, "Expected seven accommodations fixture states.");
        var emitted = new List<string>();
        for (int i = 0; i < lines.Length; i++)
        {
            Check(Wire.ParseEvent(lines[i]) is DaemonEvent.State, "Each accommodations state must parse with the production parser.");
            var state = ((DaemonEvent.State)Wire.ParseEvent(lines[i])!).Snapshot;
            using var raw = JsonDocument.Parse(lines[i]);
            var expected = raw.RootElement.TryGetProperty("headphone_accommodations", out var value)
                ? value.Deserialize<HeadphoneAccommodationsState>() : null;
            Check(state.HeadphoneAccommodations == expected, "Accommodations state must survive cross-language parsing.");
            if (i == 0) Check(expected is null, "Older daemons must leave accommodations unavailable.");
            if (i == 1) Check(expected is { Phone: null, Media: null, Preset: null, Editable: false }, "Unknown curves and mode flags must stay unknown.");
            if (i == 6) Check(expected is { Phone: true, Media: false, EditVersion: ulong.MaxValue, Result: "sent", AudiogramAvailable: true,
                Requested: { Phone: false, Media: true, Preset: null, Audiogram: true } }, "A sent choice must stay separate from received values and preserve its full version.");
            var roundtrip = JsonSerializer.SerializeToNode(state, Wire.Json)!.AsObject();
            roundtrip["event"] = "state";
            emitted.Add(roundtrip.ToJsonString());
        }
        var valid = JsonNode.Parse(lines[2])!;
        var older = valid.DeepClone();
        older["headphone_accommodations"]!.AsObject().Remove("edit_version");
        older["headphone_accommodations"]!.AsObject().Remove("requested");
        older["headphone_accommodations"]!.AsObject().Remove("audiogram_available");
        Check(Wire.ParseEvent(older.ToJsonString()) is DaemonEvent.State { Snapshot.HeadphoneAccommodations.AudiogramAvailable: false },
            "Older reported-only states must remain compatible without claiming Audiogram support.");
        var invalidSent = JsonNode.Parse(lines[6])!;
        invalidSent["headphone_accommodations"]!["edit_version"] = 0;
        Check(Wire.ParseEvent(invalidSent.ToJsonString()) is null, "An unversioned sent choice cannot authorize editing.");
        var conflictingSource = valid.DeepClone();
        conflictingSource["headphone_accommodations"]!["audiogram"] = true;
        Check(Wire.ParseEvent(conflictingSource.ToJsonString()) is null, "A stock preset cannot also be classified as an Audiogram.");
        foreach (string field in new[] { "session", "revision", "phone", "media", "preset", "editable", "pending", "result", "error" })
        {
            var missing = valid.DeepClone();
            missing["headphone_accommodations"]!.AsObject().Remove(field);
            Check(Wire.ParseEvent(missing.ToJsonString()) is null, "Incomplete accommodations records must be rejected.");
        }
        foreach (string tuning in new[] { "audiogram", "custom", "unknown" })
        {
            var bad = valid.DeepClone();
            bad["headphone_accommodations"]!["preset"]!["tuning"] = tuning;
            Check(Wire.ParseEvent(bad.ToJsonString()) is null, "Unverified preset classifications must be rejected.");
        }
        emitted.Add(Wire.ToLine(new GetHeadphoneAccommodationsCmd { DeviceId = "001122334455" }).TrimEnd('\n'));
        emitted.Add(Wire.ToLine(new SetHeadphoneAccommodationsCmd { RequestId = ulong.MaxValue, DeviceId = "001122334455",
            Session = 17, Revision = 4, EditVersion = ulong.MaxValue, Audiogram = true, Phone = false }).TrimEnd('\n'));
        emitted.Add(Wire.ToLine(new SetHeadphoneAccommodationsCmd { RequestId = 81, DeviceId = "001122334455",
            Session = 17, Revision = 4, Preset = new() { Tuning = "brightness", Optimization = "strong" } }).TrimEnd('\n'));
        File.WriteAllLines(output, emitted);
        Console.WriteLine("Headphone Accommodations production parser and command checks passed.");
    }
}
