using System.Globalization;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal sealed class CaseInformationPresentation
{
    private string _deviceId = "";
    private ushort? _product;
    private bool _available;
    private CaseInformationError _error;
    public int Generation { get; private set; }
    public bool Busy { get; private set; }
    public CaseMetadata? Metadata { get; private set; }
    public bool CleanupConfirmed { get; private set; } = true;
    public CaseInformationError Error => CleanupConfirmed ? _error : CaseInformationError.RestoreUncertain;
    public ushort? Product => _product;
    public bool CanRead => _available && !Busy;

    public static ushort? CasePnpProduct(string? model) =>
        DeviceModels.Find(model).Family == "airpods_pro_3" ? (ushort)0x2029 : null;

    public static ushort? AdvertisedCasePnpProduct(ushort? model) => model == 0x2720 &&
        CasePnpProduct(PairingInvitationPolicy.NearbyCatalogModel(model.Value)) is { } product ? product : null;

    public bool Update(Snapshot snapshot, bool daemonReady)
    {
        ushort? product = CasePnpProduct(snapshot.Model);
        bool available = daemonReady && !string.IsNullOrEmpty(snapshot.DeviceId) && product.HasValue &&
            !snapshot.MicCaptureActive && !snapshot.MicUplinkActive && !snapshot.MicRecording && !snapshot.MicManualActive &&
            snapshot.MicState is "idle" or "unavailable";
        bool invalidate = !string.Equals(_deviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase) ||
            _product != product || !available && _available;
        _deviceId = snapshot.DeviceId;
        _product = product;
        _available = available;
        if (invalidate) Cancel();
        return invalidate;
    }

    public int Begin()
    {
        if (!CanRead) throw new InvalidOperationException();
        Cancel();
        Busy = true;
        return Generation;
    }

    public bool Complete(int generation, CaseInformationResult result)
    {
        // Cleanup belongs to the connection, even when its display result is stale.
        if (result.Error == CaseInformationError.RestoreUncertain) CleanupConfirmed = false;
        if (generation != Generation || !Busy || !_available) return false;
        Busy = false;
        Metadata = result.Success ? result.Metadata : null;
        _error = result.Error == CaseInformationError.None && result.Metadata is null
            ? CaseInformationError.InvalidResponse : result.Error;
        return true;
    }

    public void Cancel()
    {
        ++Generation;
        Busy = false;
        Metadata = null;
        _error = CaseInformationError.None;
    }

    public static string ProtocolVersion(uint value) =>
        FormattableString.Invariant($"{value >> 16}.{value >> 8 & 0xff}.{value & 0xff}");

    public static string Raw(uint value) => "0x" + value.ToString("X8", CultureInfo.InvariantCulture);

    public static IEnumerable<string> CapabilityKeys(uint value)
    {
        if ((value & 1) != 0) yield return "NearbyCase.Sound";
        if ((value & 2) != 0) yield return "NearbyCase.Motion";
        if ((value & 4) != 0) yield return "NearbyCase.Nfc";
        if ((value & 8) != 0) yield return "NearbyCase.BluetoothIdentity";
    }

    public static uint UnknownCapabilities(uint value) => value & ~0x0fu;

    public static string? ErrorKey(CaseInformationError error) => error switch
    {
        CaseInformationError.None or CaseInformationError.Cancelled => null,
        CaseInformationError.NotFound => "NearbyCase.NotFound",
        CaseInformationError.Ambiguous => "NearbyCase.Ambiguous",
        CaseInformationError.Paired => "NearbyCase.Paired",
        CaseInformationError.Unsupported => "NearbyCase.Unsupported",
        CaseInformationError.InvalidResponse => "NearbyCase.InvalidResponse",
        CaseInformationError.RestoreUncertain => "NearbyCase.RestoreUncertain",
        CaseInformationError.Busy => "NearbyCase.Busy",
        _ => "NearbyCase.Unreachable",
    };
}
