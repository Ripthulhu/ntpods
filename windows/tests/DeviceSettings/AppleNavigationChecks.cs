using System.Text.RegularExpressions;
using System.Xml.Linq;
using NTPods.WinUI.Controls;

internal static class AppleNavigationChecks
{
    public static void Run(Action<bool, string> check)
    {
        var repository = new DirectoryInfo(AppContext.BaseDirectory);
        while (repository is not null && !File.Exists(Path.Combine(repository.FullName, "windows", "device-models.json")))
            repository = repository.Parent;
        if (repository is null) throw new InvalidOperationException("Cannot locate the device UI sources.");
        string ui = Path.Combine(repository.FullName, "windows", "winui", "NTPods.WinUI");
        var page = XDocument.Load(Path.Combine(ui, "Pages", "DevicePage.xaml"));
        XNamespace x = "http://schemas.microsoft.com/winfx/2006/xaml";
        var named = page.Descendants().Where(e => e.Attribute(x + "Name") is not null).ToArray();
        check(named.Select(e => (string)e.Attribute(x + "Name")!).Distinct().Count() == named.Length,
            "Device navigation must keep one instance of each named control.");
        var source = File.ReadAllText(Path.Combine(ui, "Pages", "DevicePage.xaml.cs"));
        var routes = Regex.Matches(source, "\\[\"([^\"]+)\"\\] = ").Select(m => m.Groups[1].Value).ToHashSet();
        foreach (var button in page.Descendants().Where(e => e.Name.LocalName == "Button" && e.Attribute("Tag") is not null))
        {
            check((string)button.Attribute("Tag")! == "rename" || routes.Contains((string)button.Attribute("Tag")!),
                "Every device navigation row must have a destination.");
            check((string?)button.Attribute("Style") is "{StaticResource AppleNavigationButton}" or "{StaticResource AppleGroupedNavigationButton}",
                "Device navigation must use the filled Apple list appearance.");
        }
        var sections = named.Single(e => (string?)e.Attribute(x + "Name") == "DeviceSections");
        var offlineStop = named.Single(e => (string?)e.Attribute(x + "Name") == "OfflineSpatialStop");
        check(offlineStop.Ancestors().Any(e => (string?)e.Attribute(x + "Name") == "MainSections") &&
              !offlineStop.Ancestors().Any(e => (string?)e.Attribute(x + "Name") == "DeviceBody") &&
              (string?)offlineStop.Attribute("Click") == "OfflineSpatialStop_Click",
              "A waiting session can be stopped from the main page even when the dongle controls disappear.");
        check((string?)sections.Elements().First().Attribute(x + "Name") == "NameButton",
            "Name must be the first settings row after the compact battery hero.");
        foreach (string optional in new[] { "CompanionCard", "VolumeCard" })
            check(named.Single(e => (string?)e.Attribute(x + "Name") == optional).Ancestors()
                .Any(e => (string?)e.Attribute(x + "Name") == "ExtraSections"),
                "Windows-specific controls must follow the common Apple settings.");
        var dongleCard = named.Single(e => (string?)e.Attribute(x + "Name") == "DongleCard");
        check((string?)dongleCard.ElementsBeforeSelf().Last().Attribute(x + "Name") == "DeviceBody",
            "Dongle setup must be reachable before a pair is selected and follow the device settings.");
        foreach (string control in new[] { "HearingProtectionButton", "HearingAidButton", "LeftHoldButton", "RightHoldButton" })
            check(named.Single(e => (string?)e.Attribute(x + "Name") == control).Ancestors()
                .Any(e => e.Name.LocalName == "Border" && (string?)e.Attribute("Style") == "{StaticResource Card}"),
                "Hearing Health and Press and Hold rows must share filled inset groups.");
        check((string?)named.Single(e => (string?)e.Attribute(x + "Name") == "SubpageNavigation").Attribute("Height") == "64" &&
            (string?)named.Single(e => (string?)e.Attribute(x + "Name") == "SubpageBackButton").Attribute("Style") == "{StaticResource CircleButton}",
            "Subpages must retain the Apple scaffold height and round Back target.");
        foreach (var (control, destination) in new[]
        {
            ("AccommodationsButton", "AccessibilityPage"), ("AccommodationsCard", "AccommodationsPage"),
            ("TransparencyCard", "TransparencyPage"), ("MicCard", "RecorderPage"),
            ("DeviceSettingsCard", "PlaybackPage"), ("HearingAidCard", "HearingPage"),
            ("HardwareDiagnosticsCard", "HardwarePage"), ("HeartRateCard", "HeartRatePage"),
            ("HeadTrackingCard", "HeadTrackingPage"), ("NearbyFinderCard", "FinderPage"),
            ("NearbyCaseCard", "CasePage"), ("MultipointCard", "MultipointPage"), ("EqualizerCard", "EqualizerPage")
        })
        {
            var element = named.Single(e => (string?)e.Attribute(x + "Name") == control);
            check(element.Ancestors().Any(e => (string?)e.Attribute(x + "Name") == destination),
                "Existing controls must remain reachable in the intended Apple submenu.");
        }
        var settings = XDocument.Load(Path.Combine(ui, "Controls", "DeviceSettingsCard.xaml"));
        var protection = settings.Descendants().Single(e => (string?)e.Attribute(x + "Name") == "LoudSoundReductionRow");
        string protectionOwner = (string?)protection.Parent?.Attribute(x + "Name") ?? "";
        var settingsSource = File.ReadAllText(Path.Combine(ui, "Controls", "DeviceSettingsCard.xaml.cs"));
        check(protectionOwner.Length > 0 && settingsSource.Contains($"{protectionOwner}.Children.Remove(LoudSoundReductionRow)") &&
            !settingsSource.Contains("LoudSoundReductionRow.Parent"),
            "Hearing Protection must detach from its declared container before an Expander creates its visual parent.");
        var transparency = XDocument.Load(Path.Combine(ui, "Controls", "TransparencyCard.xaml"));
        check(transparency.Descendants().Count(e => e.Name.LocalName == "Slider") == 5,
            "Transparency must expose the five captured sliders without adding arbitrary curve editors.");
        check(!transparency.Descendants().Any(e => e.Name.LocalName == "Expander"),
            "Transparency is a dedicated screen and must not add a redundant expander.");
        var transparencyOptions = transparency.Descendants().Single(e => (string?)e.Attribute(x + "Name") == "Options");
        check(transparencyOptions.Name.LocalName == "StackPanel" &&
            !transparencyOptions.Descendants().Any(e => e.Name.LocalName == "ContentControl"),
            "Transparency inputs must remain direct layout children without a focusable content wrapper hiding automation descendants.");
        var transparencySource = File.ReadAllText(Path.Combine(ui, "Controls", "TransparencyCard.xaml.cs"));
        foreach (string input in new[] { "EnableSwitch", "Amplification", "Balance", "Tone", "Ambient", "OwnVoice", "BoostSwitch" })
        {
            var control = transparencyOptions.Descendants().Single(e => (string?)e.Attribute(x + "Name") == input);
            check(control.Attribute("AutomationProperties.Name") is not null && (string?)control.Attribute("IsTabStop") != "False" &&
                (string?)control.Attribute("AutomationProperties.AccessibilityView") != "Raw" &&
                transparencySource.Contains(input + ".IsEnabled"),
                "Every Transparency input must have an accessible name, native tab access and its own availability guard.");
        }
        var app = XDocument.Load(Path.Combine(ui, "App.xaml"));
        var ctStyle = app.Descendants().Single(e => (string?)e.Attribute(x + "Key") == "TransparencySlider");
        foreach (string track in new[] { "HorizontalTrackRect", "HorizontalDecreaseRect" })
        {
            var rectangle = ctStyle.Descendants().Single(e => (string?)e.Attribute(x + "Name") == track);
            check((string?)rectangle.Attribute("Height") == "6" && (string?)rectangle.Attribute("MaxHeight") == "6" &&
                (string?)rectangle.Attribute("VerticalAlignment") == "Center" &&
                (string?)rectangle.Attribute("RadiusX") == "3" && (string?)rectangle.Attribute("RadiusY") == "3",
                "Native slider layout must not enlarge the six-DIP CT track or flatten its rounded caps.");
        }
        var thumbStyle = app.Descendants().Single(e => (string?)e.Attribute(x + "Key") == "TransparencySliderThumbStyle");
        var thumbRoot = thumbStyle.Descendants().Single(e => e.Name.LocalName == "ControlTemplate").Elements().Single();
        check(thumbRoot.Name.LocalName == "Grid" && (string?)thumbRoot.Attribute("Background") == "Transparent" &&
            (string?)thumbRoot.Attribute("IsHitTestVisible") != "False",
            "The native Thumb template must retain a hit-testable root so drag capture and pressed glass can activate.");
        check(ctStyle.Descendants().Any(e => (string?)e.Attribute(x + "Name") == "HorizontalThumb" &&
            (string?)e.Attribute("Width") == "40" && (string?)e.Attribute("Height") == "24") &&
            transparencyOptions.Descendants().Where(e => e.Name.LocalName == "Slider").All(e =>
                (string?)e.Attribute("Style") == "{StaticResource TransparencySlider}" && e.Ancestors().Any(parent =>
                    parent.Name.LocalName == "Border" && (string?)parent.Attribute("Padding") == "24,16")),
            "The CT-specific native slider must retain a 40-by-24 thumb and padded groups.");
        check(transparencyOptions.Descendants().Where(e => e.Name.LocalName == "Slider").All(slider =>
            slider.Parent!.Elements().Single(e => e.Name.LocalName == "TextBlock").Attribute("Margin")?.Value == "0,0,0,8"),
            "Each CT caption must keep clear of the rounded corner with an eight-DIP gap before its track.");
        foreach (double width in new[] { 220d, 320, 576 })
        foreach (double fraction in new[] { 0d, .5, 1 })
        foreach (double progress in new[] { 0d, .5, 1 })
        {
            double inset = TransparencySliderMetrics.CardInset + TransparencySliderMetrics.SliderMargin;
            double travel = width - 2 * inset - TransparencySliderMetrics.Width;
            double center = inset + TransparencySliderMetrics.Width / 2 + fraction * travel;
            double half = TransparencySliderMetrics.Width * TransparencySliderMetrics.Scale(progress) / 2;
            check(center - half >= 24 && center + half <= width - 24,
                "The entire pressed glass thumb must stay inside the 24-DIP card inset at both endpoints.");
        }
        var accommodations = XDocument.Load(Path.Combine(ui, "Controls", "AccommodationsCard.xaml"));
        var optimization = accommodations.Descendants().Single(e => (string?)e.Attribute(x + "Name") == "OptimizationSlider");
        check((string?)optimization.Attribute("Minimum") == "0" && (string?)optimization.Attribute("Maximum") == "2" &&
            (string?)optimization.Attribute("StepFrequency") == "1", "Optimization must have exactly three fixed choices.");
        check((string?)optimization.Attribute("Style") == "{StaticResource TransparencySlider}" &&
            optimization.Ancestors().Any(e => e.Name.LocalName == "Border" && (string?)e.Attribute("Padding") == "24,16"),
            "Optimization uses the shared inset track and keeps the pressed thumb clear of card edges.");
        check(!accommodations.Descendants().Any(e => ((string?)e.Attribute(x + "Name")) is "CustomizeAudio" or "PlaySample"),
            "Unverified setup and sample actions must not appear as working controls.");
        check(accommodations.Descendants().Single(e => (string?)e.Attribute(x + "Name") == "TuningPicker")
            .Elements().Count(e => e.Name.LocalName == "RadioButton") == 4,
            "Audio tuning must expose Audiogram and the three stock choices together.");
        var tuningRows = accommodations.Descendants().Where(e => e.Name.LocalName == "RadioButton").ToArray();
        check(tuningRows.All(e => (string?)e.Attribute("Width") == "{Binding ActualWidth, ElementName=TuningPicker}") &&
            tuningRows.Take(3).All(e => (string?)e.Attribute("BorderThickness") == "0,0,0,1") &&
            (string?)tuningRows[3].Attribute("BorderThickness") == "0",
            "Tuning rows must span their group with dividers only between choices.");
        var choiceTemplate = app.Descendants().Single(e => (string?)e.Attribute(x + "Key") == "AppleChoiceRow");
        check(choiceTemplate.Descendants().Any(e => e.Name.LocalName == "ContentPresenter" &&
            (string?)e.Attribute("HorizontalContentAlignment") == "{TemplateBinding HorizontalContentAlignment}") &&
            choiceTemplate.Descendants().Any(e => e.Name.LocalName == "Border" &&
                (string?)e.Attribute("Grid.ColumnSpan") == "2" &&
                (string?)e.Attribute("BorderThickness") == "{TemplateBinding BorderThickness}"),
            "Choice dividers must span the label and checkmark columns instead of sizing to the label.");
        var modes = XDocument.Load(Path.Combine(ui, "Controls", "NoiseControlCard.xaml"));
        check(new[] { "OffIcon", "TransparencyIcon", "AdaptiveIcon", "AncIcon" }.All(name =>
            (string?)modes.Descendants().Single(e => (string?)e.Attribute(x + "Name") == name).Attribute("Foreground") ==
                "{ThemeResource AppleSecondaryBrush}"),
            "Unselected listening icons must use an explicit contrasting foreground in both themes.");
        foreach (string theme in new[] { "Light", "Dark" })
        {
            var palette = app.Descendants().Single(e => (string?)e.Attribute(x + "Key") == theme);
            double foreground = Luminance((string)palette.Elements().Single(e => (string?)e.Attribute(x + "Key") == "AppleSecondaryBrush").Attribute("Color")!);
            double background = Luminance((string)palette.Elements().Single(e => (string?)e.Attribute(x + "Key") == "AppleTrackBrush").Attribute("Color")!);
            check((Math.Max(foreground, background) + .05) / (Math.Min(foreground, background) + .05) >= 3,
                "Unselected listening icons must retain at least 3:1 contrast against their rail.");
        }
        var iconSource = File.ReadAllText(Path.Combine(ui, "Controls", "ListeningModeIcon.cs"));
        check(iconSource.Contains("ActualThemeChanged += (_, _) => { _rendered = null; ObserveForeground(); }") &&
            !iconSource.Contains("ActualThemeChanged += (_, _) => LoadSource()"),
            "Changing theme must redraw cached listening artwork without reloading it during a gesture.");
        var battery = XDocument.Load(Path.Combine(ui, "Controls", "BatteryCard.xaml"));
        check((string?)battery.Descendants().Single(e => (string?)e.Attribute(x + "Name") == "BatteryLayout").Attribute("MaxWidth") == "350" &&
            new[] { "LeftArtwork", "RightArtwork", "LeftCell", "RightCell", "CaseCell" }.All(name =>
                battery.Descendants().Any(e => (string?)e.Attribute(x + "Name") == name)),
            "The compact two-column hero must retain independent artwork and battery readings.");
        Console.WriteLine("Apple device navigation reachability checks passed.");
    }

    private static double Luminance(string color)
    {
        int rgb = Convert.ToInt32(color[^6..], 16);
        double Channel(int shift)
        {
            double value = ((rgb >> shift) & 255) / 255d;
            return value <= .04045 ? value / 12.92 : Math.Pow((value + .055) / 1.055, 2.4);
        }
        return .2126 * Channel(16) + .7152 * Channel(8) + .0722 * Channel(0);
    }
}
