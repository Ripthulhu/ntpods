using NTPods.WinUI.Popup;

internal static class PairingArtworkChecks
{
    internal static void Run(Action<bool, string> check)
    {
        foreach (string model in new[] { "A2084", "A2698", "A3047", "A3063" })
            check(PairingArtworkPolicy.SupportsLoop(model, false) && PairingArtworkPolicy.ShouldPlay(model, false, true, true, false, false),
                "Known Pro generations may animate only while the popup is presented.");
        foreach (string? model in new string?[] { null, "unknown", "AirPods Pro 3", "A2031", "A2096" })
            check(!PairingArtworkPolicy.SupportsLoop(model, false) && !PairingArtworkPolicy.ShouldPlay(model, false, true, true, false, false),
                "Unknown models, names, ordinary buds and headphones retain static artwork.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", true, true, true, false, false), "A headphone override cannot inherit the bud loop.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, false, true, false, false), "A loaded but hidden popup must not start a decoder.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, true, false, false, false), "Reduced motion keeps artwork static.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, true, true, true, false), "High contrast retains the accessible static artwork.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, true, true, false, true), "A failed decoder must stay on its fallback.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, false, false, false, false) &&
            !PairingArtworkPolicy.ShouldPlay("A3063", false, true, false, false, false),
            "A hidden animation preference change must remain effective when the card reopens.");
        check(!PairingArtworkPolicy.ShouldPlay("A3063", false, false, true, false, false) &&
            PairingArtworkPolicy.ShouldPlay("A3063", false, true, true, false, false),
            "An interrupted dismissal can resume its eligible artwork after the popup is shown again.");
        Console.WriteLine("Pairing artwork lifecycle checks passed.");
    }
}
