using System.Globalization;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class CompanionSetupControls
{
    private ulong _epoch;
    private bool _ready;
    private ulong _pendingEpoch;
    private string _pendingDevice = "";
    private string _pendingCommand = "";
    private bool _linkReceipt;
    private PairingWindow? _window;
    private string? _dialogErrorKey;

    private sealed class PairingWindow(ulong request, ulong epoch, string device, string peer, long deadline)
    {
        public readonly ulong Request = request;
        public readonly ulong Epoch = epoch;
        public readonly string Device = device;
        public readonly string Peer = peer;
        public long Deadline = deadline;
        public string? Phase;
        public bool Acknowledged;
    }

    public string DeviceId { get; private set; } = "";
    public CompanionStatus? Status { get; private set; }
    public ulong? Pending { get; private set; }
    public bool Failed { get; private set; }
    public string? DialogErrorKey => Failed ? "CompanionSetup_SaveFailed" : _dialogErrorKey;
    public bool AwaitingStatus { get; private set; }
    public bool CanLink => _ready && Status is not null && ValidDevice(DeviceId) && !Pending.HasValue && !AwaitingStatus && !PairingActive;
    public bool CanForget => _ready && Status is { Linked: true } && !Pending.HasValue && !AwaitingStatus && !PairingActive;
    public ulong? PairingRequestId => _window?.Request;
    public string? PairingPhase => _window?.Phase;
    public bool PairingActive => _window is not null && _window.Phase is null or "waiting" or "authenticating";

    public bool CanRevealCode(string preparedDevice, string preparedPeer, string? selectedPeer, bool acknowledged, long nowMs = long.MinValue) =>
        acknowledged && _linkReceipt && _ready && !Failed && !Pending.HasValue && !AwaitingStatus && ValidDevice(preparedDevice) &&
        string.Equals(DeviceId, preparedDevice, StringComparison.OrdinalIgnoreCase) &&
        NormalizePeer(preparedPeer) is not null &&
        string.Equals(selectedPeer, preparedPeer, StringComparison.OrdinalIgnoreCase) &&
        (_window is { } window
            ? window.Acknowledged && PairingActive && RemainingPairingMs(Now(nowMs)) > 0 &&
                Matches(window, Status?.Pairing) &&
                string.Equals(window.Device, preparedDevice, StringComparison.OrdinalIgnoreCase) &&
                string.Equals(window.Peer, preparedPeer, StringComparison.OrdinalIgnoreCase)
            : Status is { Linked: true } status &&
                string.Equals(status.DeviceId, preparedDevice, StringComparison.OrdinalIgnoreCase) &&
                string.Equals(status.PeerAddress, preparedPeer, StringComparison.OrdinalIgnoreCase));

    public void SetSession(ulong epoch, bool ready)
    {
        if (_epoch != epoch || !ready)
        {
            RetirePending();
            Status = null;
            Failed = false;
            _dialogErrorKey = null;
            _window = null;
        }
        _epoch = epoch;
        _ready = ready;
    }

    public void Update(Snapshot snapshot)
    {
        string device = CompanionControls.SelectedDeviceId(snapshot).ToLowerInvariant();
        if (!string.Equals(DeviceId, device, StringComparison.Ordinal))
        {
            RetirePending();
            Failed = false;
            _dialogErrorKey = null;
            _window = null;
        }
        DeviceId = device;
    }

    public CompanionCancelPairingCmd? Apply(CompanionStatus status, long receivedAtMs = long.MinValue)
    {
        if (!_ready) return null;
        Status = status;
        AwaitingStatus = false;
        if (_window is not { } window) return null;
        CompanionCancelPairingCmd? expired = null;
        if (Matches(window, status.Pairing))
        {
            var pairing = status.Pairing!;
            if (pairing.Phase == "completed" || PairingActive)
            {
                window.Phase = pairing.Phase;
                if (pairing.Phase == "completed") { Failed = false; _dialogErrorKey = null; }
                if (PairingActive)
                {
                    window.Deadline = Math.Min(window.Deadline, Now(receivedAtMs) + pairing.RemainingMs);
                    var cancel = CancelPairingCommand(_epoch);
                    if (ExpirePairing(Now(receivedAtMs))) expired = cancel;
                }
                else { window.Acknowledged = false; _linkReceipt = false; }
            }
        }
        else if (window.Acknowledged && !Pending.HasValue)
        {
            window.Phase = "cancelled";
            window.Acknowledged = false;
            _linkReceipt = false;
        }
        return expired;
    }

    public CompanionBeginPairingCmd? BeginPairing(string? peer, string? code, ulong requestId, long nowMs)
    {
        string? address = NormalizePeer(peer);
        if (!CanLink || requestId == 0 || address is null || !ValidPairingCode(code) ||
            string.Equals(address.Replace(":", ""), DeviceId, StringComparison.OrdinalIgnoreCase)) return null;
        Begin("companion_begin_pairing", requestId);
        _window = new(requestId, _epoch, DeviceId, address, nowMs + 120_000);
        return new() { PeerAddress = address, DeviceId = DeviceId, Code = code!, RequestId = requestId };
    }

    public CompanionCancelPairingCmd? CancelPairingCommand(ulong epoch) =>
        _window is { } window && window.Epoch == epoch && PairingActive
            ? new() { RequestId = window.Request } : null;

    public long RemainingPairingMs(long nowMs) => _window is { } window ? Math.Clamp(window.Deadline - nowMs, 0, 120_000) : 0;

    public bool ExpirePairing(long nowMs)
    {
        if (!PairingActive || RemainingPairingMs(nowMs) != 0) return false;
        RetirePending();
        _window!.Phase = "expired";
        _window.Acknowledged = false;
        return true;
    }

    public static bool ValidPairingCode(string? code) => code is { Length: 8 } && code.All(c => c is >= '0' and <= '9');
    public static string DisplayPairingCode(string code) => ValidPairingCode(code) ? code[..4] + " " + code[4..] : "";

    private static long Now(long value) => value == long.MinValue ? Environment.TickCount64 : value;
    private static bool Matches(PairingWindow window, CompanionPairingStatus? pairing) =>
        pairing is { IsValid: true } && pairing.RequestId == window.Request &&
        string.Equals(pairing.DeviceId, window.Device, StringComparison.OrdinalIgnoreCase) &&
        string.Equals(pairing.PeerAddress, window.Peer, StringComparison.OrdinalIgnoreCase);

    public CompanionLinkCmd? BeginLink(string? peer, string? secret, ulong requestId)
    {
        string? address = NormalizePeer(peer);
        string? code = NormalizeSecret(secret);
        if (!CanLink || requestId == 0 || address is null || code is null ||
            string.Equals(address.Replace(":", ""), DeviceId, StringComparison.OrdinalIgnoreCase)) return null;
        Begin("companion_link", requestId);
        return new() { PeerAddress = address, SecretHex = code, DeviceId = DeviceId, RequestId = requestId };
    }

    public CompanionForgetCmd? BeginForget(ulong requestId)
    {
        if (!CanForget || requestId == 0) return null;
        Begin("companion_forget", requestId);
        return new() { RequestId = requestId };
    }

    public bool Complete(DaemonEvent.CommandResult result, ulong epoch)
    {
        if (!_ready || epoch != _epoch || epoch != _pendingEpoch || Pending != result.RequestId ||
            !Pending.HasValue || _pendingDevice != DeviceId || result.Command != _pendingCommand) return false;
        RetirePending();
        Failed = !result.Ok;
        AwaitingStatus = result.Ok;
        _linkReceipt = result.Ok && result.Command is "companion_link" or "companion_begin_pairing";
        if (result.Ok) _dialogErrorKey = null;
        if (_window is { } window && result.Command == "companion_begin_pairing")
        {
            window.Acknowledged = result.Ok;
            if (!result.Ok) window.Phase = "failed";
        }
        return true;
    }

    public void SendFailed()
    {
        RetirePending();
        Failed = true;
        if (_window is { } window) { window.Phase = "failed"; window.Acknowledged = false; }
    }

    public void Dismiss()
    {
        RetirePending();
        Failed = false;
        _dialogErrorKey = null;
        _window = null;
    }

    public void SetDialogError(string? key) => _dialogErrorKey = key;

    public static string? NormalizeSecret(string? value)
    {
        string code = value?.Trim().ToLowerInvariant() ?? "";
        return code.Length == 64 && code.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f') ? code : null;
    }

    private void Begin(string command, ulong requestId)
    {
        Pending = requestId;
        _pendingEpoch = _epoch;
        _pendingDevice = DeviceId;
        _pendingCommand = command;
        _linkReceipt = false;
        _window = null;
        Failed = false;
        _dialogErrorKey = null;
    }

    private void RetirePending()
    {
        Pending = null;
        _pendingEpoch = 0;
        _pendingDevice = _pendingCommand = "";
        _linkReceipt = false;
        AwaitingStatus = false;
    }

    private static bool ValidDevice(string value) => value.Length == 12 && Address(value);

    private static string? NormalizePeer(string? value)
    {
        string address = value?.Trim().ToUpperInvariant() ?? "";
        return address.Length == 17 && address.Where((_, index) => index % 3 == 2).All(c => c == ':') &&
            Address(address.Replace(":", "")) ? address : null;
    }

    private static bool Address(string value) => value.Length == 12 && value.All(Uri.IsHexDigit) &&
        ulong.TryParse(value, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out ulong address) &&
        address is > 0 and < 0xffffffffffff and not 0x020000000000;
}
