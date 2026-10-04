use super::*;

pub(super) fn evaluate(
    name: &str,
    mut raw: RawReport,
    fixture: &SnippetFixture,
    elapsed: u64,
    escaped: bool,
) -> Result<SnippetFixtureReport, ToolError> {
    let validation_started = Instant::now();
    let mut schema_budget = super::super::schemas::validation_budget();
    let mut failures = Vec::new();
    for call in &raw.contract_calls {
        if elapsed.saturating_add(
            u64::try_from(validation_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ) >= fixture.budgets.wall_clock_ms
        {
            failures.push("argument validation deadline exceeded".into());
            break;
        }
        if let Some(schema) = fixture
            .schemas
            .get(&call.tool)
            .and_then(|s| s.input_schema.as_ref())
            && super::super::schemas::validate_value_with_budget(
                &call.params,
                schema,
                &mut schema_budget,
            )
            .is_err()
        {
            failures.push(if schema_budget.is_exhausted() {
                "argument schema validation work budget exceeded".into()
            } else {
                format!(
                    "tool arguments violate saved input schema for {}",
                    call.tool
                )
            });
            break;
        }
    }
    if let Some(exception) = raw.exception {
        failures.push(format!("snippet exception: {exception}"));
    }
    if escaped {
        failures.push("snippet attempted to use the real host bridge".into());
    }
    if raw.unexpected > 0 {
        failures.push(format!(
            "{} unexpected or over-budget calls",
            raw.unexpected
        ));
    }
    if !raw.unused.is_empty() {
        failures.push(format!(
            "{} fixture rules were not fully consumed",
            raw.unused.len()
        ));
    }
    if raw.result.get("ok").and_then(Value::as_bool) == Some(false)
        && fixture.expect.get("/ok") != Some(&Value::Bool(false))
    {
        failures.push("snippet returned ok: false".into());
    }
    let bytes = serde_json::to_vec(&raw.result)
        .map_err(|e| invalid(e.to_string()))?
        .len();
    if bytes > fixture.budgets.output_bytes {
        failures.push("output_bytes budget exceeded".into());
    }
    let elapsed = elapsed.saturating_add(
        u64::try_from(validation_started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    if elapsed > fixture.budgets.wall_clock_ms {
        failures.push("wall_clock_ms budget exceeded".into());
    }
    if raw.attempted > fixture.budgets.tool_calls {
        failures.push("tool_calls budget exceeded".into());
    }
    for (pointer, expected) in &fixture.expect {
        if raw.result.pointer(pointer) != Some(expected) {
            failures.push(format!("assertion failed at {pointer}"));
        }
    }
    for pointer in &fixture.absent {
        if raw.result.pointer(pointer).is_some() {
            failures.push(format!("expected absent path at {pointer}"));
        }
    }
    if let Some(snapshot) = &fixture.snapshot {
        let mut expected = snapshot.clone();
        let mut actual = raw.result.clone();
        for pointer in &fixture.ignore_paths {
            if let Some(v) = expected.pointer_mut(pointer) {
                *v = Value::Null;
            }
            if let Some(v) = actual.pointer_mut(pointer) {
                *v = Value::Null;
            }
        }
        if actual != expected {
            failures.push("normalized snapshot mismatch".into());
        }
    }
    let trace_truncated = raw.calls.len() > 32;
    raw.calls.truncate(32);
    Ok(SnippetFixtureReport {
        name: name.into(),
        mode: "mock".into(),
        passed: failures.is_empty(),
        failures,
        metrics: FixtureMetrics {
            wall_clock_ms: elapsed,
            tool_calls: raw.attempted,
            output_bytes: bytes,
            estimated_tokens: bytes.div_ceil(4),
            max_in_flight: raw.max_in_flight,
        },
        calls: raw.calls,
        trace_truncated,
        result: (bytes <= fixture.budgets.output_bytes).then_some(raw.result),
    })
}
