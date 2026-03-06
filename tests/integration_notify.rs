/// Integration tests for the Notifier (generic webhook, Slack, Gotify).
///
/// Uses `wiremock` to spin up a real local HTTP mock server and verify that
/// the Notifier sends correct payloads.
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

use updock::{
    config::NotificationConfig,
    notify::Notifier,
    watcher::SessionReport,
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn empty_report() -> SessionReport {
    SessionReport {
        scanned: 0,
        updated: vec![],
        failed: vec![],
        skipped: vec![],
        fresh: vec![],
    }
}

fn report_with_update(name: &str) -> SessionReport {
    SessionReport {
        scanned: 1,
        updated: vec![name.to_string()],
        failed: vec![],
        skipped: vec![],
        fresh: vec![],
    }
}

fn report_with_failure(name: &str) -> SessionReport {
    SessionReport {
        scanned: 1,
        updated: vec![],
        failed: vec![name.to_string()],
        skipped: vec![],
        fresh: vec![],
    }
}

// ── send_report: nothing sent when empty ─────────────────────────────────────

#[tokio::test]
async fn send_report_does_nothing_when_empty() {
    let mock_server = MockServer::start().await;

    // Any request to the mock server would indicate an unexpected notification.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0) // assert ZERO requests are made
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&empty_report()).await;

    mock_server.verify().await;
}

// ── generic webhook URL ───────────────────────────────────────────────────────

#[tokio::test]
async fn generic_webhook_receives_json_text_field_on_update() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("mycontainer")).await;

    mock_server.verify().await;

    // Inspect the body that was received.
    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("body must be JSON");
    let text = body["text"].as_str().expect("text field must be a string");
    assert!(
        text.contains("mycontainer"),
        "text should mention the container name; got: {text}"
    );
    assert!(
        text.contains("updated"),
        "text should mention 'updated'; got: {text}"
    );
}

#[tokio::test]
async fn generic_webhook_receives_json_text_field_on_failure() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_failure("badcontainer")).await;

    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let text = body["text"].as_str().unwrap();
    assert!(
        text.contains("badcontainer"),
        "text should mention the failed container; got: {text}"
    );
    assert!(
        text.contains("failed"),
        "text should mention 'failed'; got: {text}"
    );
}

#[tokio::test]
async fn generic_webhook_multiple_urls_all_receive_notification() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;

    for ms in [&server_a, &server_b] {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(ms)
            .await;
    }

    let cfg = NotificationConfig {
        urls: vec![server_a.uri(), server_b.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("app")).await;

    server_a.verify().await;
    server_b.verify().await;
}

#[tokio::test]
async fn generic_webhook_non_success_response_does_not_panic() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    // Should not panic even when the server returns 500.
    notifier.send_report(&report_with_update("app")).await;
    mock_server.verify().await;
}

// ── Slack webhook ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn slack_webhook_sends_text_and_username() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/slack"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        slack_webhook_url: Some(format!("{}/slack", mock_server.uri())),
        slack_username: Some("bot".to_string()),
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("webapp")).await;

    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["text"].as_str().unwrap().contains("webapp"));
    assert_eq!(body["username"].as_str(), Some("bot"));
}

#[tokio::test]
async fn slack_webhook_default_username_when_not_set() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        slack_webhook_url: Some(mock_server.uri()),
        slack_username: None,
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("webapp")).await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    // Default username is "updock".
    assert_eq!(body["username"].as_str(), Some("updock"));
}

#[tokio::test]
async fn slack_webhook_sends_channel_when_set() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        slack_webhook_url: Some(mock_server.uri()),
        slack_channel: Some("#ops".to_string()),
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("service")).await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["channel"].as_str(), Some("#ops"));
}

#[tokio::test]
async fn slack_webhook_no_channel_field_when_not_set() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        slack_webhook_url: Some(mock_server.uri()),
        slack_channel: None,
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("service")).await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    // No channel key should be present if not configured.
    assert!(
        body.get("channel").is_none(),
        "channel should not be in payload when not configured"
    );
}

// ── Gotify ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gotify_sends_message_with_token() {
    let mock_server = MockServer::start().await;

    // Gotify endpoint: /message?token=<token>
    Mock::given(method("POST"))
        .and(path("/message"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        gotify_url: Some(mock_server.uri()),
        gotify_token: Some("my_gotify_token".to_string()),
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("container")).await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    // Verify the token was passed as a query parameter.
    let url_str = requests[0].url.as_str();
    assert!(
        url_str.contains("token=my_gotify_token"),
        "Gotify URL should include token query param; got: {url_str}"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["title"].as_str(), Some("updock"));
    assert!(body["message"].as_str().unwrap().contains("container"));
    assert_eq!(body["priority"].as_u64(), Some(5));
}

#[tokio::test]
async fn gotify_not_sent_when_token_missing() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0) // should NOT be called
        .mount(&mock_server)
        .await;

    // gotify_url set but no token → should not send.
    let cfg = NotificationConfig {
        gotify_url: Some(mock_server.uri()),
        gotify_token: None,
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("container")).await;
    mock_server.verify().await;
}

#[tokio::test]
async fn gotify_not_sent_when_url_missing() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;

    // gotify_token set but no URL → should not send.
    let cfg = NotificationConfig {
        gotify_url: None,
        gotify_token: Some("tok".to_string()),
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("container")).await;
    mock_server.verify().await;
}

// ── send (direct message) ─────────────────────────────────────────────────────

#[tokio::test]
async fn send_direct_message_posts_to_generic_urls() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send("hello from updock").await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["text"].as_str(), Some("hello from updock"));
}

// ── format_report text content ────────────────────────────────────────────────

#[tokio::test]
async fn report_text_contains_counts() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let report = SessionReport {
        scanned: 5,
        updated: vec!["a".to_string(), "b".to_string()],
        failed: vec!["c".to_string()],
        skipped: vec!["d".to_string()],
        fresh: vec![],
    };

    let cfg = NotificationConfig {
        urls: vec![mock_server.uri()],
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report).await;
    mock_server.verify().await;

    let requests = mock_server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let text = body["text"].as_str().unwrap();

    assert!(text.contains("5 scanned") || text.contains("scanned"), "got: {text}");
    assert!(text.contains("2") || text.contains("updated"), "got: {text}");
    assert!(text.contains("1") || text.contains("failed"), "got: {text}");
    assert!(text.contains("a"), "should mention updated container a; got: {text}");
    assert!(text.contains("b"), "should mention updated container b; got: {text}");
    assert!(text.contains("c"), "should mention failed container c; got: {text}");
    assert!(text.contains("d"), "should mention skipped container d; got: {text}");
}

// ── all channels sent simultaneously ─────────────────────────────────────────

#[tokio::test]
async fn all_channels_receive_notification_in_parallel() {
    let generic_server = MockServer::start().await;
    let slack_server = MockServer::start().await;
    let gotify_server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&generic_server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&slack_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/message"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&gotify_server)
        .await;

    let cfg = NotificationConfig {
        urls: vec![generic_server.uri()],
        slack_webhook_url: Some(slack_server.uri()),
        gotify_url: Some(gotify_server.uri()),
        gotify_token: Some("tok".to_string()),
        ..Default::default()
    };
    let notifier = Notifier::new(cfg);
    notifier.send_report(&report_with_update("myapp")).await;

    generic_server.verify().await;
    slack_server.verify().await;
    gotify_server.verify().await;
}
