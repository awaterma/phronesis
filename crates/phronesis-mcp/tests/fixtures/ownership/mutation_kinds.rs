// Adversarial case 19: one example of each mutation form, plus a plain
// assignment to a bare local (not a mutation per D3).

struct Holder {
    x: i32,
    y: Vec<i32>,
}

fn mutation_get_mut() {
    let mut v = vec![1, 2, 3];
    let _ref = v.get_mut(0);
}

fn mutation_iter_mut() {
    let mut v = vec![1, 2, 3];
    let _iter = v.iter_mut();
}

fn mutation_field_assignment() {
    let mut h = Holder { x: 0, y: vec![] };
    h.x = 5;
}

fn mutation_index_assignment() {
    let mut v = vec![1, 2, 3];
    v[0] = 99;
}

fn mutation_compound_assignment() {
    let mut n = 0;
    n += 1;
}

fn mutation_plain_assignment_is_not_mutation() {
    let mut x = 0;
    x = 1;
}
