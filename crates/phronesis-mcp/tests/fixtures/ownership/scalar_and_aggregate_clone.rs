// Adversarial case 8: two clones with syntactically identical shape — one of
// a small scalar/identifier and one of a large collection. The point is that
// syntax cannot tell them apart.

fn scalar_and_aggregate_clone() {
    let small = 42i32;
    let _cloned_small = small.clone();

    let big = vec![0u8; 10_000];
    let _cloned_big = big.clone();
}
