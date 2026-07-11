use rmcp::ErrorData;
use rmcp::model::PaginatedRequestParams;

pub(crate) const MCP_LIST_PAGE_SIZE: usize = 100;

pub(crate) fn paginate_items<T>(
    items: Vec<T>,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData> {
    let start = match request.and_then(|request| request.cursor) {
        Some(cursor) => parse_cursor(&cursor)?,
        None => 0,
    };
    if start > items.len() {
        return Err(invalid_cursor("cursor is past the end of the result set"));
    }

    let total = items.len();
    let page = items
        .into_iter()
        .skip(start)
        .take(MCP_LIST_PAGE_SIZE)
        .collect::<Vec<_>>();
    let next = start + page.len();
    let next_cursor = (next < total).then(|| next.to_string());
    Ok((page, next_cursor))
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

fn parse_cursor(cursor: &str) -> Result<usize, ErrorData> {
    cursor
        .parse::<usize>()
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
}
