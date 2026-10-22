using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

internal static class CompanionPreferenceChecks
{
    public static void Run(string input, string output)
    {
        static void Check(bool value, string message) { if (!value) throw new InvalidOperationException(message); }
        var lines = File.ReadAllLines(input);
        Check(lines.Length == 2, "Expected configured and cleared preference states.");
        var result = new List<string>();
        for (int i = 0; i < lines.Length; ++i)
        {
            Check(Wire.ParseEvent(lines[i]) is DaemonEvent.Preferences, "Preference event must dispatch to a bounded typed state.");
            var state = ((DaemonEvent.Preferences)Wire.ParseEvent(lines[i])!).Value;
            Check(state.Records.Length == (i == 0 ? 15 : 0) && !state.LocalCallConsent && state.PeerPrefs == (i == 0) && state.PeerCalls == (i == 0),
                "Synced call policy must remain separate from local consent and peer capabilities.");
            Check(!state.LocalCallTakeoverEnabled, "A received true policy cannot authorize local automatic call takeover.");
            if (i == 0)
            {
                Check(state.Matches("00:11:22:33:44:55") && !state.Matches("001122334466"), "Per-pair preferences cannot project onto another selection.");
                Check(state.Conversation is { Mode: "relative", First: "43.25", Second: "19.8", PauseMedia: true } &&
                    state.Records.All(record => record.Rev == "ffffffffffffffff" && record.Origin == "android"), "Percent strings and full-width logical stamps must survive without rounding.");
            }
            result.Add(JsonSerializer.Serialize(new { @event = "companion_preferences", preferences = state }, Wire.Json));
        }
        var valid = JsonNode.Parse(lines[0])!.AsObject();
        foreach (string invalid in new[] { "-1", "+1", "01", "1.", "1.0", ".5", "1e1", "100.1", "0.0000000001", "NaN" })
        {
            var copy = valid.DeepClone();
            copy["preferences"]!["records"]![2]!["value"]!["first"] = invalid;
            Check(Wire.ParseEvent(copy.ToJsonString()) is null, "Malformed percentages must never reach the UI.");
        }
        foreach (string invalid in new[] { "", "0000000000000000", "FFFFFFFFFFFFFFFF", "10000000000000000" })
        {
            var copy = valid.DeepClone();
            copy["preferences"]!["records"]![0]!["rev"] = invalid;
            Check(Wire.ParseEvent(copy.ToJsonString()) is null, "Malformed logical revisions must fail closed.");
        }
        foreach (string key in new[] { "permission", "raw_aacp", "secret_hex" })
        {
            var copy = valid.DeepClone();
            copy["preferences"]!["records"]![0]!["key"] = key;
            Check(Wire.ParseEvent(copy.ToJsonString()) is null, "No arbitrary keys or grants may enter preference state.");
        }
        var duplicate = valid.DeepClone();
        duplicate["preferences"]!["records"]!.AsArray().Add(duplicate["preferences"]!["records"]![0]!.DeepClone());
        Check(Wire.ParseEvent(duplicate.ToJsonString()) is null, "Duplicate or excessive preference records must be rejected.");
        var collision = valid.DeepClone();
        collision["preferences"]!["records"]![2]!["value"]!["second"] = "99";
        Check(Wire.ParseEvent(collision.ToJsonString()) is null, "The second stage cannot be louder than the first.");
        var extra = valid.DeepClone();
        extra["preferences"]!["records"]![2]!["value"]!["raw"] = 1;
        Check(Wire.ParseEvent(extra.ToJsonString()) is null, "Policy objects must have only their fixed typed fields.");
        result.Add(Wire.ToLine(new CompanionSetPreferenceCmd { Key = "theme", Value = JsonSerializer.SerializeToElement("light"), RequestId = ulong.MaxValue }).TrimEnd('\n'));
        result.Add(Wire.ToLine(new CompanionSetPreferenceCmd { Key = "conversation_policy", Value = JsonSerializer.SerializeToElement(new ConversationPolicy
            { Mode = "abs_cap", First = "33", Second = "19.8" }), DeviceId = "001122334455", RequestId = 9_007_199_254_740_993UL }).TrimEnd('\n'));
        result.Add(Wire.ToLine(new CompanionCallConsentCmd { Enabled = true, DeviceId = "001122334455", RequestId = ulong.MaxValue }).TrimEnd('\n'));
        File.WriteAllLines(output, result);
        Console.WriteLine("C#: typed shared preferences, consent separation, exact revisions/percentages and malformed input checks passed");
    }
}
