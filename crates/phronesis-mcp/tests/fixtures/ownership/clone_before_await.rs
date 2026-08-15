// Core case 2: an async fn that performs a .clone() and later has an
// .await in the same body.

async fn clone_then_await() {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    let _result = async_work(&cloned).await;
}

async fn async_work(_data: &[i32]) -> i32 {
    0
}
