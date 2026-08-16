// D20: structural distinction between sync and async lock acquisitions.
// An awaited .lock().await is an async lock and must produce NO sync_lock_site.
// A synchronous .lock() (not awaited) DOES produce a sync_lock_site.
// Same contrast for .read() and .write().

use std::sync::Mutex;

// Async lock — NO sync_lock_site
async fn async_lock(m: &Mutex<()>) {
    let _g = m.lock().await;
}

// Synchronous lock — DOES produce sync_lock_site
fn sync_lock(m: &Mutex<()>) {
    let _g = m.lock().expect("lock");
}

// Async read — NO sync_lock_site
async fn async_read(m: &Mutex<()>) {
    let _g = m.read().await;
}

// Synchronous read — DOES produce sync_lock_site
fn sync_read(m: &Mutex<()>) {
    let _g = m.read().expect("read");
}

// Async write — NO sync_lock_site
async fn async_write(m: &Mutex<()>) {
    let _g = m.write().await;
}

// Synchronous write — DOES produce sync_lock_site
fn sync_write(m: &Mutex<()>) {
    let _g = m.write().expect("write");
}
