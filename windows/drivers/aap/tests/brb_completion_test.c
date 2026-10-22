#include <assert.h>
#include <stdint.h>
#include <stdio.h>

typedef int32_t NTSTATUS;
typedef unsigned char BOOLEAN;
#define STATUS_SUCCESS ((NTSTATUS)0)
#define STATUS_TIMEOUT ((NTSTATUS)0x00000102)
#define STATUS_UNSUCCESSFUL ((NTSTATUS)0xC0000001)
#define STATUS_CANCELLED ((NTSTATUS)0xC0000120)
#define STATUS_IO_TIMEOUT ((NTSTATUS)0xC00000B5)
#define STATUS_DEVICE_REMOVED ((NTSTATUS)0xC00002B6)
#define NT_SUCCESS(status) ((status) >= 0)
#include "../BrbCompletion.h"

int main(void)
{
    const NTSTATUS aborts[] = {
        STATUS_CANCELLED, STATUS_IO_TIMEOUT, STATUS_DEVICE_REMOVED
    };
    const NTSTATUS failures[] = {
        STATUS_TIMEOUT, STATUS_UNSUCCESSFUL, STATUS_CANCELLED,
        STATUS_IO_TIMEOUT, STATUS_DEVICE_REMOVED
    };
    size_t a, f;

    assert(LpCompletedBrbStatus(STATUS_SUCCESS, STATUS_SUCCESS, 0, 1,
                              STATUS_SUCCESS) == STATUS_SUCCESS);
    for (a = 0; a < sizeof(aborts) / sizeof(*aborts); ++a) {
        /* Completion wins the cancellation race only for successful closes. */
        assert(LpCompletedBrbStatus(STATUS_SUCCESS, STATUS_SUCCESS, 0, 1,
                                  aborts[a]) == STATUS_SUCCESS);
        assert(LpCompletedBrbStatus(STATUS_SUCCESS, STATUS_SUCCESS, 0, 0,
                                  aborts[a]) == aborts[a]);
        assert(LpCompletedBrbStatus(STATUS_SUCCESS, STATUS_SUCCESS, 1, 1,
                                  aborts[a]) == aborts[a]);
        for (f = 0; f < sizeof(failures) / sizeof(*failures); ++f) {
            assert(LpCompletedBrbStatus(failures[f], STATUS_SUCCESS, 0, 1,
                                      aborts[a]) == aborts[a]);
            assert(LpCompletedBrbStatus(STATUS_SUCCESS, failures[f], 0, 1,
                                      aborts[a]) == aborts[a]);
        }
    }
    for (f = 0; f < sizeof(failures) / sizeof(*failures); ++f) {
        NTSTATUS expected = failures[f] == STATUS_TIMEOUT
            ? STATUS_IO_TIMEOUT : failures[f];
        assert(LpCompletedBrbStatus(failures[f], STATUS_SUCCESS, 0, 1,
                                  STATUS_SUCCESS) == expected);
        assert(LpCompletedBrbStatus(STATUS_SUCCESS, failures[f], 0, 1,
                                  STATUS_SUCCESS) == expected);
    }
    assert(LpCompletedBrbStatus(STATUS_SUCCESS, STATUS_SUCCESS, 1, 1,
                              STATUS_SUCCESS) == STATUS_UNSUCCESSFUL);
    puts("BRB close completion and cancellation status tests passed");
    return 0;
}
