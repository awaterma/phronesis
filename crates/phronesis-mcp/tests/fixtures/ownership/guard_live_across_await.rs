// Adversarial case 10: a guard genuinely still in scope across an .await
// (the extractor must NOT claim it crosses; it simply emits no scope
// relation).

use std::sync::Mutex;

async fn guard_live_across_await(m: &Mutex<()>) {
    let _g = m.lock().unwrap();
    // guard is still alive here
    do_something().await;
    // guard drops at end of function
}

async fn do_something() {}
