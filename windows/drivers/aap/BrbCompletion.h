#pragma once

static NTSTATUS LpCompletedBrbStatus(NTSTATUS IoStatus, NTSTATUS BrbStatus,
                                    BOOLEAN BluetoothError, BOOLEAN Closing,
                                    NTSTATUS AbortStatus)
{
    NTSTATUS status = IoStatus;
    if (status == STATUS_TIMEOUT) status = STATUS_IO_TIMEOUT;
    if (NT_SUCCESS(status) && (BrbStatus == STATUS_TIMEOUT || !NT_SUCCESS(BrbStatus))) {
        status = BrbStatus == STATUS_TIMEOUT ? STATUS_IO_TIMEOUT : BrbStatus;
    }
    if (NT_SUCCESS(status) && BluetoothError) status = STATUS_UNSUCCESSFUL;

    /* A completed successful CLOSE must not restore an already released handle. */
    if (AbortStatus != STATUS_SUCCESS && (!Closing || !NT_SUCCESS(status))) {
        return AbortStatus;
    }
    return status;
}
