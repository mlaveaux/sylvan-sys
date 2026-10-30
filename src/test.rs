use crate::*;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

static CALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn termination_handler() {
    CALLED.store(true, Ordering::SeqCst);
}

/// Node counts computed inside the Lace worker, checked afterwards on the test thread.
#[derive(Default)]
struct Results {
    a_and_b: usize,
    not_a_or_b: usize,
    identity_holds: bool,
    or_equals_not_nand: bool,
    min_intermediate: usize,
    big_bdd: usize,
}

unsafe extern "C" fn sylvan_task(context: *mut c_void) {
    unsafe {
        let results = &mut *(context as *mut Results);

        // Check that the basic identity (a & b) <=> !(!a | !b) holds.
        let a = sylvan_ithvar(1);
        let b = sylvan_ithvar(2);

        let not_a = mtbdd_not(a);
        let not_b = mtbdd_not(b);

        let a_and_b = mtbdd_times(a, b);
        let not_a_or_b = mtbdd_not(mtbdd_plus(not_a, not_b));

        results.a_and_b = sylvan_nodecount(a_and_b);
        results.not_a_or_b = sylvan_nodecount(not_a_or_b);
        results.identity_holds = a_and_b == not_a_or_b;
        results.or_equals_not_nand = sylvan_or(a, b) == sylvan_nand(not_a, not_b);

        let mut big_bdd = sylvan_true;
        results.min_intermediate = usize::MAX;
        for i in 1..20 {
            let x = sylvan_ithvar(i);
            let y = sylvan_ithvar(2 * i);
            let x_iff_y = sylvan_xor(x, y);
            big_bdd = sylvan_and(big_bdd, x_iff_y);

            let reported = sylvan_nodecount(big_bdd);
            results.min_intermediate = results.min_intermediate.min(reported);
        }

        results.big_bdd = sylvan_nodecount(big_bdd);
    }
}

#[test]
pub fn basic_functionality_test() {
    unsafe {
        lace_start(0, 0); // auto-detect thread count

        // 2^26 = ~64MB, should be enough for this test.
        sylvan_set_limits(1 << 26, 1, 5);
        sylvan_init_package();
        sylvan_init_bdd();
        sylvan_gc_disable();
        sylvan_register_quit(Some(termination_handler));

        // Sylvan operations must run inside a Lace worker.
        let mut results = Results::default();
        lace_run_callback(
            Some(sylvan_task),
            &mut results as *mut Results as *mut c_void,
        );

        assert_eq!(3, results.a_and_b);
        assert_eq!(3, results.not_a_or_b);
        assert!(results.identity_holds);
        assert!(results.or_equals_not_nand);
        assert!(results.min_intermediate > 0);
        assert_eq!(4091, results.big_bdd);

        sylvan_quit();
        lace_stop();

        assert!(CALLED.load(Ordering::SeqCst));
    }
}
