use super::*;

fn draft() -> Draft {
    let seed = OpenContext::shelf(&egui::Context::default());
    Draft {
        version: 1,
        endpoint: transport::DEFAULT_ENDPOINT.into(),
        retry_until: None,
        payload: Payload {
            request_id: uuid::Uuid::new_v4().to_string(),
            kind: Kind::Bug,
            description: "统一版式下出现重复的表格背景。".into(),
            context: seed.context,
        },
    }
}

#[test]
fn character_validation_matches_the_service() {
    assert!(!valid_description("一二三四五六七八九"));
    assert!(valid_description("一 二 三 四 五 六 七 八 九 十"));
    assert_eq!(character_count(&"e\u{301}".repeat(10)), 10);
    assert_eq!(character_count(&"👨‍👩‍👧‍👦".repeat(9)), 9);
    assert!(!valid_description(&"\u{200b}".repeat(10)));
    assert!(!valid_description(&"字".repeat(5001)));
    assert!(!valid_description(&"👨‍👩‍👧‍👦".repeat(1000)));
}

#[test]
fn retry_freezes_the_exact_request_and_disallows_parallel_submission() {
    let original = draft();
    let mut feature = FeedbackFeature {
        draft: Some(original.clone()),
        ..Default::default()
    };
    feature.send();
    let first = feature.task.take_pending().unwrap();
    let Work::Send(sent) = &first.payload else {
        panic!()
    };
    assert_ne!(sent.payload.request_id, original.payload.request_id);
    feature.send();
    assert!(feature.task.take_pending().is_none());
    feature.complete(TaskResult {
        id: first.id,
        result: Ok(Output::Sent(
            sent.clone(),
            Outcome::Failed(Failure::Network, None),
        )),
    });
    feature.send();
    let second = feature.task.take_pending().unwrap();
    let Work::Send(retry) = second.payload else {
        panic!()
    };
    assert_eq!(
        serde_json::to_value(&sent.payload).unwrap(),
        serde_json::to_value(&retry.payload).unwrap()
    );
    assert_eq!(sent.endpoint, retry.endpoint);
}

#[test]
fn stale_completion_cannot_replace_an_active_draft_or_reopen_the_dialog() {
    let mut feature = FeedbackFeature {
        draft: Some(draft()),
        ..Default::default()
    };
    feature.send();
    let pending = feature.task.take_pending().unwrap();
    feature.complete(TaskResult {
        id: pending.id + 1,
        result: Err("storage".into()),
    });
    assert!(feature.task.is_pending());
    assert!(!feature.is_open());
    assert!(feature.locked);
}

#[test]
fn api_responses_must_match_the_request_and_expected_github_repository() {
    let id = "report-id";
    let response = serde_json::json!({"requestId":id,"status":"submitted","issue":{"number":123,"url":"https://github.com/TortoTech/torto/issues/123"}});
    let bytes = serde_json::to_vec(&response).unwrap();
    assert!(matches!(
        transport::parse_response(201, &bytes, id, None),
        Outcome::Submitted(_)
    ));
    assert!(matches!(
        transport::parse_response(201, &bytes, "other", None),
        Outcome::Failed(Failure::Protocol, _)
    ));
    let mut wrong = response;
    wrong["issue"]["url"] = "https://evil.example".into();
    assert!(matches!(
        transport::parse_response(201, &serde_json::to_vec(&wrong).unwrap(), id, None),
        Outcome::Failed(Failure::Protocol, _)
    ));
    let pending =
        serde_json::to_vec(&serde_json::json!({"requestId":id,"status":"pending"})).unwrap();
    assert!(
        matches!(transport::parse_response(202, &pending, id, Some(Duration::from_secs(60))), Outcome::Pending(wait) if wait.as_secs()==60)
    );
    assert!(
        matches!(transport::parse_response(429, b"{}", id, Some(Duration::from_secs(120))), Outcome::Failed(Failure::RateLimited, Some(wait)) if wait.as_secs()==120)
    );
}

