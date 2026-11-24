using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

internal static class CompanionPairingChecks
{
    public static void Run()
    {
        static void Check(bool value, string message) { if (!value) throw new InvalidOperationException(message); }
        var begin = JsonNode.Parse(Wire.ToLine(new CompanionBeginPairingCmd { PeerAddress = "11:22:33:44:55:66",
            DeviceId = "aabbccddeeff", Code = "01234567", RequestId = ulong.MaxValue }))!;
        Check(begin["cmd"]!.GetValue<string>() == "companion_begin_pairing" && begin["code"]!.GetValue<string>() == "01234567" &&
            begin["request_id"]!.GetValue<ulong>() == ulong.MaxValue, "Setup IPC preserves leading zeroes and full-width correlation.");
        var value = JsonNode.Parse("""{"event":"companion_status","status":{"linked":false,"pairing":{"request_id":18446744073709551615,"device_id":"aabbccddeeff","peer_address":"11:22:33:44:55:66","phase":"waiting","remaining_ms":120000}}}""")!;
        Check(Wire.ParseEvent(value.ToJsonString()) is DaemonEvent.Companion { Status.Pairing.RequestId: ulong.MaxValue },
            "A pairing window roundtrips independently of any existing approval.");
        foreach (var (key, replacement) in new (string, JsonNode?)[] {
            ("request_id", JsonValue.Create(0)), ("device_id", null), ("peer_address", null),
            ("device_id", JsonValue.Create("ffffffffffff")), ("peer_address", JsonValue.Create("00:00:00:00:00:00")),
            ("phase", JsonValue.Create("authenticated")), ("remaining_ms", JsonValue.Create(120001)),
            ("code", JsonValue.Create("01234567")), ("key", JsonValue.Create(new string('a',64))),
        }) {
            var invalid = value.DeepClone(); invalid["status"]!["pairing"]![key] = replacement;
            Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Malformed or secret-bearing setup status must be rejected.");
        }
        var missing = value.DeepClone(); missing["status"]!["pairing"]!.AsObject().Remove("remaining_ms");
        Check(Wire.ParseEvent(missing.ToJsonString()) is null, "An incomplete pairing window cannot invent a lifetime.");
        Console.WriteLine("Short-code IPC bounds and secret-free status checks passed.");
    }
}
