//! Persistence acceptance tests: CRUD, integrity, incremental writes, restart
//! recovery, filtering, paging and retention.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use netra_core::{
    domain::{
        Body, BodyData, BodyDirection, CaptureSession, CaptureSource, ContentType, DurationMs,
        ExchangeFilter, ExchangeId, ExchangeQuery, ExchangeState, ExchangeUpdate, HttpHeaders,
        HttpMethod, MediaKind, NetworkExchange, NetworkRequest, NetworkResponse, OmitReason,
        RequestUrl, SessionId, SessionStatus, SourceSelection, StatusCode, Timestamp,
        TimingMetrics, filter::ExchangeOrder,
    },
    error::StorageError,
    ports::{ExchangeRepository, SessionRepository},
};
use netra_storage::SqliteStore;

async fn session(store: &SqliteStore) -> CaptureSession {
    let session = CaptureSession::new(
        "integration",
        SourceSelection::Both,
        Timestamp::from_unix_millis(1_000),
    )
    .unwrap();
    store.create(&session).await.unwrap();
    session
}

async fn load_session(store: &SqliteStore, id: SessionId) -> Option<CaptureSession> {
    SessionRepository::get(store, id).await.unwrap()
}

async fn load_exchange(store: &SqliteStore, id: ExchangeId) -> Option<NetworkExchange> {
    ExchangeRepository::get(store, id).await.unwrap()
}

fn request(url: &str, method: &str, at: i64) -> NetworkRequest {
    let mut request = NetworkRequest::new(
        HttpMethod::parse(method).unwrap(),
        RequestUrl::new(url),
        Timestamp::from_unix_millis(at),
    );
    request.headers = HttpHeaders::from_pairs([
        ("Accept", "application/json"),
        ("Set-Cookie", "a=1"),
        ("set-cookie", "b=2"),
        ("Authorization", "[REDACTED]"),
    ]);
    request
}

fn completed(session: &CaptureSession, url: &str, status: u16, at: i64) -> NetworkExchange {
    let mut exchange = NetworkExchange::pending(
        session.id,
        CaptureSource::Cdp,
        request(url, "GET", at),
        Some(url.to_owned()),
    );
    let mut response = NetworkResponse::new(
        StatusCode::try_new(status).unwrap(),
        Timestamp::from_unix_millis(at + 90),
    );
    response.headers = HttpHeaders::from_pairs([("Content-Type", "application/json")]);
    response.body = Body::captured(
        Some(ContentType::parse("application/json")),
        br#"{"ok":true}"#.to_vec(),
    );
    exchange.complete(response);
    exchange.set_timing(TimingMetrics {
        dns: Some(DurationMs::from_millis(3)),
        ttfb: Some(DurationMs::from_millis(60)),
        total: Some(DurationMs::from_millis(90)),
        ..TimingMetrics::default()
    });
    exchange
}

