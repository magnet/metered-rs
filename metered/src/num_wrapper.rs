use std::marker::PhantomData;

/// Metered metrics wrap when the counters are at capacity instead of
/// overflowing or underflowing.
///
/// This struct provides wrapping logic where a metric can be incremented or
/// decremented `N` times, where `N` is `usize`.
///
/// This is the most logical default:
/// * It is observable
/// * Metered should never panic business code
/// * While we could argue that gauges saturate at max capacity, doing
/// so will unbalance the gauge when decrementing the count after a saturated
/// add. Instead we guarantee that for all `N`, each `incr_by(N)` followed by a
/// `decr_by(N)` results in the original value.
///
/// Should we avoid calling `NumWrapper` with `count = 1`, i.e is the optimizer
/// able to get rid of the wrapping computations?  The Godbolt compiler explorer
/// shows that starting `opt-level = 1`, the generated call is equivalent.
/// Program below for future reference:
/// ```rust
/// pub fn wrap(count: usize) -> u8 {
///     (count % (u8::MAX as usize + 1)) as u8
/// }
///
/// pub fn naive(c: u8) -> u8 {
///     c.wrapping_add(wrap(1))
/// }
///
/// pub fn manual(c: u8) -> u8  {
///     c.wrapping_add(1)
/// }
///
///
/// pub fn main(){
///     let m = manual(42);
///     let n = naive(42);
///     println!("{n} {m}");
/// }
/// ```
pub(crate) struct NumWrapper<T>(PhantomData<T>);

macro_rules! impl_num_wrapper_for_smaller_than_usize {
    ($int:path) => {
        impl NumWrapper<$int> {
            /// Wrap count wrapped over $int
            pub(crate) fn wrap(count: usize) -> $int {
                (count % (<$int>::MAX as usize + 1)) as $int
            }
        }
    };
}

macro_rules! impl_num_wrapper_for_equal_or_larger_than_usize {
    ($int:path) => {
        impl NumWrapper<$int> {
            /// Return count as $int
            pub(crate) fn wrap(count: usize) -> $int {
                count as $int
            }
        }
    };
}

cfg_if::cfg_if! {
    if #[cfg(target_pointer_width = "8")] {
        impl_num_wrapper_for_equal_or_larger_than_usize!(u8);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u16);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u32);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u64);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u128);
    } else if #[cfg(target_pointer_width = "16")] {
        impl_num_wrapper_for_smaller_than_usize!(u8);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u16);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u32);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u64);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u128);
    } else if #[cfg(target_pointer_width = "32")] {
        impl_num_wrapper_for_smaller_than_usize!(u8);
        impl_num_wrapper_for_smaller_than_usize!(u16);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u32);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u64);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u128);
    } else if #[cfg(target_pointer_width = "64")] {
        impl_num_wrapper_for_smaller_than_usize!(u8);
        impl_num_wrapper_for_smaller_than_usize!(u16);
        impl_num_wrapper_for_smaller_than_usize!(u32);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u64);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u128);
    } else if #[cfg(target_pointer_width = "128")] {
        impl_num_wrapper_for_smaller_than_usize!(u8);
        impl_num_wrapper_for_smaller_than_usize!(u16);
        impl_num_wrapper_for_smaller_than_usize!(u32);
        impl_num_wrapper_for_smaller_than_usize!(u64);
        impl_num_wrapper_for_equal_or_larger_than_usize!(u128);
    } else {
        compile_error!("Unsupported architecture - unhandled pointer size.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // test with u8 for more wrapping - same model/properties applies to other ints.
    fn incr_by(cc: u8, count: usize) -> u8 {
        let v = NumWrapper::<u8>::wrap(count);
        cc.wrapping_add(v)
    }
    fn incr_by_naive(mut cc: u8, count: usize) -> u8 {
        for _ in 0..count {
            cc = cc.wrapping_add(1);
        }
        cc
    }

    fn decr_by(cc: u8, count: usize) -> u8 {
        let v = NumWrapper::<u8>::wrap(count);
        cc.wrapping_sub(v)
    }
    fn decr_by_naive(mut cc: u8, count: usize) -> u8 {
        for _ in 0..count {
            cc = cc.wrapping_sub(1);
        }
        cc
    }

    proptest! {
        #[test]
        fn test_wrapping_incr(x: u8, y in 0..4096usize) {
            // Tests if calling incr() Y times returns the same value
            // as the optimized version
            assert_eq!(incr_by_naive(x, y), incr_by(x, y));
        }

        #[test]
        fn test_wrapping_decr(x: u8,  y in 0..4096usize) {
            // Tests if calling decr() Y times returns the same value
            // as the optimized version
            assert_eq!(decr_by_naive(x, y), decr_by(x, y));
        }

        #[test]
        fn test_wrapping_incr_decr_symmetric(x: u8, y: usize) {
            // reduce strategy space, usize takes too long
            // Tests if calling decr() Y times on incr() Y times returns
            // the original value
            assert_eq!(x, decr_by(incr_by(x, y), y));
        }
    }
}
