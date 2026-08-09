use rmcp::ErrorData;
use rmcp::model::PaginatedRequestParams;

pub(crate) const MCP_LIST_PAGE_SIZE: usize = 100;

pub(crate) struct PageCollector<T> {
    start: usize,
    expected_revision: Option<String>,
    bound_revision: Option<String>,
    seen: usize,
    page: Vec<T>,
    has_next: bool,
}

impl<T> PageCollector<T> {
    pub(crate) fn new(request: Option<PaginatedRequestParams>) -> Result<Self, ErrorData> {
        let (start, expected_revision) = match request.and_then(|request| request.cursor) {
            Some(cursor) => parse_cursor(&cursor)?,
            None => (0, None),
        };
        Ok(Self {
            start,
            expected_revision,
            bound_revision: None,
            seen: 0,
            page: Vec::with_capacity(MCP_LIST_PAGE_SIZE),
            has_next: false,
        })
    }

    /// Bind this page to the complete sorted contract that produced it.
    ///
    /// A subsequent page must present the same revision embedded in the cursor;
    /// otherwise an offset could resume against a different catalog and silently
    /// duplicate or omit entries.
    pub(crate) fn bind_revision(&mut self, revision: &str) -> Result<(), ErrorData> {
        if self.start > 0 && self.expected_revision.is_none() {
            return Err(invalid_cursor(
                "cursor must include the result-set revision",
            ));
        }
        if self
            .expected_revision
            .as_deref()
            .is_some_and(|expected| expected != revision)
        {
            return Err(invalid_cursor(
                "cursor was issued for a different result-set revision",
            ));
        }
        self.bound_revision = Some(revision.to_string());
        Ok(())
    }

    pub(crate) fn accept(&mut self, item: T) {
        if self.finished() {
            return;
        }
        if self.seen < self.start {
            self.seen += 1;
            return;
        }
        if self.page.len() < MCP_LIST_PAGE_SIZE {
            self.page.push(item);
            self.seen += 1;
            return;
        }
        self.has_next = true;
        self.seen += 1;
    }

    pub(crate) fn finished(&self) -> bool {
        self.has_next
    }

    pub(crate) fn finish(self) -> Result<(Vec<T>, Option<String>), ErrorData> {
        if self.seen < self.start {
            return Err(invalid_cursor("cursor is past the end of the result set"));
        }
        let next_cursor = self.has_next.then(|| {
            let offset = self.start + self.page.len();
            self.bound_revision.as_ref().map_or_else(
                || offset.to_string(),
                |revision| format!("v1:{offset}:{revision}"),
            )
        });
        Ok((self.page, next_cursor))
    }
}