#[tokio::test]
async fn session_rows_round_trip_with_counters() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let session = CaptureSession::new(
        "api",
        SourceSelection::Cdp,
        Timestamp::from_unix_millis(1_000),
    )
    .unwrap();
    store.create(&session).await.unwrap();

    let exchange = completed(&session, "https://api.test/a", 200, 1_100);
    store.upsert(&exchange).await.unwrap();
    let mut failed = completed(&session, "https://api.test/b", 500, 1_200);
    failed.fail("boom");
    store.upsert(&failed).await.unwrap();

    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].exchange_count, 2);
    assert_eq!(listed[0].error_count, 1);
    assert_eq!(listed[0].source, SourceSelection::Cdp);

    let loaded = load_session(&store, session.id).await.unwrap();
    assert_eq!(loaded.name, "api");
    assert_eq!(loaded.status, SessionStatus::Capturing);

    let mut updated = loaded.clone();
    updated.rename("api renamed").unwrap();
    updated.stop(Timestamp::from_unix_millis(9_000));
    updated.mark_saved(Timestamp::from_unix_millis(9_500));
    store.update(&updated).await.unwrap();
    let reloaded = load_session(&store, session.id).await.unwrap();
    assert_eq!(reloaded.name, "api renamed");
    assert_eq!(reloaded.status, SessionStatus::Stopped);
    assert!(reloaded.saved_at.is_some());

    store.delete(session.id).await.unwrap();
    assert!(load_session(&store, session.id).await.is_none());
    assert!(matches!(
        store.update(&updated).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn exchange_round_trip_keeps_headers_bodies_and_timings() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    let exchange = completed(&session, "https://api.test/a", 201, 1_100);
    store.upsert(&exchange).await.unwrap();

    let loaded = load_exchange(&store, exchange.id).await.unwrap();
    assert_eq!(loaded, exchange);
    assert_eq!(loaded.request.headers.get_all("set-cookie").count(), 2);
    assert_eq!(loaded.request.headers.get("set-cookie"), Some("a=1"));
    assert_eq!(loaded.request.headers.iter().next().unwrap().name, "Accept");
    assert_eq!(loaded.response.as_ref().unwrap().status.as_u16(), 201);
    assert_eq!(
        loaded.timing.unwrap().total,
        Some(DurationMs::from_millis(90))
    );
    assert_eq!(loaded.media_kind(), MediaKind::Json);
}

#[tokio::test]
async fn bodies_keep_their_state() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;

    let mut exchange = completed(&session, "https://api.test/a", 200, 1_100);
    exchange.request.body = Body::redacted(Some(ContentType::parse("application/json")), 42);
    let response = exchange.response.as_mut().unwrap();
    response.body = Body::omitted(
        Some(ContentType::parse("audio/mpeg")),
        OmitReason::TooLarge,
        5_000_000,
    );
    store.upsert(&exchange).await.unwrap();

    let loaded = load_exchange(&store, exchange.id).await.unwrap();
    assert!(matches!(loaded.request.body.data, BodyData::Redacted));
    assert_eq!(loaded.request.body.size_bytes, 42);
    match loaded.response.unwrap().body.data {
        BodyData::Omitted { reason } => assert_eq!(reason, OmitReason::TooLarge),
        other => panic!("expected omitted body, got {other:?}"),
    }

    let mut truncated = completed(&session, "https://api.test/b", 200, 1_300);
    let response = truncated.response.as_mut().unwrap();
    response.body =
        Body::captured(Some(ContentType::parse("text/plain")), b"0123".to_vec()).with_size(4096);
    response.body.truncated = true;
    store.upsert(&truncated).await.unwrap();
    let loaded = load_exchange(&store, truncated.id).await.unwrap();
    let body = &loaded.response.unwrap().body;
    assert!(body.truncated);
    assert_eq!(body.bytes().unwrap(), b"0123");
    assert_eq!(body.size_bytes, 4096);
}

#[tokio::test]
async fn deleting_a_session_cascades_to_children() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    let exchange = completed(&session, "https://api.test/a", 200, 1_100);
    store.upsert(&exchange).await.unwrap();
    store.delete(session.id).await.unwrap();
    assert!(load_exchange(&store, exchange.id).await.is_none());
    assert_eq!(store.counts(session.id).await.unwrap().total, 0);
    assert!(store.hosts(session.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn orphan_exchanges_violate_the_foreign_key() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = CaptureSession::new("ghost", SourceSelection::Cdp, Timestamp::now()).unwrap();
    let mut exchange = completed(&session, "https://api.test/a", 200, 1_100);
    exchange.session_id = session.id;
    assert!(matches!(
        store.upsert(&exchange).await,
        Err(StorageError::Integrity(_))
    ));
}

#[tokio::test]
async fn incremental_updates_build_a_complete_exchange() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    let request = request("https://api.test/inc", "POST", 1_100);
    let exchange = NetworkExchange::pending(
        session.id,
        CaptureSource::Proxy,
        request,
        Some("7".to_owned()),
    );
    store
        .apply(&ExchangeUpdate::Discovered(Box::new(exchange.clone())))
        .await
        .unwrap();
    let pending = load_exchange(&store, exchange.id).await.unwrap();
    assert_eq!(pending.state, ExchangeState::Pending);
    assert!(pending.response.is_none());

    let mut response = NetworkResponse::new(
        StatusCode::try_new(202).unwrap(),
        Timestamp::from_unix_millis(1_250),
    );
    response.headers = HttpHeaders::from_pairs([("Content-Type", "text/plain")]);
    store
        .apply(&ExchangeUpdate::Response {
            id: exchange.id,
            response: Box::new(response.clone()),
        })
        .await
        .unwrap();
    store
        .apply(&ExchangeUpdate::Body {
            id: exchange.id,
            direction: BodyDirection::Response,
            body: Box::new(Body::captured(
                Some(ContentType::parse("text/plain")),
                b"accepted".to_vec(),
            )),
        })
        .await
        .unwrap();
    store
        .apply(&ExchangeUpdate::Timing {
            id: exchange.id,
            timing: TimingMetrics {
                total: Some(DurationMs::from_millis(150)),
                ttfb: Some(DurationMs::from_millis(120)),
                ..TimingMetrics::default()
            },
        })
        .await
        .unwrap();
    store
        .apply(&ExchangeUpdate::State {
            id: exchange.id,
            state: ExchangeState::Completed,
            error: None,
        })
        .await
        .unwrap();

    let loaded = load_exchange(&store, exchange.id).await.unwrap();
    assert_eq!(loaded.state, ExchangeState::Completed);
    assert_eq!(loaded.status().unwrap().as_u16(), 202);
    assert_eq!(loaded.duration(), Some(DurationMs::from_millis(150)));
    assert_eq!(
        loaded.timing.unwrap().ttfb,
        Some(DurationMs::from_millis(120))
    );
    assert_eq!(
        loaded.response.unwrap().body.text().as_deref(),
        Some("accepted")
    );

    let updates = store
        .page(&ExchangeQuery::newest(session.id, 10))
        .await
        .unwrap();
    assert_eq!(updates[0].size_bytes, 8);
    assert_eq!(updates[0].media_kind, MediaKind::Text);
}

