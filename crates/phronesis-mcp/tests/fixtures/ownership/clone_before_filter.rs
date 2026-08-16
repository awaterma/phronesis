// Adversarial case 6: reverse order — .cloned() appears BEFORE .filter(..)
// in the chain. There must be no filter in the receiver chain of the clone.

fn clone_before_filter() -> Vec<i32> {
    let data = vec![1, 2, 3];
    // cloned() wraps data directly; filter() is a separate chain
    let _cloned: Vec<i32> = data.iter().cloned().collect();
    let _filtered: Vec<i32> = data.iter().filter(|x| *x > 1).collect();
    vec![]
}
