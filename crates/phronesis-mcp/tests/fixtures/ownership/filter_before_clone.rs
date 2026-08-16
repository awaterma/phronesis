// Core case 1: filter appears in the receiver chain of a cloned() call.
// Per D2 this exercises filter_before_clone (the relation name used by
// tests; the actual derived relation is `filter_before_clone` on the
// expression chain).

fn chained() -> Vec<i32> {
    let data = vec![1, 2, 3, 4, 5];
    let _result: Vec<i32> = data
        .iter()
        .filter(|x| *x > 2)
        .map(|x| x * 2)
        .cloned()
        .collect();
    vec![]
}

// Second function where filter(p).map(f).cloned() has an intervening
// adapter — per D2 this still counts as one chain.
fn chained_with_intervening() -> Vec<i32> {
    let data = vec![10, 20, 30];
    let _result: Vec<i32> = data
        .iter()
        .filter(|x| *x > 15)
        .map(|x| x + 1)
        .cloned()
        .collect();
    vec![]
}

// Third function: direct chain with no intervening adapter.
// Per D2 this also exercises filter_before_clone but without any adapter in between.
fn chained_direct() -> Vec<i32> {
    let data = vec![1, 2, 3, 4, 5];
    let _result: Vec<i32> = data
        .iter()
        .filter(|x| *x > 2)
        .cloned()
        .collect();
    vec![]
}
