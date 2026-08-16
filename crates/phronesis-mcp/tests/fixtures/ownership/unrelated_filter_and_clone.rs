// Adversarial case 7: a .filter(..) statement and a .clone() statement on
// adjacent lines but in separate statements, so they are in different
// expression chains. Per D2: no edge.

fn unrelated_filter_and_clone() -> Vec<i32> {
    let xs = vec![1, 2, 3];
    let ys: Vec<&i32> = xs.iter().filter(|x| *x > 1).collect();
    let zs: Vec<i32> = xs.clone();
    vec![]
}
