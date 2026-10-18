using System.Buffers.Binary;

namespace NTPods.WinUI.Services;

internal sealed record CaseMetadata(uint ProtocolVersion, uint Capabilities, uint FirmwareVersion);

internal enum CaseInformationError
{
    None, NotFound, Ambiguous, Paired, Unreachable, Unsupported, InvalidResponse, Cancelled, RestoreUncertain, Busy
}

internal sealed record CaseInformationResult(CaseMetadata? Metadata, CaseInformationError Error)
{
    internal bool Success => Error == CaseInformationError.None && Metadata is not null;
}

internal readonly record struct DultInformationResponse(ushort RequestOpcode, uint? Value, ushort? Status);

internal static class DultCaseProtocol
{
    private static readonly Guid FindingService = new("0000fd44-0000-1000-8000-00805f9b34fb");
    private static readonly Guid DultService = new("15190001-12f4-c226-88ed-2ac5579f2a85");
    internal const ushort ProtocolVersion = 0x0007, Capabilities = 0x0008, FirmwareVersion = 0x000a;
    internal static bool Allowed(ushort opcode) => opcode is ProtocolVersion or Capabilities or FirmwareVersion;

    internal static bool AdvertisesCaseService(IEnumerable<Guid> serviceUuids, IEnumerable<(byte Type, byte[] Data)> serviceData) =>
        serviceUuids.Take(32).Any(uuid => uuid == FindingService || uuid == DultService) ||
        serviceData.Take(32).Any(section => CaseServiceData(section.Type, section.Data));

    private static bool CaseServiceData(byte type, ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length > 512) return false;
        if (type == 0x16) return bytes.Length >= 2 && BinaryPrimitives.ReadUInt16LittleEndian(bytes) == 0xfd44;
        if (type != 0x21 || bytes.Length < 16) return false;
        Span<byte> uuid = stackalloc byte[16];
        bytes[..16].CopyTo(uuid);
        uuid.Reverse();
        return new Guid(uuid, bigEndian: true) == DultService;
    }

    internal static byte[] Request(ushort opcode)
    {
        if (!Allowed(opcode)) throw new ArgumentOutOfRangeException(nameof(opcode));
        byte[] bytes = new byte[2];
        BinaryPrimitives.WriteUInt16LittleEndian(bytes, opcode);
        return bytes;
    }

    internal static bool TryParse(ReadOnlySpan<byte> bytes, out DultInformationResponse response)
    {
        response = default;
        if (bytes.Length != 6) return false;
        ushort opcode = BinaryPrimitives.ReadUInt16LittleEndian(bytes);
        if (opcode is 0x0807 or 0x0808 or 0x080a)
        {
            response = new((ushort)(opcode & 0x07ff), BinaryPrimitives.ReadUInt32LittleEndian(bytes[2..]), null);
            return true;
        }
        ushort request = BinaryPrimitives.ReadUInt16LittleEndian(bytes[2..]);
        if (opcode != 0x0302 || !Allowed(request)) return false;
        response = new(request, null, BinaryPrimitives.ReadUInt16LittleEndian(bytes[4..]));
        return true;
    }

    internal static bool MatchesIdentity(ReadOnlySpan<byte> bytes, ushort product) => bytes.Length == 7 &&
        bytes[0] == 1 && BinaryPrimitives.ReadUInt16LittleEndian(bytes[1..]) == 0x004c &&
        BinaryPrimitives.ReadUInt16LittleEndian(bytes[3..]) == product && BinaryPrimitives.ReadUInt16LittleEndian(bytes[5..]) == 1;

    internal static bool TryParseCccd(ReadOnlySpan<byte> bytes, out ushort value)
    {
        value = 0;
        if (bytes.Length != 2) return false;
        value = BinaryPrimitives.ReadUInt16LittleEndian(bytes);
        return value <= 3;
    }

    internal static CaseInformationError CandidateError(int count) => count switch
    {
        0 => CaseInformationError.NotFound,
        1 => CaseInformationError.None,
        _ => CaseInformationError.Ambiguous
    };

    internal static async Task<bool> RestoreAsync(bool required, Func<Task<bool>> write, Func<Task<bool>> readback)
    {
        if (!required) return true;
        bool written = false, verified = false;
        try { written = await write().ConfigureAwait(false); }
        catch (Exception) { }
        finally { try { verified = await readback().ConfigureAwait(false); } catch (Exception) { } }
        return written && verified;
    }
}

internal sealed class DultResponsePhase(ushort opcode)
{
    private readonly TaskCompletionSource<DultInformationResponse> _response = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private bool _issued;
    internal Task<DultInformationResponse> Pending => _response.Task;
    internal void Issue()
    {
        if (!DultCaseProtocol.Allowed(opcode) || _issued) throw new InvalidOperationException();
        _issued = true;
    }
    internal bool Accept(ReadOnlySpan<byte> bytes)
    {
        if (!_issued || _response.Task.IsCompleted || !DultCaseProtocol.TryParse(bytes, out var response) || response.RequestOpcode != opcode)
            return false;
        return _response.TrySetResult(response);
    }
}
