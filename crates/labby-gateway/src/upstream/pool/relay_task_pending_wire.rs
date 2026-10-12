//! Borrowed sizing view for pinned RMCP task notification JSON.
//!
//! RMCP DetailedTask clones its owned wire value; request envelopes materialize
//! params as Value, and form schemas clone into an ordered wire map. Avoid those
//! serializers here. Byte-equivalence tests qualify this view on SDK upgrades.

use rmcp::model::{
    ElicitRequestParams, ElicitationSchema, Extensions, InputRequest, InputRequests, JsonObject,
    MetaObject, NotificationMetaObject, RequestMetaObject, RequestParamsMeta, Task, TaskPayload,
    TaskStatusNotificationParams,
};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

#[path = "relay_task_pending_fields.rs"]
mod fields;
use fields::{FieldMode, ParamsFields};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskParamsWire<'a> {
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<&'a NotificationMetaObject>,
    #[serde(flatten)]
    task: &'a Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_requests: Option<InputsWire<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a JsonObject>,
}

impl<'a> TaskParamsWire<'a> {
    pub(super) fn new(params: &'a TaskStatusNotificationParams) -> Option<Self> {
        let (input_requests, result, error) = match &params.task.payload {
            TaskPayload::Working | TaskPayload::Cancelled => (None, None, None),
            TaskPayload::InputRequired { input_requests } => {
                (Some(InputsWire(input_requests)), None, None)
            }
            TaskPayload::Completed { result } => (None, Some(result), None),
            TaskPayload::Failed { error } => (None, None, Some(error)),
            _ => return None, // Unknown future payloads fail byte admission.
        };
        Some(Self {
            meta: params.meta.as_ref(),
            task: &params.task.task,
            input_requests,
            result,
            error,
        })
    }
}

struct InputsWire<'a>(&'a InputRequests);

impl Serialize for InputsWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (id, request) in self.0 {
            map.serialize_entry(id, &InputWire(request))?;
        }
        map.end()
    }
}

struct InputWire<'a>(&'a InputRequest);

impl Serialize for InputWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.0 {
            InputRequest::CreateMessage(request) => {
                map.serialize_entry("method", &request.method)?;
                map.serialize_entry(
                    "params",
                    &RequestParamsWire {
                        params: &request.params,
                        meta: MergedMeta::new(request.params.meta(), &request.extensions),
                    },
                )?;
            }
            InputRequest::Elicitation(request) => {
                map.serialize_entry("method", &request.method)?;
                map.serialize_entry(
                    "params",
                    &ElicitParamsWire {
                        params: &request.params,
                        meta: MergedMeta::new(request.params.meta(), &request.extensions),
                    },
                )?;
            }
            InputRequest::ListRoots(request) => {
                map.serialize_entry("method", &request.method)?;
                let meta = MergedMeta::new(None, &request.extensions);
                if meta.present() {
                    map.serialize_entry("params", &MetaOnly(meta))?;
                }
            }
            _ => return Err(serde::ser::Error::custom("unsupported task input request")),
        }
        map.end()
    }
}

// Preserve params < legacy extension < typed extension precedence and insertion
// order without cloning any map/key/Value. Present empty metadata stays present.
struct MergedMeta<'a> {
    params: Option<&'a JsonObject>,
    legacy: Option<&'a JsonObject>,
    typed: Option<&'a JsonObject>,
}

impl<'a> MergedMeta<'a> {
    fn new(params: Option<&'a RequestMetaObject>, extensions: &'a Extensions) -> Self {
        Self {
            params: params.map(|meta| &meta.0.0),
            legacy: extensions.get::<MetaObject>().map(|meta| &meta.0),
            typed: extensions.get::<RequestMetaObject>().map(|meta| &meta.0.0),
        }
    }

    fn present(&self) -> bool {
        self.params.is_some() || self.legacy.is_some() || self.typed.is_some()
    }
}

impl Serialize for MergedMeta<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (index, source) in [self.params, self.legacy, self.typed]
            .into_iter()
            .enumerate()
        {
            let Some(source) = source else { continue };
            for (key, original) in source {
                if (index > 0 && self.params.is_some_and(|params| params.contains_key(key)))
                    || (index > 1 && self.legacy.is_some_and(|legacy| legacy.contains_key(key)))
                {
                    continue;
                }
                let value = self
                    .typed
                    .and_then(|typed| typed.get(key))
                    .or_else(|| self.legacy.and_then(|legacy| legacy.get(key)))
                    .unwrap_or(original);
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

struct MetaOnly<'a>(MergedMeta<'a>);

impl Serialize for MetaOnly<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("_meta", &self.0)?;
        map.end()
    }
}

