#ifndef SYLVAN_SYS_WRAPPER_H
#define SYLVAN_SYS_WRAPPER_H

#include <sylvan.h>

/*
    The bindings are generated from this header. Everything else is provided by Sylvan
    itself; this header only adds what cannot be expressed from Rust.
*/

typedef void (*lace_callback_t)(void* context);

/**
 * Runs `callback(context)` inside a Lace worker and waits for it to complete. Sylvan
 * operations must be called from within a Lace worker, and Lace tasks can only be defined
 * using C macros. Lace must have been started with `lace_start`, otherwise this aborts.
 */
void lace_run_callback(lace_callback_t callback, void* context);

/**
 * Calls `lddmc_satcount`, converting its `long double` result to `double`.
 */
double sylvan_sys_lddmc_satcount(MDD mdd);

#endif
