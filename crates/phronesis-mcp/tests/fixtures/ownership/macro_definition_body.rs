// D22: macro_rules! definition bodies emit nothing.
// A site inside a macro definition has no enclosing function and therefore
// no canonical function ID.

macro_rules! macro_with_clone_and_lock {
    ($x:expr) => {
        let _cloned = $x.clone();
        let _g = std::sync::Mutex::new(()).lock();
    };
}

fn macro_invocation_with_own_clone() {
    let data = vec![1, 2, 3];
    macro_with_clone_and_lock!(data);
    // This real .clone() is inside a normal function — it DOES produce a site
    let _real_clone = data.clone();
}
