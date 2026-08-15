// Adversarial case 12: clone and await sites separated by control-flow
// boundaries. One function per shape, clearly named.

// 12a: early return between clone and await
async fn control_flow_early_return() -> Option<String> {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    if cloned.is_empty() {
        return None;
    }
    let _result = async_work(&cloned).await;
    Some(String::new())
}

// 12b: match arm separates clone from await
async fn control_flow_match() {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    match cloned.len() {
        0 => {}
        _ => {
            let _result = async_work(&cloned).await;
        }
    }
}

// 12c: loop body separates clone from await
async fn control_flow_loop() {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    for _ in 0..1 {
        let _result = async_work(&cloned).await;
    }
}

// 12d: closure separates clone from await
async fn control_flow_closure() {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    let _f = || async move {
        let _result = async_work(&cloned).await;
    };
}

// 12e: nested async block separates clone from await
async fn control_flow_nested_async() {
    let data = vec![1, 2, 3];
    let cloned = data.clone();
    async {
        let _result = async_work(&cloned).await;
    }
    .await;
}

async fn async_work(_data: &[i32]) -> i32 {
    0
}
