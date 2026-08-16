// Core case 5: an async fn with two synchronous lock acquisitions bound to
// named guards, where the enclosing block of each guard ends *before* a
// later .await.

use std::sync::Mutex;

async fn lock_scope_ends_before_await(m: &Mutex<()>) {
    {
        let _g1 = m.lock().unwrap();
        // guard g1's block ends here
    }
    {
        let _g2 = m.lock().unwrap();
        // guard g2's block ends here
    }
    do_something().await;
}

async fn do_something() {}
