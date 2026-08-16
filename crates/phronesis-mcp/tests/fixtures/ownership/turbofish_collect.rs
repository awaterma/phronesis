// Adversarial case 15: collect calls written with turbofish (collect::<Vec<_>>())
// and with a typed binding (let v: Vec<_> = ...). Per the decisions doc,
// turbofish calls parse differently (generic_function wraps the callee).

fn turbofish_collect() -> Vec<i32> {
    // Turbofish on method call
    let _v1: Vec<i32> = (1..10).filter(|x| *x > 5).collect::<Vec<_>>();

    // Turbofish on typed binding
    let v2: Vec<String> = vec![1, 2, 3].iter().map(|x| x.to_string()).collect();

    // Turbofish on method call with clone in chain
    let data = vec![1, 2, 3];
    let _v3: Vec<i32> = data.iter().filter(|x| *x > 1).cloned().collect::<Vec<_>>();

    vec![]
}
