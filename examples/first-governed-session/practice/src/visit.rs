//! Additional source fixture for the executable onboarding checker.
//! The interactive tutorial edits notes.md; this file needs no build.

pub fn visits_today(count: &str) -> usize {
    match count.parse::<usize>() {
        Ok(n) => n,
        Err(_) => 0,
    }
}
