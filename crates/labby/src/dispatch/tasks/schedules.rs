//! Durable recurring task delegation. Missed runs coalesce into one occurrence;
//! claim recovery reuses the same immutable Task ID. No authentication secrets persist.
use super::*;
use crate::access::task_schedule::{RetryPolicy, ScheduleOccurrence, TaskSchedule};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ScheduleSpec {
    Once {
        at: i64,
    },
    Interval {
        every_ms: i64,
    },
    Cron {
        expression: String,
        timezone: String,
    },
    Daily {
        hour: u8,
        minute: u8,
        timezone: String,
    },
    Weekly {
        weekdays: Vec<u8>,
        hour: u8,
        minute: u8,
        timezone: String,
    },
}
impl ScheduleSpec {
    pub(crate) fn next_after(&self, now: i64) -> Result<Option<i64>, ToolError> {
        match self {
            Self::Once { at } => {
                if *at < 0 {
                    return Err(invalid("schedule.at"));
                }
                Ok((*at > now).then_some(*at))
            }
            Self::Interval { every_ms } => {
                if !(60_000..=366 * 24 * 60 * 60 * 1000).contains(every_ms) {
                    return Err(invalid("schedule.every_ms"));
                }
                Ok(Some(now.checked_add(*every_ms).ok_or_else(internal)?))
            }
            Self::Daily {
                hour,
                minute,
                timezone,
            } => {
                if *hour > 23 || *minute > 59 {
                    return Err(invalid("schedule"));
                }
                next_cron(&format!("{minute} {hour} * * *"), timezone, now)
            }
            Self::Weekly {
                weekdays,
                hour,
                minute,
                timezone,
            } => {
                if weekdays.is_empty()
                    || weekdays.len() > 7
                    || weekdays.iter().any(|d| *d > 6)
                    || *hour > 23
                    || *minute > 59
                {
                    return Err(invalid("schedule"));
                }
                let expression = format!(
                    "{minute} {hour} * * {}",
                    weekdays
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
                next_cron(&expression, timezone, now)
            }
            Self::Cron {
                expression,
                timezone,
            } => next_cron(expression, timezone, now),
        }
    }
}
async fn next_at(spec: ScheduleSpec, now: i64) -> Result<Option<i64>, ToolError> {
    tokio::task::spawn_blocking(move || spec.next_after(now))
        .await
        .map_err(|_| internal())?
}
fn cron_field(text: &str, min: u8, max: u8) -> Result<BTreeSet<u8>, ToolError> {
    if text.len() > 128 {
        return Err(invalid("schedule.expression"));
    }
    let mut values = BTreeSet::new();
    for term in text.split(',') {
        let (base, step) = match term.split_once('/') {
            Some((a, b)) => (
                a,
                b.parse::<u8>()
                    .map_err(|_| invalid("schedule.expression"))?,
            ),
            None => (term, 1),
        };
        if step == 0 {
            return Err(invalid("schedule.expression"));
        }
        let (a, b) = if base == "*" {
            (min, max)
        } else if let Some((a, b)) = base.split_once('-') {
            (
                a.parse().map_err(|_| invalid("schedule.expression"))?,
                b.parse().map_err(|_| invalid("schedule.expression"))?,
            )
        } else {
            let a = base.parse().map_err(|_| invalid("schedule.expression"))?;
            (a, a)
        };
        if a < min || b > max || a > b {
            return Err(invalid("schedule.expression"));
        }
        for value in (a..=b).step_by(usize::from(step)) {
            values.insert(value);
        }
    }
    Ok(values)
}
fn next_cron(expression: &str, timezone: &str, now: i64) -> Result<Option<i64>, ToolError> {
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(invalid("schedule.expression"));
    }
    let minute = cron_field(fields[0], 0, 59)?;
    let hour = cron_field(fields[1], 0, 23)?;
    let day = cron_field(fields[2], 1, 31)?;
    let month = cron_field(fields[3], 1, 12)?;
    let weekday = cron_field(fields[4], 0, 6)?;
    let zone = jiff::tz::TimeZone::get(timezone).map_err(|_| invalid("schedule.timezone"))?;
    let start = now
        .div_euclid(60_000)
        .checked_add(1)
        .and_then(|v| v.checked_mul(60_000))
        .ok_or_else(internal)?;
    // Search is bounded to a leap-year cycle. Nonexistent wall times are skipped;
    // repeated DST wall minutes are separate real occurrences (UTC identities).
    for offset in 0..(366_i64 * 24 * 60 * 4) {
        let at = start.checked_add(offset * 60_000).ok_or_else(internal)?;
        let local = jiff::Timestamp::from_millisecond(at)
            .map_err(|_| invalid("schedule"))?
            .to_zoned(zone.clone());
        let date_match = if fields[2] != "*" && fields[4] != "*" {
            day.contains(&(local.day() as u8))
                || weekday.contains(&(local.weekday().to_sunday_zero_offset() as u8))
        } else {
            day.contains(&(local.day() as u8))
                && weekday.contains(&(local.weekday().to_sunday_zero_offset() as u8))
        };
        if minute.contains(&(local.minute() as u8))
            && hour.contains(&(local.hour() as u8))
            && month.contains(&(local.month() as u8))
            && date_match
        {
            return Ok(Some(at));
        }
    }
    Err(invalid("schedule.expression"))
}
fn schedule_owner(row: &TaskSchedule) -> Result<OwnerScope, ToolError> {
    owner(&json!({"owner_kind":row.owner_kind,"owner_id":row.owner_id}))
}
fn request(
    context: &TaskDispatchContext,
    action: &str,
    row: &TaskSchedule,
    cap: Capability,
    at: u64,
) -> Result<AuthorityRequest, ToolError> {
    authority_request(
        context,
        action,
        &schedule_owner(row)?,
        row.schedule_id.clone(),
        cap,
        at,
    )
}
fn render(row: &TaskSchedule) -> Value {
    let template: Value = serde_json::from_str(&row.task_template_json).unwrap_or(Value::Null);
    json!({"schedule_id":row.schedule_id,"name":row.name,"owner_kind":row.owner_kind,"owner_id":row.owner_id,"agent_id":template.get("agent_id"),"schedule":serde_json::from_str::<Value>(&row.schedule_spec_json).unwrap_or(Value::Null),"armed":row.armed,"next_run_at":if row.armed{Some(row.next_run_at)}else{None},"revision":row.revision,"last_task_id":row.last_task_id,"last_error_kind":row.last_error_kind,"missed_run_policy":"coalesce_one","dst_policy":"skip_nonexistent_repeat_ambiguous","next_retry_at":row.next_retry_at,"retry_policy":serde_json::from_str::<Value>(&row.retry_policy_json).unwrap_or(Value::Null)})
}
async fn validate_agent(
    context: &TaskDispatchContext,
    task: &Value,
    own: &OwnerScope,
) -> Result<(), ToolError> {
    let agent = context
        .store
        .get_agent_definition(required(task, "agent_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)?;
    if &agent.owner != own || agent.state != AgentState::Active {
        return Err(denied());
    }
    Ok(())
}
fn template(params: &Value) -> Result<Value, ToolError> {
    let input = required(params, "input")?;
    if input.len() > 1024 * 1024 {
        return Err(invalid("input"));
    }
    let mut value = json!({"owner_kind":required(params,"owner_kind")?,"owner_id":required(params,"owner_id")?,"agent_id":required(params,"agent_id")?,"input_digest":format!("sha256:{}",hex::encode(Sha256::digest(input.as_bytes()))),"input":input});
    if let Some(project) = params.get("project_id") {
        value["project_id"] = project.clone();
    }
    Ok(value)
}
pub(crate) async fn dispatch_schedule(
    context: TaskDispatchContext,
    name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let now = now()?;
    let now_i = i64::try_from(now).map_err(|_| internal())?;
    if name == "tasks.schedule_list" {
        let after = params
            .get("cursor")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let rows = context
            .store
            .task_schedule_page(after, 100)
            .await
            .map_err(map)?;
        let next = rows.last().map(|r| r.schedule_id.clone());
        let mut visible = Vec::new();
        for row in rows {
            if authorize_action(
                &context.store,
                request(&context, name, &row, Capability::ScopeRead, now)?,
            )
            .await
            .is_ok()
            {
                visible.push(render(&row));
            }
        }
        return Ok(json!({"schedules":visible,"next_cursor":next}));
    }
    if name == "tasks.schedule_create" {
        let id = required(&params, "schedule_id")?;
        let display = required(&params, "name")?;
        if id.len() > 256 || display.len() > 200 {
            return Err(invalid("schedule"));
        }
        let task = template(&params)?;
        let own = owner(&task)?;
        validate_agent(&context, &task, &own).await?;

        // Creation is an explicit durable delegation, requiring both create and operate now.
        authorize(
            &context,
            "tasks.create",
            &own,
            id.clone(),
            Capability::ScopeCreate,
            now,
        )
        .await?;
        authorize(
            &context,
            "tasks.queue",
            &own,
            id.clone(),
            Capability::ScopeOperate,
            now,
        )
        .await?;
        let spec: ScheduleSpec = serde_json::from_value(
            params
                .get("schedule")
                .cloned()
                .ok_or_else(|| invalid("schedule"))?,
        )
        .map_err(|_| invalid("schedule"))?;
        let next = next_at(spec.clone(), now_i)
            .await?
            .ok_or_else(|| invalid("schedule.at"))?;
        let retry_policy: RetryPolicy =
            serde_json::from_value(params.get("retry_policy").cloned().unwrap_or(json!({})))
                .map_err(|_| invalid("retry_policy"))?;
        retry_policy
            .validate()
            .map_err(|_| invalid("retry_policy"))?;
        let (kind, owner_id) = owner_wire(&own);
        let row = TaskSchedule {
            schedule_id: id,
            owner_kind: kind.into(),
            owner_id: owner_id.into(),
            creator_principal_id: String::new(),
            name: display,
            task_template_json: task.to_string(),
            schedule_spec_json: serde_json::to_string(&spec).map_err(|_| internal())?,
            retry_policy_json: serde_json::to_string(&retry_policy).map_err(|_| internal())?,
            identity_ref_json: serde_json::to_string(
                &crate::access::DurableIdentityReference::capture(&context.identity),
            )
            .map_err(|_| internal())?,
            ceiling_json: "[\"scope:read\",\"scope:create\",\"scope:operate\"]".into(),
            armed: params
                .get("armed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            next_run_at: next,
            revision: 1,
            last_task_id: None,
            last_error_kind: None,
            next_retry_at: None,
        };
        context
            .store
            .task_schedule_save(
                row.clone(),
                request(&context, name, &row, Capability::ScopeCreate, now)?,
                true,
                now_i,
            )
            .await
            .map_err(map)?;
        return Ok(render(&row));
    }
    let mut row = context
        .store
        .task_schedule_get(required(&params, "schedule_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)?;
    let cap = if name == "tasks.schedule_get" {
        Capability::ScopeRead
    } else if name == "tasks.schedule_delete" {
        Capability::ScopeDelete
    } else {
        Capability::ScopeOperate
    };
    let lease = authorize_action(&context.store, request(&context, name, &row, cap, now)?)
        .await
        .map_err(map)?;
    // Schedule input and durable delegation edits remain creator-only, like Task outputs.
    if lease.binding().principal_id() != row.creator_principal_id {
        return Err(denied());
    }
    if name == "tasks.schedule_get" {
        let mut result = render(&row);
        result["task_template"] =
            serde_json::from_str(&row.task_template_json).map_err(|_| internal())?;
        return Ok(result);
    }
    if name == "tasks.schedule_delete" {
        context
            .store
            .task_schedule_delete(
                row.schedule_id.clone(),
                request(&context, name, &row, cap, now)?,
            )
            .await
            .map_err(map)?;
        return Ok(json!({"deleted":true}));
    }
    if name == "tasks.schedule_run_now" {
        let idempotency = required(&params, "idempotency_key")?;
        if idempotency.len() > 256 {
            return Err(invalid("idempotency_key"));
        }
        let key = format!("manual:{idempotency}");
        context
            .store
            .task_schedule_enqueue(row, key, now_i, None, now_i)
            .await
            .map_err(map)?;
        return Ok(json!({"state":"pending"}));
    }
    match name {
        "tasks.schedule_arm" => {
            row.armed = true;
            let spec: ScheduleSpec =
                serde_json::from_str(&row.schedule_spec_json).map_err(|_| internal())?;
            row.next_run_at = next_at(spec.clone(), now_i)
                .await?
                .ok_or_else(|| invalid("schedule.at"))?;
        }
        "tasks.schedule_pause" => row.armed = false,
        "tasks.schedule_edit" => {
            if let Some(policy) = params.get("retry_policy") {
                let policy: RetryPolicy =
                    serde_json::from_value(policy.clone()).map_err(|_| invalid("retry_policy"))?;
                policy.validate().map_err(|_| invalid("retry_policy"))?;
                row.retry_policy_json = serde_json::to_string(&policy).map_err(|_| internal())?;
            }

            if let Some(display) = params.get("name").and_then(Value::as_str) {
                if display.trim().is_empty() || display.len() > 200 {
                    return Err(invalid("name"));
                }
                row.name = display.into();
            }
            if let Some(spec) = params.get("schedule") {
                let spec: ScheduleSpec =
                    serde_json::from_value(spec.clone()).map_err(|_| invalid("schedule"))?;
                row.next_run_at = next_at(spec.clone(), now_i)
                    .await?
                    .ok_or_else(|| invalid("schedule.at"))?;
                row.schedule_spec_json = serde_json::to_string(&spec).map_err(|_| internal())?;
            }
            if params.get("input").is_some() || params.get("agent_id").is_some() {
                let mut old: Value =
                    serde_json::from_str(&row.task_template_json).map_err(|_| internal())?;
                for key in ["input", "agent_id"] {
                    if let Some(v) = params.get(key) {
                        old[key] = v.clone();
                    }
                }
                row.task_template_json = template(&old)?.to_string();
            }
        }
        _ => return Err(unknown(name)),
    }
    let task: Value = serde_json::from_str(&row.task_template_json).map_err(|_| internal())?;
    if name != "tasks.schedule_pause" {
        validate_agent(&context, &task, &schedule_owner(&row)?).await?;
    }
    row.identity_ref_json = serde_json::to_string(
        &crate::access::DurableIdentityReference::capture(&context.identity),
    )
    .map_err(|_| internal())?;
    context
        .store
        .task_schedule_save(
            row.clone(),
            request(&context, name, &row, cap, now)?,
            false,
            now_i,
        )
        .await
        .map_err(map)?;
    row.revision += 1;
    Ok(render(&row))
}

/// Starts once per AccessRuntime. The runtime owns the loop; each tick uses its
/// current ready store. No detached per-schedule timers or stored transport credentials.
pub(crate) fn start<F, Fut>(ready_store: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Option<Option<crate::access::AccessStore>>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match ready_store().await {
                None => break,
                Some(None) => {}
                Some(Some(store)) => {
                    if let Err(error) = tick(store).await {
                        tracing::warn!(kind = error.kind(), "task scheduler tick failed");
                    }
                }
            }
        }
    });
}
async fn tick(store: crate::access::AccessStore) -> Result<(), ToolError> {
    let now = i64::try_from(now()?).map_err(|_| internal())?;
    store
        .task_schedule_observe_settlements(now)
        .await
        .map_err(map)?;
    for row in store.task_schedules_due(now).await.map_err(map)? {
        let spec: ScheduleSpec =
            serde_json::from_str(&row.schedule_spec_json).map_err(|_| internal())?;
        let next = next_at(spec, now).await?;
        let due = row.next_run_at;
        let key = format!("due:{}:{due}", row.revision);
        store
            .task_schedule_enqueue(row, key, due, next, now)
            .await
            .map_err(map)?;
    }
    for _ in 0..4 {
        let Some(occurrence) = store.task_schedule_claim(now).await.map_err(map)? else {
            break;
        };
        let result = submit(&store, &occurrence).await;
        store
            .task_schedule_finish(
                occurrence,
                result.err().map(|error| error.kind().to_owned()),
            )
            .await
            .map_err(map)?;
    }
    Ok(())
}
async fn submit(
    store: &crate::access::AccessStore,
    occurrence: &ScheduleOccurrence,
) -> Result<(), ToolError> {
    store
        .task_schedule_validate_claim(
            occurrence.clone(),
            i64::try_from(now()?).map_err(|_| internal())?,
        )
        .await
        .map_err(map)?;
    let identity = serde_json::from_str::<crate::access::DurableIdentityReference>(
        &occurrence.schedule.identity_ref_json,
    )
    .map_err(|_| internal())?
    .restore()
    .map_err(map)?;
    let context = TaskDispatchContext {
        store: store.clone(),
        identity,
        ceiling: AuthorityCeiling::durable_execution(),
    };
    let mut params: Value =
        serde_json::from_str(&occurrence.schedule.task_template_json).map_err(|_| internal())?;
    // Validate current authority before any replay shortcut.
    let admission_time = now()?;
    let own = owner(&params)?;
    let lease = authorize(
        &context,
        "tasks.queue",
        &own,
        occurrence.task_id.clone(),
        Capability::ScopeOperate,
        admission_time,
    )
    .await?;
    if lease.binding().principal_id() != occurrence.schedule.creator_principal_id {
        return Err(denied());
    }
    if occurrence.attempt_number > 0 {
        let original = store
            .get_agent_task(occurrence.original_task_id.clone())
            .await
            .map_err(map)?
            .ok_or_else(denied)?;
        // Retry the original pinned definition; an agent edit requires a new occurrence.
        pinned_definition(&context, &original).await?;
    }
    if let Some(existing) = store
        .get_agent_task(occurrence.task_id.clone())
        .await
        .map_err(map)?
    {
        if existing.intent.owner != own
            || existing.intent.creator.as_str() != occurrence.schedule.creator_principal_id
            || existing.intent.input_digest != required(&params, "input_digest")?
        {
            return Err(denied());
        }
        if existing.state != TaskState::Created {
            return Ok(());
        }
    } else {
        params["task_id"] = json!(occurrence.task_id);
        params["idempotency_key"] = json!(occurrence.task_id);
        Box::pin(dispatch(context.clone(), "tasks.create", params)).await?;
    }
    let record = store
        .get_agent_task(occurrence.task_id.clone())
        .await
        .map_err(map)?
        .ok_or_else(denied)?;
    queue_authorized_task(
        context,
        record,
        Some(crate::access::ScheduleAdmission {
            schedule_id: occurrence.schedule.schedule_id.clone(),
            occurrence_key: occurrence.key.clone(),
            claim_token: occurrence.claim_token.clone(),
            revision: occurrence.schedule.revision,
            attempt_number: occurrence.attempt_number,
        }),
        now()?,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn schedule_revocation_and_pause_prevent_new_task_admissions() {
        use crate::dispatch::agents::{
            self,
            test_support::{BOOTSTRAP_PRINCIPAL, agent_context, agent_params, fixture},
        };
        let (_directory, store, identity) = fixture().await;
        agents::dispatch(
            agent_context(&store, &identity),
            "agents.create",
            agent_params("scheduled-agent"),
        )
        .await
        .unwrap();
        let context = TaskDispatchContext {
            store: store.clone(),
            identity,
            ceiling: AuthorityCeiling::trusted_local(),
        };
        dispatch_schedule(context.clone(),"tasks.schedule_create",json!({"schedule_id":"schedule-one","name":"Daily","owner_kind":"personal","owner_id":BOOTSTRAP_PRINCIPAL,"agent_id":"scheduled-agent","input":"hello","schedule":{"kind":"interval","every_ms":60000},"armed":true})).await.unwrap();
        dispatch_schedule(
            context.clone(),
            "tasks.schedule_run_now",
            json!({"schedule_id":"schedule-one","idempotency_key":"first"}),
        )
        .await
        .unwrap();
        let at = i64::try_from(now().unwrap()).unwrap();
        let claimed = store.task_schedule_claim(at).await.unwrap().unwrap();
        dispatch_schedule(
            context.clone(),
            "tasks.schedule_pause",
            json!({"schedule_id":"schedule-one"}),
        )
        .await
        .unwrap();
        assert!(submit(&store, &claimed).await.is_err());
        assert!(
            store
                .get_agent_task(claimed.task_id)
                .await
                .unwrap()
                .is_none()
        );
        dispatch_schedule(
            context,
            "tasks.schedule_run_now",
            json!({"schedule_id":"schedule-one","idempotency_key":"second"}),
        )
        .await
        .unwrap();
        let claimed = store
            .task_schedule_claim(i64::try_from(now().unwrap()).unwrap())
            .await
            .unwrap()
            .unwrap();
        store.execute_test_statement("UPDATE principal_links SET status='revoked',link_generation=link_generation+1 WHERE principal_id='bootstrap-owner'").await.unwrap();
        assert!(submit(&store, &claimed).await.is_err());
        assert!(
            store
                .get_agent_task(claimed.task_id)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn schedule_queue_transaction_rejects_missing_and_paused_claims() {
        use crate::dispatch::agents::{
            self,
            test_support::{BOOTSTRAP_PRINCIPAL, agent_context, agent_params, fixture},
        };
        let (_directory, store, identity) = fixture().await;
        agents::dispatch(
            agent_context(&store, &identity),
            "agents.create",
            agent_params("fenced-agent"),
        )
        .await
        .unwrap();
        let context = TaskDispatchContext {
            store: store.clone(),
            identity,
            ceiling: AuthorityCeiling::trusted_local(),
        };
        dispatch_schedule(context.clone(),"tasks.schedule_create",json!({"schedule_id":"fenced","name":"Fenced","owner_kind":"personal","owner_id":BOOTSTRAP_PRINCIPAL,"agent_id":"fenced-agent","input":"hello","schedule":{"kind":"interval","every_ms":60000}})).await.unwrap();
        dispatch_schedule(
            context.clone(),
            "tasks.schedule_run_now",
            json!({"schedule_id":"fenced","idempotency_key":"one"}),
        )
        .await
        .unwrap();
        let at = now().unwrap();
        let occurrence = store
            .task_schedule_claim(i64::try_from(at).unwrap())
            .await
            .unwrap()
            .unwrap();
        let mut input: Value =
            serde_json::from_str(&occurrence.schedule.task_template_json).unwrap();
        input["task_id"] = json!(occurrence.task_id);
        input["idempotency_key"] = json!(occurrence.task_id);
        Box::pin(dispatch(context.clone(), "tasks.create", input))
            .await
            .unwrap();
        let own = schedule_owner(&occurrence.schedule).unwrap();
        let queue_request = authority_request(
            &context,
            "tasks.queue",
            &own,
            occurrence.task_id.clone(),
            Capability::ScopeOperate,
            at,
        )
        .unwrap();
        assert!(
            store
                .authorize_and_transition_agent_task(
                    queue_request,
                    occurrence.task_id.clone(),
                    TaskState::Created,
                    TaskState::Queued,
                    context.identity.safe_fingerprint(),
                    0,
                    i64::try_from(at).unwrap(),
                    None
                )
                .await
                .is_err()
        );
        dispatch_schedule(
            context.clone(),
            "tasks.schedule_pause",
            json!({"schedule_id":"fenced"}),
        )
        .await
        .unwrap();
        let queue_request = authority_request(
            &context,
            "tasks.queue",
            &own,
            occurrence.task_id.clone(),
            Capability::ScopeOperate,
            at,
        )
        .unwrap();
        let fence = crate::access::ScheduleAdmission {
            schedule_id: "fenced".into(),
            occurrence_key: occurrence.key,
            claim_token: occurrence.claim_token,
            revision: occurrence.schedule.revision,
            attempt_number: occurrence.attempt_number,
        };
        assert!(
            store
                .authorize_and_transition_agent_task(
                    queue_request,
                    occurrence.task_id.clone(),
                    TaskState::Created,
                    TaskState::Queued,
                    context.identity.safe_fingerprint(),
                    0,
                    i64::try_from(at).unwrap(),
                    Some(fence)
                )
                .await
                .is_err()
        );
        assert_eq!(
            store
                .get_agent_task(occurrence.task_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            TaskState::Created
        );
    }
    fn millis(value: &str) -> i64 {
        value.parse::<jiff::Timestamp>().unwrap().as_millisecond()
    }
    #[test]
    fn schedule_weekly_uses_timezone_and_skips_nonexistent_wall_time() {
        let spec = ScheduleSpec::Weekly {
            weekdays: vec![0],
            hour: 2,
            minute: 30,
            timezone: "America/New_York".into(),
        };
        assert_eq!(
            spec.next_after(millis("2026-03-08T05:00:00Z")).unwrap(),
            Some(millis("2026-03-15T06:30:00Z"))
        );
    }
    #[test]
    fn schedule_cron_supports_selected_weekdays_ranges_and_steps() {
        let spec = ScheduleSpec::Cron {
            expression: "*/15 7-8 * * 1,4".into(),
            timezone: "America/New_York".into(),
        };
        assert_eq!(
            spec.next_after(millis("2026-09-14T11:02:00Z")).unwrap(),
            Some(millis("2026-09-14T11:15:00Z"))
        );
        assert!(next_cron("*/0 * * * *", "UTC", 0).is_err());
        assert!(next_cron("0 25 * * *", "UTC", 0).is_err());
        assert!(next_cron("0 2 * * *", "Not/A_Zone", 0).is_err());
    }
    #[test]
    fn schedule_interval_and_once_have_explicit_missed_run_policy() {
        assert_eq!(
            ScheduleSpec::Interval { every_ms: 60_000 }
                .next_after(180_001)
                .unwrap(),
            Some(240_001)
        );
        assert_eq!(
            ScheduleSpec::Once { at: 60_000 }
                .next_after(180_001)
                .unwrap(),
            None
        );
        assert!(
            ScheduleSpec::Interval { every_ms: 0 }
                .next_after(0)
                .is_err()
        );
    }
    #[test]
    fn schedule_input_digest_matches_actual_task_contract() {
        let value = template(
            &json!({"owner_kind":"personal","owner_id":"p","agent_id":"a","input":"hello"}),
        )
        .unwrap();
        assert_eq!(
            value["input_digest"],
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
