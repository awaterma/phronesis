// Adversarial case 18: one function exercising each distinct clone operation
// kind separately, clearly separated: .clone(), .cloned(), .to_owned(),
// .to_string(), and .collect().

fn clone_method() {
    let data = vec![1, 2, 3];
    let _c = data.clone();
}

fn cloned_method() {
    let data = vec![1, 2, 3];
    let _c: Vec<i32> = data.iter().cloned().collect();
}

fn to_owned_method() {
    let s = "hello";
    let _o: String = s.to_owned();
}

fn to_string_method() {
    let n = 42;
    let _s = n.to_string();
}

fn collect_method() {
    let data = vec![1, 2, 3];
    let _c: Vec<i32> = data.iter().cloned().collect();
}
