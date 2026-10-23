using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;

internal static class IslandBatteryChecks
{
    public static void Run(Action<bool, string> check)
    {
        const ulong seen = 1_792_000_000_000;
        DateTimeOffset At(ulong age) => DateTimeOffset.FromUnixTimeMilliseconds((long)(seen + age));
        Snapshot Case() => new()
        {
            DeviceId = "001122334455", BatterySource = "aap", BatteryLive = true,
            BatteryTimestampMs = seen + 80_000,
            Battery = new Battery { Case = 90, CaseCharging = true },
            BatteryFreshness = new BatteryFreshness
            {
                Case = new BatteryObservation { TimestampMs = seen, Live = false, Present = false },
            },
        };
        var state = Case();
        foreach (ulong age in new ulong[] { 0, 89_999 })
            check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(age)) &&
                IslandBatteryPresentation.CaseGraceExpiresAt(state, At(age)) == At(90_000),
                "A recently absent case stays current until its original timestamp plus 90 seconds.");
        check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(90_000)) &&
            IslandBatteryPresentation.CaseGraceExpiresAt(state, At(90_000)) is null,
            "The display grace expires exactly at 90,000 ms, independently of newer bud reports.");
        check(!BatteryPresentation.IsLive(state, BatteryPart.Case) && !BatteryPresentation.IsCharging(state, BatteryPart.Case),
            "Displaying a recent case never changes its transport freshness or charging state.");
        check(state.BatteryFreshness!.Case.TimestampMs == seen && !state.BatteryFreshness.Case.Live &&
            state.BatteryFreshness.Case.Present == false && state.Battery.CaseCharging,
            "Presentation must not rewrite the observation, presence or charging input.");

        state.Connected = true;
        check(IslandBatteryPresentation.FreshnessText(state, BatteryPart.Case, At(1)) == "",
            "The recent case does not show a stale label beside connected buds.");
        check(IslandBatteryPresentation.FreshnessText(state, BatteryPart.Case, At(90_000)) ==
            BatteryPresentation.FreshnessText(state, BatteryPart.Case),
            "An expired case uses its original per-case last-seen text.");
        state.Connected = false;
        check(IslandBatteryPresentation.FreshnessText(state, BatteryPart.Case, At(1)) == "Battery.LiveHelp" &&
            IslandBatteryPresentation.SharedFreshnessText(state, false, At(1)) == "",
            "A recent nearby case uses nearby help without a shared stale line.");
        check(IslandBatteryPresentation.SharedFreshnessText(state, false, At(90_000)) ==
            BatteryPresentation.SharedFreshnessText(state, false),
            "After grace, the island retains the existing shared-age rendering.");

        foreach (string source in new[] { "aap", "ble", "ble_case" })
        {
            state = Case(); state.BatterySource = source;
            check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
                "Both known battery sources can retain a recent case percentage.");
            state.BatteryLive = false;
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
                "Display grace cannot revive a source marked non-live.");
        }
        foreach (string source in new[] { "", "unknown" })
        {
            state = Case(); state.Connected = true; state.BatterySource = source;
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
                "Unknown and legacy source metadata cannot authorize absent-case grace.");
        }
        foreach (ulong? stamp in new ulong?[] { null, 0, seen + 2, ulong.MaxValue, 253_402_300_800_000 })
        {
            state = Case(); state.BatteryFreshness!.Case.TimestampMs = stamp;
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)) &&
                IslandBatteryPresentation.CaseGraceExpiresAt(state, At(1)) is null,
                "Missing, zero, future and out-of-range case times never gain display freshness.");
        }
        state = Case(); state.Battery.Case = 0;
        check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
            "A reported zero percent is a valid case reading.");
        foreach (byte? level in new byte?[] { null, 101, 255 })
        {
            state.Battery.Case = level;
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
                "Missing or invalid case percentages never gain display freshness.");
        }
        foreach (bool? present in new bool?[] { null, true })
        {
            state = Case(); state.BatteryFreshness!.Case.Present = present;
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
                "Grace covers explicit case absence, not an old or unknown presence report.");
        }
        state = Case(); state.BatteryFreshness!.Case.Live = true;
        check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)) &&
            !BatteryPresentation.IsCharging(state, BatteryPart.Case),
            "A contradictory live flag cannot turn an absent case into a charging case.");
        state.BatteryFreshness.Case.Present = true;
        check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)) &&
            BatteryPresentation.IsCharging(state, BatteryPart.Case) &&
            IslandBatteryPresentation.CaseGraceExpiresAt(state, At(1)) is null,
            "An actual live case keeps the existing charging behavior without a grace deadline.");

        state = Case(); state.DeviceId = "";
        check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
            "A snapshot without a selected device cannot retain an absent case as current.");
        state = new Snapshot { DeviceId = "aabbccddeeff", BatterySource = "aap", BatteryLive = true };
        check(!IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)),
            "A new pair cannot inherit a case value or observation from another snapshot.");
        state = Case(); state.BatteryFreshness = null;
        check(IslandBatteryPresentation.IsCurrentForDisplay(state, BatteryPart.Case, At(1)) ==
            BatteryPresentation.IsLive(state, BatteryPart.Case) &&
            IslandBatteryPresentation.CaseGraceExpiresAt(state, At(1)) is null,
            "Legacy snapshots keep their existing live behavior without adding grace.");
        foreach (var part in new[] { BatteryPart.Left, BatteryPart.Right, BatteryPart.Headphone })
        {
            state = Case();
            state.Battery.Left = state.Battery.Right = state.Battery.Headphone = 90;
            state.BatteryFreshness!.Left = state.BatteryFreshness.Right = state.BatteryFreshness.Headphone =
                new BatteryObservation { TimestampMs = seen, Live = false, Present = false };
            check(!IslandBatteryPresentation.IsCurrentForDisplay(state, part, At(1)) &&
                IslandBatteryPresentation.FreshnessText(state, part, At(1)) == BatteryPresentation.FreshnessText(state, part),
                "Bud and headphone readings remain strict even while the case has display grace.");
        }
        Console.WriteLine("Island case battery display grace checks passed.");
    }
}
