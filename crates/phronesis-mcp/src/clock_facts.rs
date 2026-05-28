//! Wall-clock facts asserted at hook time.
//!
//! Some rules want to fire based on *when* the tool call is happening,
//! not just *what* it is. The classic example: a commit-timing rule
//! that refuses `git commit` during weekday business hours. The rule
//! needs a fact representing "are we currently inside business hours?",
//! evaluated at the moment the hook runs.
//!
//! This module reads the local clock and returns small `(predicate, args)`
//! tuples the hook can assert into the RETE network. The clock is read
//! once per hook invocation; the resulting facts are evaluated by the
//! same equality matcher every other predicate uses, so rules can match
//! on them without any new engine machinery.

use chrono::{Datelike, Local, Timelike, Weekday};

/// A `(predicate, args)` pair ready to be wrapped in a `Fact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockFact {
    pub predicate: &'static str,
    pub args: Vec<String>,
}

/// Read the local clock and return the clock-derived facts a hook
/// should assert. Returns a small fixed set so rules can match
/// equality-style.
///
/// The current facts are:
/// - `business_hours_local("true")` or `business_hours_local("false")` —
///   true when the local clock reads Mon–Fri, 09:00–17:00.
/// - `weekday_local("monday")` / `"tuesday"` / … — lowercase ASCII name.
/// - `hour_local("17")` — current hour as zero-padded-free decimal string.
pub fn now() -> Vec<ClockFact> {
    let now = Local::now();
    facts_for(now.weekday(), now.hour())
}

fn facts_for(weekday: Weekday, hour: u32) -> Vec<ClockFact> {
    let in_business_hours = is_business_hours(weekday, hour);
    vec![
        ClockFact {
            predicate: "business_hours_local",
            args: vec![bool_str(in_business_hours)],
        },
        ClockFact {
            predicate: "weekday_local",
            args: vec![weekday_name(weekday).to_string()],
        },
        ClockFact {
            predicate: "hour_local",
            args: vec![hour.to_string()],
        },
    ]
}

/// `true` when the local clock reads Mon–Fri, 09:00–16:59 inclusive.
/// 17:00 exactly is **outside** the window (the after-hours edge).
fn is_business_hours(weekday: Weekday, hour: u32) -> bool {
    let is_weekday = matches!(
        weekday,
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
    );
    is_weekday && (9..17).contains(&hour)
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

fn bool_str(b: bool) -> String {
    if b {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_at_noon_is_business_hours() {
        assert!(is_business_hours(Weekday::Wed, 12));
    }

    #[test]
    fn weekday_at_9am_sharp_is_business_hours() {
        assert!(is_business_hours(Weekday::Mon, 9));
    }

    #[test]
    fn weekday_at_5pm_sharp_is_after_hours() {
        // 17:00 is the start of the after-hours window — the rule
        // explicitly says "after 5pm", not "5pm onwards".
        assert!(!is_business_hours(Weekday::Mon, 17));
    }

    #[test]
    fn weekday_at_8am_is_before_hours() {
        assert!(!is_business_hours(Weekday::Tue, 8));
    }

    #[test]
    fn saturday_at_noon_is_not_business_hours() {
        assert!(!is_business_hours(Weekday::Sat, 12));
    }

    #[test]
    fn sunday_at_noon_is_not_business_hours() {
        assert!(!is_business_hours(Weekday::Sun, 12));
    }

    #[test]
    fn facts_for_includes_business_hours_predicate() {
        let facts = facts_for(Weekday::Wed, 12);
        assert!(
            facts
                .iter()
                .any(|f| f.predicate == "business_hours_local" && f.args == vec!["true"])
        );
    }

    #[test]
    fn facts_for_business_hours_false_after_5pm() {
        let facts = facts_for(Weekday::Wed, 18);
        assert!(
            facts
                .iter()
                .any(|f| f.predicate == "business_hours_local" && f.args == vec!["false"])
        );
    }

    #[test]
    fn facts_for_includes_weekday_name() {
        let facts = facts_for(Weekday::Tue, 12);
        assert!(
            facts
                .iter()
                .any(|f| f.predicate == "weekday_local" && f.args == vec!["tuesday"])
        );
    }

    #[test]
    fn facts_for_includes_hour() {
        let facts = facts_for(Weekday::Mon, 17);
        assert!(
            facts
                .iter()
                .any(|f| f.predicate == "hour_local" && f.args == vec!["17"])
        );
    }

    #[test]
    fn now_returns_three_facts() {
        let facts = now();
        assert_eq!(facts.len(), 3);
        let predicates: Vec<&str> = facts.iter().map(|f| f.predicate).collect();
        assert!(predicates.contains(&"business_hours_local"));
        assert!(predicates.contains(&"weekday_local"));
        assert!(predicates.contains(&"hour_local"));
    }
}
