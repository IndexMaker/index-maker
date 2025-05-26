use arc_swap::ArcSwapAny;
use std::sync::Arc;

pub trait SafeUpdate<T> {
    /// Safe Update
    /// 
    /// Perform modification on the copy, and if all operations were successful
    /// then write into store. Practically this is implemented as an RCU (Read-Compare-Update)
    /// loop, specifically for ArcSwap.
    /// 
    /// Trait requires that `self` is not changed if an `Err()` was returned by `f`, and only
    /// changed if `Ok()` was returned.
    /// 
    fn safe_update<R, E>(&self, f: impl Fn(&mut T) -> Result<R, E>) -> Result<R, E>;
}

impl<T: Clone> SafeUpdate<T> for ArcSwapAny<Arc<T>> {
    fn safe_update<R, E>(&self, f: impl Fn(&mut T) -> Result<R, E>) -> Result<R, E> {
        let mut loaded_guard = self.load();
        loop {
            let mut new_value = (**loaded_guard).clone();

            let result_value = f(&mut new_value)?;

            // Commit
            let new_arc = Arc::new(new_value);
            let prev_guard = self.compare_and_swap(&*loaded_guard, new_arc);
            let swapped = Arc::ptr_eq(&*loaded_guard, &*prev_guard);
            if swapped {
                return Ok(result_value);
            } else {
                loaded_guard = prev_guard;
            }
        }
    }
}
