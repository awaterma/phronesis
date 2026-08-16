// Adversarial case 9: a lock guard explicitly drop()-ed before a later
// .await, in the same block. Per D6 case 2 this exercises the explicit
// drop path.

use std::sync::Mutex;

async fn explicit_drop_before_await(m: &Mutex<()>) {
    let g = m.lock().unwrap();
    drop(g);
    do_something().await;
}

async fn do_something() {}