#[test]
fn endpoint_and_metadata_never_include_credentials_or_absolute_book_paths() {
    assert!(transport::endpoint(transport::DEFAULT_ENDPOINT).is_ok());
    assert!(transport::endpoint("http://[::1]/api/feedback").is_ok());
    for url in [
        "http://public.example/api/feedback",
        "https://user:secret@example.com/api/feedback",
        "https://example.com/api/feedback?token=secret",
    ] {
        assert!(transport::endpoint(url).is_err());
    }
    for path in [
        "C:\\Users\\Chris\\book.epub",
        "/Users/chris/book.epub",
        "../private",
        "https://private.example",
    ] {
        assert!(book_document(path).is_none());
    }
    let value = serde_json::to_value(draft().payload.context).unwrap();
    assert!(value.get("book").is_none());
    assert!(value.get("location").is_none());
    assert!(value.get("reader").is_none());
    let mut stored = serde_json::to_value(draft()).unwrap();
    stored["payload"]["context"]["apiKey"] = "secret".into();
    assert!(serde_json::from_value::<Draft>(stored).is_err());
}

#[test]
fn persisted_submission_survives_restart_without_new_context_or_request_id() {
    let original = draft();
    let folder = std::env::temp_dir().join(format!("torto-feedback-{}", uuid::Uuid::new_v4()));
    let path = folder.join("draft.json");
    save_draft(&path, &original).unwrap();
    let restored: Draft = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&original.payload).unwrap(),
        serde_json::to_value(&restored.payload).unwrap()
    );
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn closing_and_reopening_preserves_a_pending_report_and_its_cooldown() {
    let mut feature = FeedbackFeature {
        draft: Some(draft()),
        ..Default::default()
    };
    feature.send();
    let pending = feature.task.take_pending().unwrap();
    let Work::Send(mut sent) = pending.payload else {
        panic!()
    };
    sent.retry_until = Some(now_seconds() + 120);
    feature.visible = false;
    feature.complete(TaskResult {
        id: pending.id,
        result: Ok(Output::Sent(
            sent.clone(),
            Outcome::Pending(Duration::from_secs(120)),
        )),
    });
    assert!(!feature.is_open());
    let mut new_context = OpenContext::shelf(&egui::Context::default());
    new_context.context.app.version = "changed".into();
    feature.open(new_context);
    assert!(feature.is_open());
    assert_eq!(
        feature.draft.as_ref().unwrap().payload.request_id,
        sent.payload.request_id
    );
    assert_ne!(
        feature.draft.as_ref().unwrap().payload.context.app.version,
        "changed"
    );
    feature.send();
    assert!(feature.task.take_pending().is_none());
    feature.retry_at = Some(Instant::now() - Duration::from_secs(1));
    feature.send();
    assert!(feature.task.take_pending().is_some());
}

#[test]
fn http_transport_sends_only_the_report_and_honors_retry_after() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut report = draft();
    report.endpoint = format!("http://{address}/api/feedback");
    let expected = serde_json::to_value(&report.payload).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("feedback connection: {e}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let (headers, body) = loop {
            let mut chunk = [0; 4096];
            let size = socket.read(&mut chunk).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
            assert!(request.len() < 70_000);
            if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break (headers, request[end + 4..end + 4 + length].to_vec());
                }
            }
        };
        assert!(headers.starts_with("post /api/feedback http/1.1"));
        assert!(headers.contains("content-type: application/json"));
        for header in ["authorization:", "cookie:", "origin:"] {
            assert!(!headers.lines().any(|line| line.starts_with(header)));
        }
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            expected
        );
        socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\nRetry-After: 60\r\nConnection: close\r\n\r\n{}").unwrap();
    });
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(transport::submit(&report));
    server.join().unwrap();
    assert!(
        matches!(result, Outcome::Failed(Failure::RateLimited, Some(wait)) if wait.as_secs() == 60)
    );
}

#[test]
fn reopening_an_unsent_draft_refreshes_context_but_preserves_the_description() {
    let original = draft();
    let mut feature = FeedbackFeature {
        loaded: true,
        draft: Some(original.clone()),
        ..Default::default()
    };
    let mut context = OpenContext::shelf(&egui::Context::default());
    context.context.app.version = "new-context".into();
    feature.open(context.clone());
    let pending = feature.task.take_pending().unwrap();
    let Work::Prepare(seed, restore) = pending.payload else {
        panic!()
    };
    assert!(!restore);
    assert_eq!(seed.context.app.version, "new-context");
    let mut prepared = original.clone();
    prepared.payload.description.clear();
    prepared.payload.context = context.context;
    feature.complete(TaskResult {
        id: pending.id,
        result: Ok(Output::Ready(prepared, false)),
    });
    let ready = feature.draft.unwrap();
    assert_eq!(ready.payload.description, original.payload.description);
    assert_eq!(ready.payload.context.app.version, "new-context");
    assert!(!feature.locked);
}