struct RequestParamsWire<'a, T> {
    params: &'a T,
    meta: MergedMeta<'a>,
}

impl<T: Serialize + RequestParamsMeta> Serialize for RequestParamsWire<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if self.meta.present() {
            map.serialize_entry("_meta", &self.meta)?;
        }
        // Pinned WithMeta removes params._meta with JSON Map::swap_remove.
        // Reproduce its last-field-first order with a constant-space field scan.
        let last = if self.params.meta().is_some() {
            let last = self.params.serialize(ParamsFields {
                map: &mut map,
                mode: FieldMode::Last,
                last: None,
            })?;
            if let Some(last) = last {
                self.params.serialize(ParamsFields {
                    map: &mut map,
                    mode: FieldMode::Only(last),
                    last: None,
                })?;
            }
            last
        } else {
            None
        };
        self.params.serialize(ParamsFields {
            map: &mut map,
            mode: FieldMode::Except(last),
            last: None,
        })?;
        map.end()
    }
}

struct ElicitParamsWire<'a> {
    params: &'a ElicitRequestParams,
    meta: MergedMeta<'a>,
}

impl Serialize for ElicitParamsWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if self.meta.present() {
            map.serialize_entry("_meta", &self.meta)?;
        }
        match self.params {
            ElicitRequestParams::FormElicitationParams {
                meta,
                message,
                requested_schema,
            } => {
                map.serialize_entry("mode", "form")?;
                if meta.is_some() {
                    map.serialize_entry("requestedSchema", &SchemaWire(requested_schema))?;
                    map.serialize_entry("message", message)?;
                } else {
                    map.serialize_entry("message", message)?;
                    map.serialize_entry("requestedSchema", &SchemaWire(requested_schema))?;
                }
            }
            ElicitRequestParams::UrlElicitationParams {
                meta,
                message,
                url,
                elicitation_id,
            } => {
                map.serialize_entry("mode", "url")?;
                if meta.is_some() {
                    map.serialize_entry("elicitationId", elicitation_id)?;
                    map.serialize_entry("message", message)?;
                    map.serialize_entry("url", url)?;
                } else {
                    map.serialize_entry("message", message)?;
                    map.serialize_entry("url", url)?;
                    map.serialize_entry("elicitationId", elicitation_id)?;
                }
            }
            _ => return Err(serde::ser::Error::custom("unsupported task elicitation")),
        }
        map.end()
    }
}

struct SchemaWire<'a>(&'a ElicitationSchema);

impl Serialize for SchemaWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = self.0;
        let mut map = serializer.serialize_map(None)?;
        if let Some(dialect) = &schema.schema {
            map.serialize_entry("$schema", dialect)?;
        }
        map.serialize_entry("type", &schema.type_)?;
        if let Some(title) = &schema.title {
            map.serialize_entry("title", title)?;
        }
        map.serialize_entry("properties", &PropertiesWire(schema))?;
        if let Some(required) = &schema.required {
            map.serialize_entry("required", required)?;
        }
        if let Some(description) = &schema.description {
            map.serialize_entry("description", description)?;
        }
        map.end()
    }
}

struct PropertiesWire<'a>(&'a ElicitationSchema);

impl Serialize for PropertiesWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let schema = self.0;
        let mut map = serializer.serialize_map(Some(schema.properties.len()))?;
        if let Some(order) = &schema.property_order {
            for (index, name) in order.iter().enumerate() {
                if !order[..index].contains(name)
                    && let Some(property) = schema.properties.get(name)
                {
                    map.serialize_entry(name, property)?;
                }
            }
        }
        for (name, property) in &schema.properties {
            if !schema
                .property_order
                .as_ref()
                .is_some_and(|order| order.contains(name))
            {
                map.serialize_entry(name, property)?;
            }
        }
        map.end()
    }
}

#[cfg(test)]
#[path = "relay_task_pending_wire_tests.rs"]
mod tests;
