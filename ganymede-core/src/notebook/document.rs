use super::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CellId(String);
impl CellId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    fn fresh() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl std::fmt::Display for CellId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Retains the file's string/line-array representation until edited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Multiline {
    Text(String),
    Lines(Vec<String>),
}
impl Multiline {
    pub fn text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Lines(lines) => lines.concat(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Output {
    Known(KnownOutput),
    /// Preserves extension output types and fields we cannot yet interpret.
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "output_type", rename_all = "snake_case")]
pub enum KnownOutput {
    Stream {
        name: String,
        text: Multiline,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    DisplayData {
        data: Map<String, Value>,
        metadata: Map<String, Value>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    ExecuteResult {
        data: Map<String, Value>,
        metadata: Map<String, Value>,
        execution_count: u64,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Error {
        ename: String,
        evalue: String,
        traceback: Vec<String>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Code,
    Markdown,
    Raw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    id: CellId,
    kind: CellKind,
    source: Multiline,
    metadata: Map<String, Value>,
    outputs: Vec<Output>,
    execution_count: Option<u64>,
    extra: Map<String, Value>,
}
impl Cell {
    pub fn id(&self) -> &CellId {
        &self.id
    }
    pub fn kind(&self) -> CellKind {
        self.kind
    }
    pub fn source(&self) -> String {
        self.source.text()
    }
    pub fn metadata(&self) -> &Map<String, Value> {
        &self.metadata
    }
    pub fn outputs(&self) -> &[Output] {
        &self.outputs
    }
    pub fn execution_count(&self) -> Option<u64> {
        self.execution_count
    }
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
    }
    fn to_json(&self) -> Value {
        let mut value = self.extra.clone();
        value.insert("id".into(), json!(self.id));
        value.insert(
            "cell_type".into(),
            json!(match self.kind {
                CellKind::Code => "code",
                CellKind::Markdown => "markdown",
                CellKind::Raw => "raw",
            }),
        );
        value.insert("source".into(), json!(self.source));
        value.insert("metadata".into(), json!(self.metadata));
        if self.kind == CellKind::Code {
            value.insert("outputs".into(), json!(self.outputs));
            value.insert("execution_count".into(), json!(self.execution_count));
        }
        Value::Object(value)
    }
}

/// An editable document. Mutations are revisioned; callers receive immutable cells.
#[derive(Debug, Clone)]
pub struct NotebookDocument {
    cells: Vec<Cell>,
    metadata: Map<String, Value>,
    minor: u64,
    extra: Map<String, Value>,
    revision: u64,
    saved_revision: Option<u64>,
}
impl Default for NotebookDocument {
    fn default() -> Self {
        Self::new()
    }
}
impl NotebookDocument {
    pub fn new() -> Self {
        Self {
            cells: vec![],
            metadata: Map::new(),
            minor: 5,
            extra: Map::new(),
            revision: 0,
            saved_revision: None,
        }
    }
    pub fn from_json(value: Value) -> Result<Self> {
        let mut file = object(value, "notebook")?;
        if file.remove("nbformat") != Some(json!(4)) {
            return Err(Error::Invalid("only nbformat 4 is supported".into()));
        }
        let minor = take::<u64>(&mut file, "nbformat_minor")?;
        let metadata = take(&mut file, "metadata")?;
        let values: Vec<Value> = take(&mut file, "cells")?;
        let mut cells = Vec::with_capacity(values.len());
        let mut ids = HashSet::new();
        let mut upgraded = false;
        for value in values {
            let mut fields = object(value, "cell")?;
            let kind = match take::<String>(&mut fields, "cell_type")?.as_str() {
                "code" => CellKind::Code,
                "markdown" => CellKind::Markdown,
                "raw" => CellKind::Raw,
                other => return Err(Error::Invalid(format!("unsupported cell type: {other}"))),
            };
            let id = match fields.remove("id") {
                Some(Value::String(id))
                    if !id.is_empty()
                        && id.len() <= 64
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
                {
                    CellId(id)
                }
                None => {
                    upgraded = true;
                    CellId::fresh()
                }
                _ => return Err(Error::Invalid("invalid cell ID".into())),
            };
            if !ids.insert(id.clone()) {
                return Err(Error::Invalid(format!("duplicate cell ID: {id}")));
            }
            let source = take(&mut fields, "source")?;
            let metadata = take(&mut fields, "metadata")?;
            let (outputs, execution_count) = if kind == CellKind::Code {
                (
                    take(&mut fields, "outputs")?,
                    take(&mut fields, "execution_count")?,
                )
            } else {
                (vec![], None)
            };
            cells.push(Cell {
                id,
                kind,
                source,
                metadata,
                outputs,
                execution_count,
                extra: fields,
            });
        }
        Ok(Self {
            cells,
            metadata,
            minor: if upgraded { minor.max(5) } else { minor },
            extra: file,
            revision: 0,
            saved_revision: if upgraded { None } else { Some(0) },
        })
    }
    pub fn to_json(&self) -> Value {
        let mut file = self.extra.clone();
        file.insert("nbformat".into(), json!(4));
        file.insert("nbformat_minor".into(), json!(self.minor));
        file.insert("metadata".into(), json!(self.metadata));
        file.insert(
            "cells".into(),
            Value::Array(self.cells.iter().map(Cell::to_json).collect()),
        );
        Value::Object(file)
    }
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
    pub fn cell(&self, id: &CellId) -> Result<&Cell> {
        self.cells
            .iter()
            .find(|c| c.id == *id)
            .ok_or_else(|| Error::MissingCell(id.clone()))
    }
    pub fn metadata(&self) -> &Map<String, Value> {
        &self.metadata
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn is_dirty(&self) -> bool {
        self.saved_revision != Some(self.revision)
    }
    /// Acknowledge only the revision actually written, not subsequent edits.
    pub(crate) fn mark_saved(&mut self, revision: u64) {
        self.saved_revision = Some(revision);
    }
    fn changed(&mut self) {
        self.revision += 1;
    }
    pub fn insert_cell(
        &mut self,
        index: usize,
        kind: CellKind,
        source: impl Into<String>,
    ) -> Result<CellId> {
        if index > self.cells.len() {
            return Err(Error::Invalid("cell index out of bounds".into()));
        }
        let id = CellId::fresh();
        self.cells.insert(
            index,
            Cell {
                id: id.clone(),
                kind,
                source: Multiline::Text(source.into()),
                metadata: Map::new(),
                outputs: vec![],
                execution_count: None,
                extra: Map::new(),
            },
        );
        self.minor = self.minor.max(5);
        self.changed();
        Ok(id)
    }
    pub fn remove_cell(&mut self, id: &CellId) -> Result<Cell> {
        let index = self.index(id)?;
        let cell = self.cells.remove(index);
        self.changed();
        Ok(cell)
    }
    /// `index` is the cell's final index after the move.
    pub fn move_cell(&mut self, id: &CellId, index: usize) -> Result<()> {
        if index >= self.cells.len() {
            return Err(Error::Invalid("cell index out of bounds".into()));
        }
        let previous = self.index(id)?;
        if previous != index {
            let cell = self.cells.remove(previous);
            self.cells.insert(index, cell);
            self.changed();
        }
        Ok(())
    }
    pub fn update_source(&mut self, id: &CellId, source: impl Into<String>) -> Result<()> {
        let index = self.index(id)?;
        let source = source.into();
        if self.cells[index].source() != source {
            self.cells[index].source = Multiline::Text(source);
            self.changed();
        }
        Ok(())
    }
    pub fn set_cell_kind(&mut self, id: &CellId, kind: CellKind) -> Result<()> {
        let index = self.index(id)?;
        if self.cells[index].kind != kind {
            self.cells[index].kind = kind;
            self.cells[index].outputs.clear();
            self.cells[index].execution_count = None;
            self.changed();
        }
        Ok(())
    }
    pub fn set_metadata(&mut self, metadata: Map<String, Value>) {
        if self.metadata != metadata {
            self.metadata = metadata;
            self.changed();
        }
    }
    pub fn set_cell_metadata(&mut self, id: &CellId, metadata: Map<String, Value>) -> Result<()> {
        let index = self.index(id)?;
        if self.cells[index].metadata != metadata {
            self.cells[index].metadata = metadata;
            self.changed();
        }
        Ok(())
    }
    pub fn clear_outputs(&mut self, id: &CellId) -> Result<()> {
        let index = self.code_index(id)?;
        if !self.cells[index].outputs.is_empty() || self.cells[index].execution_count.is_some() {
            self.cells[index].outputs.clear();
            self.cells[index].execution_count = None;
            self.changed();
        }
        Ok(())
    }
    fn index(&self, id: &CellId) -> Result<usize> {
        self.cells
            .iter()
            .position(|c| c.id == *id)
            .ok_or_else(|| Error::MissingCell(id.clone()))
    }
    fn code_index(&self, id: &CellId) -> Result<usize> {
        let index = self.index(id)?;
        if self.cells[index].kind != CellKind::Code {
            return Err(Error::Invalid("operation requires a code cell".into()));
        }
        Ok(index)
    }
    pub(crate) fn set_count(&mut self, id: &CellId, count: Option<u64>) -> Result<()> {
        let index = self.code_index(id)?;
        if self.cells[index].execution_count != count {
            self.cells[index].execution_count = count;
            self.changed();
        }
        Ok(())
    }
    pub(crate) fn push_output(&mut self, id: &CellId, output: Output) -> Result<usize> {
        let index = self.code_index(id)?;
        let output_index = self.cells[index].outputs.len();
        self.cells[index].outputs.push(output);
        self.changed();
        Ok(output_index)
    }
    pub(crate) fn update_display(
        &mut self,
        id: &CellId,
        index: usize,
        data: Map<String, Value>,
        metadata: Map<String, Value>,
    ) -> Result<()> {
        let cell = self.code_index(id)?;
        if let Some(Output::Known(
            KnownOutput::DisplayData {
                data: old_data,
                metadata: old_metadata,
                ..
            }
            | KnownOutput::ExecuteResult {
                data: old_data,
                metadata: old_metadata,
                ..
            },
        )) = self.cells[cell].outputs.get_mut(index)
        {
            *old_data = data;
            *old_metadata = metadata;
            self.changed();
        }
        Ok(())
    }
}
fn object(value: Value, name: &str) -> Result<Map<String, Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("{name} must be an object")))
}
fn take<T: serde::de::DeserializeOwned>(fields: &mut Map<String, Value>, key: &str) -> Result<T> {
    Ok(serde_json::from_value(fields.remove(key).ok_or_else(
        || Error::Invalid(format!("missing {key}")),
    )?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_preserves_unknown_fields_and_multiline_representations() {
        let original = json!({"nbformat":4,"nbformat_minor":5,"metadata":{"vendor":{"a":1}},"extra_root":[1,2],"cells":[
            {"id":"code","cell_type":"code","source":["print(1)\n","2"],"metadata":{"custom":true},"execution_count":3,"extra_cell":"keep","outputs":[
                {"output_type":"stream","name":"stdout","text":["a\n","b"],"vendor":7},
                {"output_type":"display_data","data":{"application/json":{"list":[1,2]},"text/plain":["hello\n","world"]},"metadata":{},"vendor":8},
                {"output_type":"extension","custom":{"x":true}}
            ]},
            {"id":"markdown","cell_type":"markdown","source":"![x](attachment:x)","metadata":{},"attachments":{"x":{"image/png":"abc"}}},
            {"id":"raw","cell_type":"raw","source":"raw","metadata":{"format":"text/html"},"vendor":null}
        ]});
        let mut document = NotebookDocument::from_json(original.clone()).unwrap();
        assert_eq!(document.to_json(), original);
        assert!(!document.is_dirty());
        assert_eq!(document.cells()[0].source(), "print(1)\n2");
        let id = document.cells()[0].id().clone();
        document.move_cell(&id, 2).unwrap();
        document.update_source(&id, "changed").unwrap();
        assert_eq!(document.cells()[2].id(), &id);
        assert!(document.is_dirty());
        assert_eq!(document.to_json()["cells"][2]["extra_cell"], "keep");
        let round_trip = NotebookDocument::from_json(document.to_json()).unwrap();
        assert_eq!(round_trip.cells()[2].id(), &id);
    }
    #[test]
    fn old_ids_upgrade_once_and_duplicate_ids_fail() {
        let old = json!({"nbformat":4,"nbformat_minor":0,"metadata":{},"cells":[{"cell_type":"markdown","source":"text","metadata":{}}]});
        let doc = NotebookDocument::from_json(old).unwrap();
        assert!(doc.is_dirty());
        assert_eq!(doc.to_json()["nbformat_minor"], 5);
        let again = NotebookDocument::from_json(doc.to_json()).unwrap();
        assert_eq!(doc.cells()[0].id(), again.cells()[0].id());
        let mut duplicate = doc.to_json();
        let cell = duplicate["cells"][0].clone();
        duplicate["cells"].as_array_mut().unwrap().push(cell);
        assert!(NotebookDocument::from_json(duplicate).is_err());
    }
    #[test]
    fn save_acknowledges_only_its_revision() {
        let mut doc = NotebookDocument::new();
        let id = doc.insert_cell(0, CellKind::Code, "1").unwrap();
        let revision = doc.revision();
        doc.update_source(&id, "2").unwrap();
        doc.mark_saved(revision);
        assert!(doc.is_dirty());
        doc.mark_saved(doc.revision());
        assert!(!doc.is_dirty());
        doc.update_source(&id, "2").unwrap();
        assert!(!doc.is_dirty());
        doc.remove_cell(&id).unwrap();
        assert!(doc.is_dirty());
    }
}
