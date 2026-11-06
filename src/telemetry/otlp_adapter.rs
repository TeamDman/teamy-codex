//! Convert admitted OTLP records without conflating the two Facet dependency identities.
use super::{CodexEvent, Field, FieldValue};
use teamy_tracing::telemetry::{Attribute, TelemetryRecord, TelemetryValue};

fn field(attribute: &Attribute) -> Field {
    Field {
        key: attribute.key.clone(),
        value: value(&attribute.value),
    }
}

fn value(input: &TelemetryValue) -> FieldValue {
    match input {
        TelemetryValue::Empty => FieldValue::Empty,
        TelemetryValue::String(value) => FieldValue::Text(value.clone()),
        TelemetryValue::Bool(value) => FieldValue::Bool(*value),
        TelemetryValue::Integer(value) => FieldValue::Integer(*value),
        TelemetryValue::Float(value) => FieldValue::Float(*value),
        TelemetryValue::Bytes(value) => FieldValue::Bytes(value.clone()),
        TelemetryValue::Array(values) => FieldValue::Array(values.iter().map(value).collect()),
        TelemetryValue::Map(fields) => FieldValue::Map(fields.iter().map(field).collect()),
    }
}

fn text(input: Option<&TelemetryValue>) -> Option<String> {
    match input {
        Some(TelemetryValue::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

/// Convert the event attributes; callers retain the original record for all metadata.
#[must_use]
pub fn from_telemetry_record(record: &TelemetryRecord) -> CodexEvent {
    let source = record
        .resource_attributes
        .iter()
        .find(|item| item.key == "service.instance.id");
    CodexEvent {
        source: text(source.map(|item| &item.value)),
        session: text(record.attribute("conversation.id")),
        call_id: text(record.attribute("call_id")),
        name: text(record.attribute("event.name")).unwrap_or_else(|| record.event_name.clone()),
        fields: record.attributes.iter().map(field).collect(),
    }
}
