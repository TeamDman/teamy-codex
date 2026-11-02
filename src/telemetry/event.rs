use facet::Facet;

/// A typed field retained for inspection, including unrecognized keys.
#[derive(Clone, Debug, Facet, PartialEq)]
pub struct Field {
    pub key: String,
    pub value: FieldValue,
}

/// Values a future OTLP adapter can transfer without stringifying primitives.
#[derive(Clone, Debug, Facet, PartialEq)]
#[repr(u8)]
pub enum FieldValue {
    Empty,
    Text(String),
    Bool(bool),
    Integer(i64),
    Float(f64),
    Bytes(Vec<u8>),
    Array(Vec<FieldValue>),
    Map(Vec<Field>),
}

/// Source identity and raw fields remain available even when no pair is found.
#[derive(Clone, Debug, Default, Facet, PartialEq)]
pub struct CodexEvent {
    pub source: Option<String>,
    pub session: Option<String>,
    pub call_id: Option<String>,
    pub name: String,
    pub fields: Vec<Field>,
}

impl CodexEvent {
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&FieldValue> {
        self.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| &field.value)
    }
}
