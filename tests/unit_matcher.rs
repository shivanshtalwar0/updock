use std::collections::HashMap;
use std::path::PathBuf;
use updock::config::ProjectConfig;
/// Unit tests for tag matching and project matching logic.
use updock::matcher::{matching_projects, tag_matches};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_project(name: &str, harbor_repository: &str, tag_filter: Option<&str>) -> ProjectConfig {
    ProjectConfig {
        name: name.to_string(),
        harbor_repository: harbor_repository.to_string(),
        tag_filter: tag_filter.map(|s| s.to_string()),
        compose_dir: PathBuf::from("/tmp"),
        compose_file: None,
        env: HashMap::new(),
        pull_before_up: true,
    }
}

// ── tag_matches: exact ────────────────────────────────────────────────────────

#[test]
fn exact_match_identical_strings() {
    assert!(tag_matches("latest", "latest"));
    assert!(tag_matches("v1.2.3", "v1.2.3"));
    assert!(tag_matches("production", "production"));
}

#[test]
fn exact_match_case_sensitive() {
    assert!(!tag_matches("Latest", "latest"));
    assert!(!tag_matches("PROD", "prod"));
}

#[test]
fn exact_match_different_strings() {
    assert!(!tag_matches("latest", "v1.0"));
    assert!(!tag_matches("stable", "latest"));
}

#[test]
fn exact_match_empty_strings() {
    assert!(tag_matches("", ""));
    assert!(!tag_matches("", "anything"));
    assert!(!tag_matches("anything", ""));
}

// ── tag_matches: single star ──────────────────────────────────────────────────

#[test]
fn star_matches_any_segment() {
    assert!(tag_matches("v*", "v1.0.0"));
    assert!(tag_matches("v*", "v2"));
    assert!(tag_matches("v*", "v"));
    assert!(tag_matches("*", "anything"));
    assert!(tag_matches("*", ""));
}

#[test]
fn star_does_not_cross_slash() {
    assert!(!tag_matches("v*", "v1/extra"));
    assert!(!tag_matches("*", "a/b"));
    assert!(!tag_matches("foo/*", "foo/bar/baz"));
}

#[test]
fn star_prefix_suffix() {
    assert!(tag_matches("*-prod", "release-prod"));
    assert!(tag_matches("release-*", "release-2024"));
    assert!(tag_matches("*2024*", "v2024rc1"));
    assert!(!tag_matches("*-prod", "release-dev"));
}

#[test]
fn star_middle_wildcard() {
    assert!(tag_matches("v*.*.0", "v1.2.0"));
    assert!(!tag_matches("v*.*.0", "v1.2.1"));
}

// ── tag_matches: double star ──────────────────────────────────────────────────

#[test]
fn double_star_matches_anything_including_slash() {
    assert!(tag_matches("**", "anything"));
    assert!(tag_matches("**", "v1/extra"));
    assert!(tag_matches("**", "a/b/c/d"));
    assert!(tag_matches("**", ""));
}

#[test]
fn double_star_with_prefix() {
    assert!(tag_matches("prefix/**", "prefix/nested/tag"));
    assert!(tag_matches("prefix/**", "prefix/"));
    assert!(!tag_matches("prefix/**", "other/nested/tag"));
}

#[test]
fn double_star_with_suffix() {
    assert!(tag_matches("**/tag", "a/b/tag"));
    assert!(tag_matches("**/tag", "tag"));
}

// ── matching_projects ─────────────────────────────────────────────────────────

#[test]
fn no_projects_returns_empty() {
    let result = matching_projects(&[], "myproject/myimage", &["latest"]);
    assert!(result.is_empty());
}

#[test]
fn single_project_exact_match() {
    let projects = vec![make_project("app", "myproject/myimage", None)];
    let result = matching_projects(&projects, "myproject/myimage", &["latest"]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "app");
}

#[test]
fn repository_match_is_case_insensitive() {
    let projects = vec![make_project("app", "MyProject/MyImage", None)];
    let result = matching_projects(&projects, "myproject/myimage", &["latest"]);
    assert_eq!(result.len(), 1);
}

#[test]
fn repository_mismatch_returns_empty() {
    let projects = vec![make_project("app", "other/repo", None)];
    let result = matching_projects(&projects, "myproject/myimage", &["latest"]);
    assert!(result.is_empty());
}

#[test]
fn tag_filter_matches_one_of_multiple_pushed_tags() {
    let projects = vec![make_project("app", "ns/img", Some("v*"))];
    let result = matching_projects(&projects, "ns/img", &["latest", "v1.0", "dev"]);
    assert_eq!(result.len(), 1);
}

#[test]
fn tag_filter_no_match_returns_empty() {
    let projects = vec![make_project("app", "ns/img", Some("v*"))];
    let result = matching_projects(&projects, "ns/img", &["latest", "dev"]);
    assert!(result.is_empty());
}

#[test]
fn no_tag_filter_matches_any_tag() {
    let projects = vec![make_project("app", "ns/img", None)];
    let result = matching_projects(&projects, "ns/img", &["whatever"]);
    assert_eq!(result.len(), 1);
}

#[test]
fn no_tag_filter_matches_empty_tags() {
    let projects = vec![make_project("app", "ns/img", None)];
    let result = matching_projects(&projects, "ns/img", &[]);
    assert_eq!(result.len(), 1);
}

#[test]
fn multiple_projects_same_repo_both_match() {
    let projects = vec![
        make_project("app-prod", "ns/img", Some("v*")),
        make_project("app-dev", "ns/img", Some("dev")),
    ];
    let result = matching_projects(&projects, "ns/img", &["v1.0", "dev"]);
    assert_eq!(result.len(), 2);
}

#[test]
fn multiple_projects_only_one_matches_tag() {
    let projects = vec![
        make_project("prod", "ns/img", Some("v*")),
        make_project("dev", "ns/img", Some("dev")),
    ];
    let result = matching_projects(&projects, "ns/img", &["v2.0"]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "prod");
}

#[test]
fn double_star_tag_filter_matches_all() {
    let projects = vec![make_project("all", "ns/img", Some("**"))];
    let result = matching_projects(&projects, "ns/img", &["v1/rc1"]);
    assert_eq!(result.len(), 1);
}

#[test]
fn matching_preserves_project_reference_ordering() {
    let projects = vec![
        make_project("first", "ns/img", None),
        make_project("second", "ns/img", None),
        make_project("third", "ns/img", None),
    ];
    let result = matching_projects(&projects, "ns/img", &["latest"]);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].name, "first");
    assert_eq!(result[1].name, "second");
    assert_eq!(result[2].name, "third");
}

// ── edge cases ────────────────────────────────────────────────────────────────

#[test]
fn star_at_end_of_pattern_matches_rest() {
    assert!(tag_matches("release-*", "release-2024-01-15"));
    assert!(tag_matches("rc*", "rc1"));
    assert!(tag_matches("rc*", "rc"));
}

#[test]
fn star_at_start_of_pattern() {
    assert!(tag_matches("*-latest", "main-latest"));
    assert!(!tag_matches("*-latest", "latest")); // needs something before dash
                                                 // Actually "*-latest" with star matching empty: ""."-latest" == "-latest" ≠ "latest"
                                                 // so this should be false — confirmed above
}

#[test]
fn literal_pattern_no_wildcards() {
    assert!(tag_matches("v1.2.3-alpine", "v1.2.3-alpine"));
    assert!(!tag_matches("v1.2.3-alpine", "v1.2.3-debian"));
}
