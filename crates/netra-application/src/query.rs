//! Read-side use cases: listing, inspection and statistics.

use netra_analysis::stats::ExchangeStats;
use netra_core::domain::{
    ExchangeFilter, ExchangeId, ExchangeQuery, ExchangeSummary, NetworkExchange, SessionId,
};

use crate::{ApplicationError, ApplicationResult, context::AppContext};

/// Default page size for the request list.
pub const DEFAULT_PAGE_SIZE: usize = 2_000;

/// Chunk size used when aggregating over a whole session.
const STATS_CHUNK: usize = 500;

/// Queries the TUI and the CLI share.
#[derive(Debug, Clone)]
pub struct QueryService {
    context: AppContext,
}

impl QueryService {
    /// Builds the service.
    #[must_use]
    pub fn new(context: AppContext) -> Self {
        Self { context }
    }

    /// Returns one page of summaries for a session.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn list(
        &self,
        session_id: SessionId,
        filter: &ExchangeFilter,
        limit: usize,
        offset: usize,
    ) -> ApplicationResult<Vec<ExchangeSummary>> {
        let query = ExchangeQuery {
            session_id,
            filter: filter.clone(),
            limit,
            offset,
            order: netra_core::domain::filter::ExchangeOrder::StartedDesc,
        };
        Ok(self.context.exchanges.page(&query).await?)
    }

    /// Returns the newest exchanges of a session with the default page size.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn recent(
        &self,
        session_id: SessionId,
        filter: &ExchangeFilter,
    ) -> ApplicationResult<Vec<ExchangeSummary>> {
        self.list(session_id, filter, DEFAULT_PAGE_SIZE, 0).await
    }

    /// Loads a complete exchange for the inspector.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::NotFound`] when the exchange does not exist.
    pub async fn inspect(&self, id: ExchangeId) -> ApplicationResult<NetworkExchange> {
        self.context
            .exchanges
            .get(id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound("exchange", id.to_string()))
    }

    /// Aggregates statistics over every exchange of a session, in bounded
    /// chunks so a large session is never loaded at once.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn stats(
        &self,
        session_id: SessionId,
        filter: &ExchangeFilter,
    ) -> ApplicationResult<ExchangeStats> {
        let mut offset = 0;
        let mut summaries: Vec<ExchangeSummary> = Vec::new();
        loop {
            let chunk = self.list(session_id, filter, STATS_CHUNK, offset).await?;
            let count = chunk.len();
            summaries.extend(chunk);
            if count < STATS_CHUNK {
                break;
            }
            offset += count;
            if offset > 1_000_000 {
                break;
            }
        }
        Ok(ExchangeStats::from_summaries(&summaries))
    }

    /// Distinct hosts of a session with request counts.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn hosts(&self, session_id: SessionId) -> ApplicationResult<Vec<(String, u64)>> {
        Ok(self.context.exchanges.hosts(session_id).await?)
    }

    /// Exchange counters of a session.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Storage`] on query failure.
    pub async fn counts(
        &self,
        session_id: SessionId,
    ) -> ApplicationResult<netra_core::ports::ExchangeCounts> {
        Ok(self.context.exchanges.counts(session_id).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netra_core::{
        domain::{
            Body, CaptureSession, CaptureSource, ContentType, HttpHeaders, HttpMethod, MediaKind,
            NetworkExchange, NetworkRequest, NetworkResponse, RequestUrl, SourceSelection,
            StatusCode, Timestamp,
        },
        ports::SystemClock,
    };
    use netra_storage::SqliteStore;

    use super::*;

    fn context() -> (AppContext, AppContext) {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let context = AppContext {
            sessions: store.clone(),
            exchanges: store,
            clock: Arc::new(SystemClock),
            config: netra_core::config::NetraConfig::with_default_data_dir(),
        };
        (context.clone(), context)
    }

    fn exchange(session: &CaptureSession, url: &str, status: u16) -> NetworkExchange {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new(url),
            Timestamp::from_unix_millis(1_000),
        );
        let mut exchange = NetworkExchange::pending(
            session.id,
            CaptureSource::Cdp,
            request,
            Some(url.to_owned()),
        );
        let mut response = NetworkResponse::new(
            StatusCode::try_new(status).unwrap(),
            Timestamp::from_unix_millis(1_100),
        );
        response.headers = HttpHeaders::from_pairs([("Content-Type", "application/json")]);
        response.body = Body::captured(
            Some(ContentType::parse("application/json")),
            br#"{"ok":true}"#.to_vec(),
        );
        exchange.complete(response);
        exchange
    }

    #[tokio::test]
    async fn lists_filter_and_inspect_a_session() {
        let context = context().0;
        let session =
            CaptureSession::new("api", SourceSelection::Cdp, Timestamp::from_unix_millis(0))
                .unwrap();
        let sessions = crate::SessionService::new(context.clone());
        sessions.create("api", SourceSelection::Cdp).await.unwrap();
        let listed = sessions.list().await.unwrap();
        let session_id = listed[0].id;

        let mut first = exchange(&session, "https://api.test/a", 200);
        first.session_id = session_id;
        let mut second = exchange(&session, "https://api.test/b", 500);
        second.session_id = session_id;
        context.exchanges.upsert(&first).await.unwrap();
        context.exchanges.upsert(&second).await.unwrap();

        let queries = QueryService::new(context.clone());
        let all = queries
            .recent(session_id, &ExchangeFilter::default())
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].media_kind, MediaKind::Json);

        let filter = ExchangeFilter {
            status: netra_core::domain::StatusRange::parse("5xx").unwrap(),
            ..ExchangeFilter::default()
        };
        let failing = queries.recent(session_id, &filter).await.unwrap();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].status.unwrap().as_u16(), 500);

        let inspected = queries.inspect(failing[0].id).await.unwrap();
        assert_eq!(
            inspected.response.unwrap().headers.get("content-type"),
            Some("application/json")
        );

        let stats = queries
            .stats(session_id, &ExchangeFilter::default())
            .await
            .unwrap();
        assert_eq!(stats.total, 2);
        assert_eq!(
            queries.hosts(session_id).await.unwrap(),
            vec![("api.test".to_owned(), 2)]
        );
    }

    #[tokio::test]
    async fn inspecting_an_unknown_exchange_is_an_explicit_error() {
        let queries = QueryService::new(context().0);
        assert!(matches!(
            queries.inspect(ExchangeId::new()).await,
            Err(ApplicationError::NotFound(..))
        ));
    }
}