#[tokio::test]
async fn redirect_closes_the_previous_exchange() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    let first = NetworkExchange::pending(
        session.id,
        CaptureSource::Cdp,
        request("http://api.test/old", "GET", 1_000),
        Some("1".to_owned()),
    );
    store
        .apply(&ExchangeUpdate::Discovered(Box::new(first.clone())))
        .await
        .unwrap();
    let next = NetworkExchange::pending(
        session.id,
        CaptureSource::Cdp,
        request("https://api.test/new", "GET", 1_050),
        Some("1".to_owned()),
    );
    let redirect = NetworkResponse::new(
        StatusCode::try_new(301).unwrap(),
        Timestamp::from_unix_millis(1_040),
    );
    store
        .apply(&ExchangeUpdate::Redirected {
            previous_id: first.id,
            previous: Box::new(redirect),
            next: Box::new(next.clone()),
        })
        .await
        .unwrap();

    let closed = load_exchange(&store, first.id).await.unwrap();
    assert_eq!(closed.state, ExchangeState::Completed);
    assert_eq!(closed.status().unwrap().as_u16(), 301);
    let opened = load_exchange(&store, next.id).await.unwrap();
    assert_eq!(opened.state, ExchangeState::Pending);
    assert_eq!(opened.request.url.as_str(), "https://api.test/new");
}

#[tokio::test]
async fn filters_order_and_page_the_list() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    for (index, url) in [
        "https://api.test/one",
        "https://api.test/two",
        "https://other.test/three",
        "https://api.test/broken",
    ]
    .iter()
    .enumerate()
    {
        let mut exchange = completed(&session, url, 200, 1_100 + index as i64);
        if url.ends_with("broken") {
            exchange.fail("net::ERR_CONNECTION_REFUSED");
        }
        store.upsert(&exchange).await.unwrap();
    }

    let all = store
        .page(&ExchangeQuery::newest(session.id, 10))
        .await
        .unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].url.as_str(), "https://api.test/broken");

    let mut host_filtered = ExchangeQuery::newest(session.id, 10);
    host_filtered.filter.host = Some("api.test".to_owned());
    assert_eq!(store.page(&host_filtered).await.unwrap().len(), 3);

    let mut failed = ExchangeQuery::newest(session.id, 10);
    failed.filter.only_failed = true;
    let only_failed = store.page(&failed).await.unwrap();
    assert_eq!(only_failed.len(), 1);
    assert_eq!(only_failed[0].state, ExchangeState::Failed);

    let mut text = ExchangeQuery::newest(session.id, 10);
    text.filter.text = Some("BROKEN".to_owned());
    assert_eq!(store.page(&text).await.unwrap().len(), 1);

    let mut status = ExchangeQuery::newest(session.id, 10);
    status.filter.status = netra_core::domain::StatusRange::parse("2xx").unwrap();
    assert_eq!(store.page(&status).await.unwrap().len(), 4);

    let mut limit = status.clone();
    limit.filter = ExchangeFilter::default();
    limit.limit = 2;
    limit.order = ExchangeOrder::StartedAsc;
    let page = store.page(&limit).await.unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].url.as_str(), "https://api.test/one");

    let hosts = store.hosts(session.id).await.unwrap();
    assert_eq!(hosts[0], ("api.test".to_owned(), 3));
}

