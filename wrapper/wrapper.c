#include <stdio.h>
#include <stdlib.h>
#include <wrapper.h>

// The Rust constants in `src/lib.rs` assume that ZDDs use complement edges.
static_assert(zdd_true == UINT64_C(0x8000000000000000), "ZDD_COMPLEMENT_EDGES must be enabled");

VOID_TASK_2(lace_run_callback_task, lace_callback_t, callback, void*, context)
{
    callback(context);
}

void lace_run_callback(lace_callback_t callback, void* context) {
    if (callback == NULL) {
        return;
    }
    if (!lace_is_running()) {
        // Otherwise the task is queued, and waits forever for a worker.
        fprintf(stderr, "lace_run_callback: Lace has not been started, call lace_start first.\n");
        abort();
    }
    RUN(lace_run_callback_task, callback, context);
}

double sylvan_sys_lddmc_satcount(MDD mdd) {
    return (double)lddmc_satcount(mdd);
}
