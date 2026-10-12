using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class BatteryFreshnessChecks
{
    public static void Run()
    {
        CheckArtworkAvailability();
        var state = new Snapshot
        {
            Connected = true, BatterySource = "aap", BatteryLive = true,
            BatteryTimestampMs = 1_792_000_100_000,
            Battery = new Battery { Left = 70, Right = 72, Case = 90, CaseCharging = true },
            BatteryFreshness = new BatteryFreshness
            {
                Left = new BatteryObservation { TimestampMs = 1_792_000_100_000, Live = true },
                Right = new BatteryObservation { TimestampMs = 1_792_000_100_000, Live = true },
                Case = new BatteryObservation { TimestampMs = 1_792_000_000_000, Live = false },
            },
        };
        Check(BatteryPresentation.IsLive(state, BatteryPart.Left) && !BatteryPresentation.IsLive(state, BatteryPart.Case),
            "Fresh buds must not make an old case report live.");
        Check(!BatteryPresentation.IsCharging(state) && !BatteryPresentation.IsCharging(state, BatteryPart.Case),
            "The old case charging flag must not appear in any battery surface.");
        string expectedDate = DateTimeOffset.FromUnixTimeMilliseconds(1_792_000_000_000).ToLocalTime().ToString("g");
        Check(BatteryPresentation.FreshnessText(state, BatteryPart.Case) == "Battery.LastSeen " + expectedDate,
            "Case help must use the case timestamp, not a newer bud report.");
        Check(BatteryPresentation.ReadingText(state, BatteryPart.Case, "Case").Contains("Battery.LastKnown"),
            "A mixed tray summary must identify its cached case value.");
        Check(BatteryPresentation.LiveEarbudReading(state) == 70, "Only live earbuds contribute to the tray badge.");
        state.BatteryFreshness.Left.Live = state.BatteryFreshness.Right.Live = false;
        state.BatteryFreshness.Case.Live = true;
        Check(BatteryPresentation.LiveEarbudReading(state) is null && BatteryPresentation.IsCharging(state),
            "A fresh case must not rejuvenate cached earbuds.");
        var summaryPart = BatteryPresentation.EarbudSummaryPart(state);
        Check(summaryPart == BatteryPart.Left && BatteryPresentation.FreshnessText(state, summaryPart.Value)
                == "Battery.LastSeen " + DateTimeOffset.FromUnixTimeMilliseconds(1_792_000_100_000).ToLocalTime().ToString("g"),
            "The navigation summary must explain its cached earbud reading even when the case is fresh.");
        state.BatteryFreshness.Right.Live = true;
        Check(BatteryPresentation.EarbudSummaryPart(state) == BatteryPart.Right,
            "Navigation must prefer a fresh earbud over a lower cached reading.");
        state.Battery.Case = null;
        Check(!BatteryPresentation.IsCharging(state) && BatteryPresentation.ReadingText(state, BatteryPart.Case, "Case") == "",
            "Absent battery readings must not show percentages or charging.");
        state.BatteryFreshness = null;
        Check(BatteryPresentation.IsLive(state, BatteryPart.Left), "Legacy snapshots remain compatible.");
        Console.WriteLine("Per-component battery freshness checks passed.");
    }

    private static void CheckArtworkAvailability()
    {
        var state = new Snapshot
        {
            Connected = true, BatterySource = "aap", BatteryLive = false,
            Battery = new Battery { Left = 70, Right = 72, Headphone = 85 },
            BatteryFreshness = new BatteryFreshness
            {
                Left = new BatteryObservation { TimestampMs = 1_792_000_000_000, Live = false },
                Right = new BatteryObservation { TimestampMs = 1_792_000_000_000, Live = false },
                Headphone = new BatteryObservation { TimestampMs = 1_792_000_000_000, Live = false },
            },
        };
        foreach (var part in new[] { BatteryPart.Left, BatteryPart.Right, BatteryPart.Headphone })
        {
            Check(BatteryPresentation.ArtworkIsAvailable(state, part) && !BatteryPresentation.IsLive(state, part),
                "Connected device artwork stays bright when its unchanged battery reading ages.");
        }
        Check(!BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Case),
            "Connected earbuds must not brighten a case with no reading.");
        state.BatteryFreshness.Left.Present = true;
        state.BatteryFreshness.Right.Present = false;
        Check(BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Left)
                && !BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Right)
                && BatteryPresentation.Reading(state, BatteryPart.Right) == 72,
            "An explicitly unavailable bud dims even while the other is connected, retaining its cached percentage.");
        state.BatteryLive = true;
        state.BatteryFreshness.Left.Live = state.BatteryFreshness.Right.Live = true;
        Check(BatteryPresentation.IsLive(state, BatteryPart.Left)
                && !BatteryPresentation.IsLive(state, BatteryPart.Right)
                && !BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Right),
            "Explicit absence takes precedence over a contradictory live flag.");
        state.BatteryFreshness.Right.Present = true;
        Check(BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Right),
            "A newly reported bud becomes bright again.");
        state.BatteryLive = false;
        state.BatteryFreshness.Left.Live = state.BatteryFreshness.Right.Live = false;
        Check(BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Left)
                && BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Right),
            "Aging both reports must not dim connected buds reported present.");
        state.Connected = false;
        Check(Enum.GetValues<BatteryPart>().All(part => !BatteryPresentation.ArtworkIsAvailable(state, part)),
            "Disconnected cached artwork must remain subdued.");
        state.BatterySource = "ble";
        state.BatteryLive = true;
        state.BatteryFreshness.Left.Live = true;
        state.Battery.LeftCharging = true;
        Check(BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Left)
                && BatteryPresentation.IsCharging(state, BatteryPart.Left)
                && !BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Right)
                && !BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Case),
            "Nearby charging reports brighten only the components they refresh.");
        state.Battery.Case = 90;
        state.BatteryFreshness.Case = new BatteryObservation { TimestampMs = 1_792_000_100_000, Live = true };
        Check(BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Case),
            "A fresh nearby case report must brighten the case artwork.");
        state.BatteryFreshness.Case.Live = false;
        Check(!BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Case),
            "A retained case percentage must not claim the case is still available.");
        state.Battery.Case = null;
        state.BatteryFreshness.Case.Live = true;
        Check(!BatteryPresentation.ArtworkIsAvailable(state, BatteryPart.Case),
            "A freshness flag without a case reading must not brighten the case.");
    }

    private static void Check(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
}
