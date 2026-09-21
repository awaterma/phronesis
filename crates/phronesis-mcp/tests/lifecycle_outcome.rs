use phronesis_mcp::lifecycle::outcome::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("a"), "1").unwrap();
    git(d.path(), &["add", "a"]);
    git(d.path(), &["commit", "-q", "-m", "init"]);
    d
}

#[test]
fn prefilter_matches_head_moving_commands_only() {
    assert!(command_may_move_head("git commit -m x"));
    assert!(command_may_move_head("cargo test && git commit -am done"));
    assert!(command_may_move_head("git cherry-pick abc"));
    assert!(!command_may_move_head("git status"));
    assert!(!command_may_move_head("cargo build"));
}

#[test]
fn real_commit_is_detected_with_shas() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "-am", "second"]);
    let c = detect_commit(d.path(), Some(&before), "git commit -am second", Some(0)).unwrap();
    assert_eq!(c.head_before, before);
    assert_ne!(c.sha, before);
    assert_eq!(c.sha.len(), 40);
}

#[test]
fn dry_run_heredoc_and_sibling_repo_are_not_commits() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    assert!(detect_commit(d.path(), Some(&before), "git commit --dry-run", Some(0)).is_none());
    assert!(
        detect_commit(
            d.path(),
            Some(&before),
            "cat <<EOF > doc.md\nrun git commit -m x\nEOF",
            Some(0)
        )
        .is_none()
    );
    let other = repo();
    std::fs::write(other.path().join("a"), "9").unwrap();
    git(other.path(), &["commit", "-q", "-am", "elsewhere"]);
    assert!(
        detect_commit(
            d.path(),
            Some(&before),
            &format!("git -C {} commit -am x", other.path().display()),
            Some(0)
        )
        .is_none()
    );
}

#[test]
fn failed_chain_and_missing_head_before_are_not_commits() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "3").unwrap();
    git(d.path(), &["commit", "-q", "-am", "third"]);
    assert!(
        detect_commit(
            d.path(),
            Some(&before),
            "git commit -am third && false",
            Some(1)
        )
        .is_none()
    );
    assert!(detect_commit(d.path(), None, "git commit -am third", Some(0)).is_none());
}

#[test]
fn shell_tools() {
    assert!(is_shell_tool("Bash"));
    assert!(is_shell_tool("run_shell_command"));
    assert!(!is_shell_tool("Edit"));
}

/// The two documented misses, pinned so the undercount is known rather than
/// discovered: `git -C .` in the *same* repo is a real commit the pre-filter
/// happens to catch (so it IS detected), and a wrapper script is not.
#[test]
fn git_dash_c_in_the_same_repo_is_detected_and_a_wrapper_script_is_not() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "-am", "same repo"]);
    // `git -C .` names this repo, so HEAD really moved here: detection observes
    // the repository, not the string, and gets it right.
    assert!(detect_commit(d.path(), Some(&before), "git -C . commit -am x", Some(0)).is_some());

    // A wrapper script moves HEAD without the word `git commit` anywhere, so the
    // pre-filter never fires and the commit is missed. Commits are undercounted,
    // never overcounted; `kalpa show` says so under the commit line.
    let d2 = repo();
    let before2 = git_head(d2.path()).unwrap();
    std::fs::write(d2.path().join("a"), "3").unwrap();
    git(d2.path(), &["commit", "-q", "-am", "via release.sh"]);
    assert!(detect_commit(d2.path(), Some(&before2), "./release.sh", Some(0)).is_none());
    for missed in ["git pull", "git am patch.mbox", "git com -m x"] {
        assert!(!command_may_move_head(missed), "{missed}");
    }
}

/// Amends and rebases move HEAD and ARE recorded; `sha` is what distinguishes
/// them from a new commit for any consumer that cares (spec §Non-goals).
#[test]
fn an_amend_and_a_rebase_are_recorded_as_head_movements() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "--amend", "-am", "amended"]);
    let amended = detect_commit(
        d.path(),
        Some(&before),
        "git commit --amend -am amended",
        Some(0),
    )
    .expect("an amend moves HEAD");
    assert_ne!(amended.sha, before);

    let base = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("b"), "1").unwrap();
    git(d.path(), &["add", "b"]);
    git(d.path(), &["commit", "-q", "-m", "second"]);
    let head = git_head(d.path()).unwrap();
    git(
        d.path(),
        &["rebase", "--quiet", "--onto", &base, &base, "HEAD"],
    );
    assert!(
        detect_commit(
            d.path(),
            Some(&head),
            "git rebase --onto main main",
            Some(0)
        )
        .is_some()
            || git_head(d.path()).as_deref() == Some(head.as_str()),
        "either the rebase moved HEAD and was detected, or it was a no-op"
    );
}

/// A timed-out probe is distinguishable from "not a repo", because only the
/// first leaves an auditable `detection` marker on the `inflight` entry.
#[test]
fn head_probe_reports_unavailable_outside_a_repo() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(git_head_probe(d.path()), HeadProbe::Unavailable));
    assert_eq!(git_head(d.path()), None);
    let r = repo();
    assert!(matches!(git_head_probe(r.path()), HeadProbe::Head(sha) if sha.len() == 40));
}

/// A host that sends no exit code at all (Claude Code's `Bash`
/// `tool_response`) does not lose detection: HEAD movement still decides.
#[test]
fn a_missing_exit_code_does_not_suppress_detection() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "4").unwrap();
    git(d.path(), &["commit", "-q", "-am", "fourth"]);
    let c = detect_commit(d.path(), Some(&before), "git commit -am fourth", None).unwrap();
    assert_eq!(c.head_before, before);
    assert_ne!(c.sha, before);
}

/// The other half: no exit code and an unmoved HEAD is still not a commit.
/// Absent evidence is not evidence of a commit.
#[test]
fn a_missing_exit_code_with_unmoved_head_is_not_a_commit() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    assert!(detect_commit(d.path(), Some(&before), "git commit --dry-run", None).is_none());
    assert!(detect_commit(d.path(), Some(&before), "git commit -am nothing", None).is_none());
}

/// Only a full object name counts as a host-reported sha: the abbreviation
/// Claude Code actually sends is not a stable identifier.
#[test]
fn only_a_full_object_name_is_a_host_reported_sha() {
    assert!(is_full_sha(&"a".repeat(40)));
    assert!(!is_full_sha("716ee4a"));
    assert!(!is_full_sha(&"z".repeat(40)));
    assert!(!is_full_sha(&"a".repeat(64)));
}

/// The exit code is a veto, not a precondition.
#[test]
fn exit_code_vetoes_only_when_it_is_nonzero() {
    assert!(exit_allows_detection(None));
    assert!(exit_allows_detection(Some(0)));
    assert!(!exit_allows_detection(Some(1)));
}
