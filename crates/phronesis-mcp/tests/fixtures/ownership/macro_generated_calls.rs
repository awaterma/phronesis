// Adversarial case 13: a declarative macro (macro_rules!) whose expansion
// contains .clone(), plus an invocation of it.

macro_rules! make_clone {
    ($x:expr) => {
        $x.clone()
    };
}

fn macro_generated_calls() -> Vec<i32> {
    let data = vec![1, 2, 3];
    let cloned = make_clone!(data);
    let _used = cloned.len();
    vec![]
}