#[tokio::test]
async fn retention_keeps_the_newest_exchanges() {
    let store = SqliteStore::open_in_memory().unwrap();
    let session = session(&store).await;
    for index in 0..6 {
        let exchange = completed(
            &session,
            &format!("https://api.test/{index}"),
            200,
            1_000 + index,
        );
        store.upsert(&exchange).await.unwrap();
    }
    let deleted = store.enforce_retention(session.id, 2).await.unwrap();
    assert_eq!(deleted, 4);
    let rows = store
        .page(&ExchangeQuery::newest(session.id, 10))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].url.as_str(), "https://api.test/5");
}

#[tokio::test]
async fn interrupted_sessions_are_recovered_with_a_finish_instant() {
    let store = SqliteStore::open_in_memory().unwrap();
    let active =
        CaptureSession::new("live", SourceSelection::Cdp, Timestamp::from_unix_millis(1)).unwrap();
    let paused = CaptureSession::new(
        "paused",
        SourceSelection::Cdp,
        Timestamp::from_unix_millis(2),
    )
    .unwrap();
    store.create(&active).await.unwrap();
    store.create(&paused).await.unwrap();
    let mut paused = paused;
    paused.pause();
    store.update(&paused).await.unwrap();
    let stopped = {
        let mut session = active.clone();
        session.id = SessionId::new();
        session.stop(Timestamp::from_unix_millis(3));
        store.create(&session).await.unwrap();
        session
    };

    let recovered = store
        .recover_interrupted(Timestamp::from_unix_millis(9_000))
        .await
        .unwrap();
    assert!(recovered.contains(&active.id));
    assert!(recovered.contains(&paused.id));
    assert!(!recovered.contains(&stopped.id));

    for id in [active.id, paused.id] {
        let session = load_session(&store, id).await.unwrap();
        assert_eq!(session.status, SessionStatus::Stopped);
        assert_eq!(
            session.finished_at,
            Some(Timestamp::from_unix_millis(9_000))
        );
    }
    assert!(
        store
            .recover_interrupted(Timestamp::now())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn data_survives_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("netra.db");
    let session_id: SessionId;
    let exchange_id: ExchangeId;
    {
        let store = SqliteStore::open(&path).unwrap();
        let session = CaptureSession::new(
            "restart",
            SourceSelection::Proxy,
            Timestamp::from_unix_millis(1),
        )
        .unwrap();
        store.create(&session).await.unwrap();
        let exchange = completed(&session, "https://api.test/persisted", 200, 1_000);
        store.upsert(&exchange).await.unwrap();
        session_id = session.id;
        exchange_id = exchange.id;
    }
    let reopened = SqliteStore::open(&path).unwrap();
    let session = load_session(&reopened, session_id).await.unwrap();
    assert_eq!(session.name, "restart");
    let exchange = load_exchange(&reopened, exchange_id).await.unwrap();
    assert_eq!(exchange.response.unwrap().status.as_u16(), 200);
    assert_eq!(
        load_exchange(&reopened, exchange_id)
            .await
            .unwrap()
            .request
            .headers
            .get("accept"),
        Some("application/json")
    );
}

#[tokio::test]
async fn missing_rows_produce_explicit_errors() {
    let store = SqliteStore::open_in_memory().unwrap();
    assert!(load_session(&store, SessionId::new()).await.is_none());
    assert!(load_exchange(&store, ExchangeId::new()).await.is_none());
    assert!(matches!(
        store.delete(SessionId::new()).await,
        Err(StorageError::NotFound(_))
    ));
    assert!(matches!(
        store
            .apply(&ExchangeUpdate::State {
                id: ExchangeId::new(),
                state: ExchangeState::Failed,
                error: Some("ghost".to_owned()),
            })
            .await,
        Err(StorageError::NotFound(_))
    ));
}
