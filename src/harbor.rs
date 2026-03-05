use serde::Deserialize;

// ──────────────────────────────────────────────────────────────────────────────
// Harbor webhook payload structures
//
// Harbor sends a JSON body like:
//
//  {
//    "type": "PUSH_ARTIFACT",
//    "occur_at": 1700000000,
//    "operator": "admin",
//    "event_data": {
//      "resources": [
//        {
//          "digest": "sha256:abc123",
//          "tag": "latest",
//          "resource_url": "registry.example.com/myproject/myimage:latest"
//        }
//      ],
//      "repository": {
//        "date_created": 1700000000,
//        "name": "myimage",
//        "namespace": "myproject",
//        "full_name": "myproject/myimage",
//        "type": "private"
//      }
//    }
//  }
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct HarborEvent {
    /// Event type, e.g. "PUSH_ARTIFACT", "DELETE_ARTIFACT".
    #[serde(rename = "type")]
    pub event_type: String,

    /// Unix timestamp.
    #[allow(dead_code)]
    pub occur_at: Option<i64>,

    /// User who triggered the event.
    pub operator: Option<String>,

    pub event_data: EventData,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EventData {
    pub resources: Vec<Resource>,
    pub repository: Repository,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Resource {
    /// Content-addressable digest.
    #[allow(dead_code)]
    pub digest: Option<String>,

    /// Image tag, e.g. "latest" or "v1.2.3".
    pub tag: Option<String>,

    /// Full resource URL, e.g. "registry.example.com/myproject/myimage:latest".
    #[allow(dead_code)]
    pub resource_url: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Repository {
    /// Short name, e.g. "myimage".
    pub name: String,

    /// Harbor project / namespace, e.g. "myproject".
    pub namespace: Option<String>,

    /// Fully-qualified name, e.g. "myproject/myimage".
    pub full_name: Option<String>,

    /// "public" | "private"
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub repo_type: Option<String>,
}

impl HarborEvent {
    /// Returns the repository full name, preferring `full_name` then
    /// constructing it from `namespace/name`.
    pub fn repository_full_name(&self) -> String {
        if let Some(ref full) = self.event_data.repository.full_name {
            if !full.is_empty() {
                return full.clone();
            }
        }
        match &self.event_data.repository.namespace {
            Some(ns) if !ns.is_empty() => {
                format!("{}/{}", ns, self.event_data.repository.name)
            }
            _ => self.event_data.repository.name.clone(),
        }
    }

    /// Returns all pushed tags.
    pub fn pushed_tags(&self) -> Vec<&str> {
        self.event_data
            .resources
            .iter()
            .filter_map(|r| r.tag.as_deref())
            .collect()
    }
}
