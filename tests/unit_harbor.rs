/// Unit tests for Harbor webhook payload deserialization and helpers.
use updock::harbor::{EventData, HarborEvent, Repository, Resource};

// ── helpers ───────────────────────────────────────────────────────────────────

fn push_event_json(
    repo_full_name: &str,
    namespace: &str,
    repo_name: &str,
    tags: &[&str],
) -> String {
    let resources: Vec<String> = tags
        .iter()
        .map(|t| {
            format!(
                r#"{{"digest":"sha256:abc","tag":"{t}","resource_url":"registry/{repo_full_name}:{t}"}}"#
            )
        })
        .collect();
    format!(
        r#"{{
  "type": "PUSH_ARTIFACT",
  "occur_at": 1700000000,
  "operator": "admin",
  "event_data": {{
    "resources": [{resources}],
    "repository": {{
      "name": "{repo_name}",
      "namespace": "{namespace}",
      "full_name": "{repo_full_name}",
      "type": "private"
    }}
  }}
}}"#,
        resources = resources.join(","),
        repo_name = repo_name,
        namespace = namespace,
        repo_full_name = repo_full_name,
    )
}

fn deserialize(json: &str) -> HarborEvent {
    serde_json::from_str(json).expect("failed to deserialize HarborEvent")
}

// ── deserialization ───────────────────────────────────────────────────────────

#[test]
fn push_artifact_event_deserializes() {
    let json = push_event_json("myproject/myimage", "myproject", "myimage", &["latest"]);
    let event = deserialize(&json);
    assert_eq!(event.event_type, "PUSH_ARTIFACT");
    assert_eq!(event.occur_at, Some(1700000000));
    assert_eq!(event.operator.as_deref(), Some("admin"));
}

#[test]
fn event_type_delete_deserializes() {
    let json = r#"{
      "type": "DELETE_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "img", "namespace": "ns"}
      }
    }"#;
    let event = deserialize(json);
    assert_eq!(event.event_type, "DELETE_ARTIFACT");
}

#[test]
fn occur_at_optional_when_absent() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "img"}
      }
    }"#;
    let event = deserialize(json);
    assert!(event.occur_at.is_none());
}

#[test]
fn operator_optional_when_absent() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "img"}
      }
    }"#;
    let event = deserialize(json);
    assert!(event.operator.is_none());
}

// ── repository_full_name ──────────────────────────────────────────────────────

#[test]
fn full_name_taken_from_full_name_field() {
    let json = push_event_json("myproject/myimage", "myproject", "myimage", &["latest"]);
    let event = deserialize(&json);
    assert_eq!(event.repository_full_name(), "myproject/myimage");
}

#[test]
fn full_name_constructed_from_namespace_and_name_when_full_name_absent() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "myimage", "namespace": "myproject"}
      }
    }"#;
    let event = deserialize(json);
    assert_eq!(event.repository_full_name(), "myproject/myimage");
}

#[test]
fn full_name_falls_back_to_name_when_no_namespace() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "myimage"}
      }
    }"#;
    let event = deserialize(json);
    assert_eq!(event.repository_full_name(), "myimage");
}

#[test]
fn full_name_prefers_full_name_over_namespace_name() {
    // full_name set but different from namespace/name
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {
          "name": "img",
          "namespace": "ns",
          "full_name": "override/value"
        }
      }
    }"#;
    let event = deserialize(json);
    assert_eq!(event.repository_full_name(), "override/value");
}

#[test]
fn full_name_empty_string_falls_back_to_namespace_name() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {
          "name": "myimage",
          "namespace": "myns",
          "full_name": ""
        }
      }
    }"#;
    let event = deserialize(json);
    assert_eq!(event.repository_full_name(), "myns/myimage");
}

// ── pushed_tags ───────────────────────────────────────────────────────────────

#[test]
fn pushed_tags_single_tag() {
    let json = push_event_json("ns/img", "ns", "img", &["latest"]);
    let event = deserialize(&json);
    assert_eq!(event.pushed_tags(), vec!["latest"]);
}

#[test]
fn pushed_tags_multiple_tags() {
    let json = push_event_json("ns/img", "ns", "img", &["latest", "v1.0", "stable"]);
    let event = deserialize(&json);
    let tags = event.pushed_tags();
    assert_eq!(tags.len(), 3);
    assert!(tags.contains(&"latest"));
    assert!(tags.contains(&"v1.0"));
    assert!(tags.contains(&"stable"));
}

#[test]
fn pushed_tags_empty_when_no_resources() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [],
        "repository": {"name": "img"}
      }
    }"#;
    let event = deserialize(json);
    assert!(event.pushed_tags().is_empty());
}

#[test]
fn pushed_tags_skips_resources_without_tag() {
    let json = r#"{
      "type": "PUSH_ARTIFACT",
      "event_data": {
        "resources": [
          {"digest": "sha256:abc"},
          {"digest": "sha256:def", "tag": "v2.0"}
        ],
        "repository": {"name": "img"}
      }
    }"#;
    let event = deserialize(json);
    let tags = event.pushed_tags();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0], "v2.0");
}

// ── direct struct construction ────────────────────────────────────────────────

#[test]
fn harbor_event_constructed_directly() {
    let event = HarborEvent {
        event_type: "PUSH_ARTIFACT".to_string(),
        occur_at: Some(123),
        operator: None,
        event_data: EventData {
            resources: vec![Resource {
                digest: Some("sha256:abc".to_string()),
                tag: Some("v3.0".to_string()),
                resource_url: None,
            }],
            repository: Repository {
                name: "myimage".to_string(),
                namespace: Some("myproject".to_string()),
                full_name: None,
                repo_type: None,
            },
        },
    };
    assert_eq!(event.repository_full_name(), "myproject/myimage");
    assert_eq!(event.pushed_tags(), vec!["v3.0"]);
}
