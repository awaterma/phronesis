// Adversarial case 14: negative control — .clone(), .lock(), and .await
// appear only inside comments and string literals. Zero real sites expected.

// This line comment contains .clone() and .lock() and .await
/* Block comment with .clone() and .lock() and .await */
/// Doc comment with .clone() and .lock() and .await
fn comments_and_strings() {
    let s = "normal string with .clone() and .lock() and .await";
    let raw = r#"raw string with .clone() and .lock() and .await"#;
    let _used = s.len() + raw.len();
}
