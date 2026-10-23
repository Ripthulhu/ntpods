using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class BatteryFreshnessChecks
{
    public static void Run()
    {
        CheckArtworkAvailability();
        CheckSharedAge();
        CheckCachePolicy();
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

    private static void CheckCachePolicy()
    {
        var state = new Snapshot
        {
            Model = "AirPods Pro 3", Connected = true, BatterySource = "aap", BatteryLive = true,
            Battery = new Battery { Left = 70, Right = 72, Case = 90 },
            BatteryFreshness = new BatteryFreshness
            {
                Left = new BatteryObservation { TimestampMs = 1_792_000_100_000, Present = true, Live = true },
                Right = new BatteryObservation { TimestampMs = 1_792_000_000_000, Present = false },
                Case = new BatteryObservation { TimestampMs = 1_792_000_000_000 },
            },
        };
        Check(BatteryPresentation.Reading(state, BatteryPart.Left, false) == 70 &&
            BatteryPresentation.Reading(state, BatteryPart.Right, false) is null &&
            BatteryPresentation.Reading(state, BatteryPart.Case, false) == 90,
            "Hiding disconnected components must preserve live readings and a case omitted from a fresh packet.");
        Check(BatteryPresentation.Reading(state, BatteryPart.Right, true) == 72 && state.BatteryFreshness.Right.TimestampMs == 1_792_000_000_000,
            "Re-enabling retained readings must restore the original value and age without changing observations.");
        state.Connected = false;
        state.BatteryLive = false;
        Check(BatteryPresentation.Reading(state, BatteryPart.Left, false) is null && state.Battery.Left == 70 && state.Model == "AirPods Pro 3",
            "Hiding cached values on disconnect cannot delete battery samples or model metadata.");
        state.BatterySource = "ble";
        state.BatteryLive = true;
        Check(BatteryPresentation.Reading(state, BatteryPart.Left, false) == 70,
            "Live nearby BLE readings remain visible without a control connection.");
        state.BatterySource = "ble_case";
        Check(BatteryPresentation.IsLive(state, BatteryPart.Left) && !state.Connected &&
            BatteryPresentation.Reading(state, BatteryPart.Right, false) is null,
            "Case-layout readings display passive battery data without reviving an absent bud or connecting audio.");
        state.BatteryLive = false;
        Check(!BatteryPresentation.IsLive(state) && !BatteryPresentation.IsCharging(state),
            "Expired case-layout observations cannot show charging.");
    }

    private static void CheckSharedAge()
    {
        var state = new Snapshot
        {
            BatterySource = "ble", BatteryLive = true,
            BatteryTimestampMs = 1_792_000_100_000,
            Battery = new Battery { Left = 70, Right = 72, Case = 90 },
            BatteryFreshness = new BatteryFreshness
            {
                Left = new BatteryObservation { TimestampMs = 1_792_000_000_000 },
                Right = new BatteryObservation { TimestampMs = 1_792_000_000_000 },
                Case = new BatteryObservation { TimestampMs = 1_792_000_000_000 },
            },
        };
        Check(BatteryPresentation.SharedFreshnessText(state, false) == BatteryPresentation.FreshnessText(state, BatteryPart.Case),
            "A shared age must come from the part readings, not a newer overall timestamp.");
        state.BatteryFreshness.Left.Live = true;
        Check(BatteryPresentation.SharedFreshnessText(state, false) == "",
            "One live part must leave stale ages on the individual cells.");
        state.BatteryFreshness.Left.Live = false;
        state.BatteryFreshness.Case.TimestampMs = 1_792_000_100_000;
        Check(BatteryPresentation.SharedFreshnessText(state, false) == "",
            "Different cached ages must not be combined into a newer shared age.");
        state.Battery.Case = null;
        Check(BatteryPresentation.SharedFreshnessText(state, false).Length > 0,
            "A missing part must not erase the shared age of known readings.");
        Check(BatteryPresentation.SharedFreshnessText(new Snapshot(), false) == "",
            "Missing readings must not produce a last-seen line.");
        state.Battery.Headphone = 80;
        state.BatteryFreshness.Headphone.TimestampMs = 1_792_000_000_000;
        Check(BatteryPresentation.SharedFreshnessText(state, true) == BatteryPresentation.FreshnessText(state, BatteryPart.Headphone),
            "Headphone age must not depend on earbud readings.");
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
