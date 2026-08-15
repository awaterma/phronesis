// Adversarial case 16: UFCS Clone::clone(&x) and <T as Clone>::clone(&x)
// forms, plus Iterator::filter(xs, p) UFCS form.

use std::clone::Clone;

fn ufcs_clone() -> i32 {
    let data = vec![1, 2, 3];
    let _a: Vec<i32> = Clone::clone(&data);
    let _b: Vec<i32> = <Vec<i32> as Clone>::clone(&data);
    // UFCS filter — per D2 this is a known incompleteness (no filter_before_clone edge)
    let filtered: Vec<&i32> = Iterator::filter(data.iter(), |x| *x > 1);
    filtered.len() as i32
}