#[cfg(test)]
fn try_collect_page<T, I>(
    items: I,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData>
where
    I: IntoIterator<Item = T>,
{
    let mut collector = PageCollector::new(request)?;
    for item in items {
        collector.accept(item);
        if collector.finished() {
            break;
        }
    }
    collector.finish()
}

#[cfg(test)]
fn paginate_items<T>(
    items: Vec<T>,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData> {
    let start = match request.as_ref().and_then(|request| request.cursor.as_ref()) {
        Some(cursor) => parse_cursor(cursor)?.0,
        None => 0,
    };
    if start > items.len() {
        return Err(invalid_cursor("cursor is past the end of the result set"));
    }
    try_collect_page(items, request)
}

pub(crate) fn error_kind(error: &ErrorData) -> &'static str {
    match error
        .data
        .as_ref()
        .and_then(|data| data.get("kind"))
        .and_then(serde_json::Value::as_str)
    {
        Some("invalid_cursor") => "invalid_cursor",
        _ => "invalid_params",
    }
}

fn parse_cursor(cursor: &str) -> Result<(usize, Option<String>), ErrorData> {
    if let Some(versioned) = cursor.strip_prefix("v1:") {
        let Some((offset, revision)) = versioned.split_once(':') else {
            return Err(invalid_cursor(
                "versioned cursor must contain an offset and revision",
            ));
        };
        if revision.is_empty() || revision.contains(':') {
            return Err(invalid_cursor("cursor revision is malformed"));
        }
        let offset = offset
            .parse::<usize>()
            .map_err(|_| invalid_cursor("cursor must contain a non-negative integer offset"))?;
        return Ok((offset, Some(revision.to_string())));
    }
    cursor
        .parse::<usize>()
        .map(|offset| (offset, None))
        .map_err(|_| invalid_cursor("cursor must be a non-negative integer offset"))
}

fn invalid_cursor(message: &str) -> ErrorData {
    ErrorData::invalid_params(
        message.to_string(),
        Some(serde_json::json!({
            "kind": "invalid_cursor",
            "message": message,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_collector_stops_after_page_plus_lookahead() {
        let mut collector = PageCollector::new(None).expect("collector");
        let mut visited = 0;

        for item in 0..250 {
            visited += 1;
            collector.accept(item);
            if collector.finished() {
                break;
            }
        }

        let (page, next_cursor) = collector.finish().expect("page");
        assert_eq!(visited, MCP_LIST_PAGE_SIZE + 1);
        assert_eq!(page, (0..MCP_LIST_PAGE_SIZE).collect::<Vec<_>>());
        assert_eq!(next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn page_collector_counts_skipped_items_without_storing_them() {
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("collector");
        let mut visited = 0;

        for item in 0..250 {
            visited += 1;
            collector.accept(item);
            if collector.finished() {
                break;
            }
        }

        let (page, next_cursor) = collector.finish().expect("page");
        assert_eq!(visited, 250);
        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }

    #[test]
    fn page_collector_rejects_cursor_past_end() {
        let request = PaginatedRequestParams::default().with_cursor(Some("4".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("collector");

        for item in 0..3 {
            collector.accept(item);
        }

        let err = collector.finish().expect_err("cursor past end");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn paginates_with_offset_cursor() {
        let items = (0..250).collect::<Vec<_>>();

        let (page, next_cursor) = paginate_items(items, None).expect("first page");

        assert_eq!(page.len(), MCP_LIST_PAGE_SIZE);
        assert_eq!(page[0], 0);
        assert_eq!(page[MCP_LIST_PAGE_SIZE - 1], 99);
        assert_eq!(next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn resumes_from_cursor() {
        let items = (0..250).collect::<Vec<_>>();
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));

        let (page, next_cursor) = paginate_items(items, Some(request)).expect("cursor page");

        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }

    #[test]
    fn rejects_invalid_cursor() {
        let request = PaginatedRequestParams::default().with_cursor(Some("nope".to_string()));

        let err = paginate_items(vec![1, 2, 3], Some(request)).expect_err("invalid cursor");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn revision_bound_cursor_rejects_a_changed_catalog() {
        let mut first = PageCollector::new(None).expect("first collector");
        first
            .bind_revision("catalog-a")
            .expect("bind first revision");
        for item in 0..250 {
            first.accept(item);
            if first.finished() {
                break;
            }
        }
        let (_, cursor) = first.finish().expect("first page");
        let request = PaginatedRequestParams::default().with_cursor(cursor);
        let mut rebuilt = PageCollector::<usize>::new(Some(request)).expect("resume collector");

        let error = rebuilt
            .bind_revision("catalog-b")
            .expect_err("stale cursor must be rejected");

        assert_eq!(
            error.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn revision_bound_pagination_rejects_an_unversioned_continuation_cursor() {
        let request = PaginatedRequestParams::default().with_cursor(Some("100".to_string()));
        let mut collector = PageCollector::<usize>::new(Some(request)).expect("parse cursor");

        let error = collector
            .bind_revision("catalog-a")
            .expect_err("unversioned continuation must be rejected");

        assert_eq!(
            error.data.as_ref().expect("error data")["kind"],
            serde_json::json!("invalid_cursor")
        );
    }

    #[test]
    fn unversioned_cursor_remains_valid_for_unbound_resource_and_prompt_pagination() {
        let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));
        let mut collector = PageCollector::new(Some(request)).expect("parse legacy cursor");
        for item in 0..250 {
            collector.accept(item);
        }

        let (page, next_cursor) = collector.finish().expect("finish unbound page");

        assert_eq!(page, (200..250).collect::<Vec<_>>());
        assert_eq!(next_cursor, None);
    }
}
