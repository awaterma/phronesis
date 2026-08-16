// Adversarial case 11: a lock acquired without binding it to a name.
//
// D6 case 3: an unbound temporary has no guard *name*, but it does have a
// drop point — Rust releases it at the end of the enclosing statement — so a
// scope conclusion is available whenever that statement ends before an await.
//
// The last function is the boundary control: a temporary `match` scrutinee
// lives across the whole match expression, so an await inside the match is
// still covered by the guard and must yield nothing.

use std::sync::Mutex;

fn unbound_temporary_guard(m: &Mutex<Holder>) -> i32 {
    m.lock().field
}

async fn unbound_temporary_released_before_await(m: &Mutex<Holder>) {
    let value = m.lock().field;
    use_value(value);
    step().await;
}

async fn unbound_temporary_acquired_after_await(m: &Mutex<Holder>) {
    step().await;
    let value = m.lock().field;
    use_value(value);
}

async fn unbound_temporary_scrutinee_lives_across_await(m: &Mutex<Holder>) {
    match m.lock() {
        _ => {
            step().await;
        }
    }
}

struct Holder {
    field: i32,
}

fn use_value(_v: i32) {}

async fn step() {}
