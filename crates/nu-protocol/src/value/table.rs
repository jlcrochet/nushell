//! Table data structure for efficient tabular storage
//!
//! `TableData` stores tabular data (rows with consistent columns) more efficiently
//! than `Vec<Record>`. The schema (column names) is stored once via `TableSchema`,
//! and a HashMap provides O(1) column lookup by name.

use crate::{Record, Span, TableSchema, Type, Value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::sync::Arc;

/// Tabular data with shared schema and O(1) column lookup.
///
/// This provides significant memory savings over `Vec<Record>` for tables with many rows,
/// as column names are stored only once. It also enables faster column access via the
/// schema index HashMap.
#[derive(Debug, Clone)]
pub struct TableData {
    schema: TableSchema,
    /// O(1) column name -> index lookup. Wrapped in Arc for cheap cloning when
    /// creating derived tables (take, skip, etc.). Rebuilt after deserialization.
    #[allow(clippy::zero_sized_map_values)]
    schema_index: Arc<HashMap<String, usize>>,
    /// Row-major storage: each inner Vec has one value per column.
    rows: Vec<Vec<Value>>,
}

impl TableData {
    /// Create an empty table with the given schema.
    pub fn new(schema: TableSchema) -> Self {
        let schema_index = Self::build_schema_index(&schema);
        Self {
            schema,
            schema_index,
            rows: Vec::new(),
        }
    }

    /// Create a table with the given schema and preallocated row capacity.
    pub fn with_capacity(schema: TableSchema, row_capacity: usize) -> Self {
        let schema_index = Self::build_schema_index(&schema);
        Self {
            schema,
            schema_index,
            rows: Vec::with_capacity(row_capacity),
        }
    }

    /// Create a table from a schema and existing rows.
    ///
    /// # Errors
    /// Returns an error if any row has a different number of values than the schema has columns.
    pub fn from_rows(schema: TableSchema, rows: Vec<Vec<Value>>) -> Result<Self, TableDataError> {
        let col_count = schema.len();
        for (i, row) in rows.iter().enumerate() {
            if row.len() != col_count {
                return Err(TableDataError::ColumnCountMismatch {
                    expected: col_count,
                    got: row.len(),
                    row_index: i,
                });
            }
        }
        let schema_index = Self::build_schema_index(&schema);
        Ok(Self {
            schema,
            schema_index,
            rows,
        })
    }

    fn build_schema_index(schema: &TableSchema) -> Arc<HashMap<String, usize>> {
        Arc::new(
            schema
                .columns()
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect(),
        )
    }

    /// Get the table schema.
    pub fn schema(&self) -> &TableSchema {
        &self.schema
    }

    /// Set the schema for a table. Can only be called when the table is empty.
    ///
    /// # Errors
    /// Returns an error if the table already has rows.
    pub fn set_schema(&mut self, schema: TableSchema) -> Result<(), TableDataError> {
        if !self.rows.is_empty() {
            return Err(TableDataError::CannotSetSchemaWithRows {
                row_count: self.rows.len(),
            });
        }
        self.schema_index = Self::build_schema_index(&schema);
        self.schema = schema;
        Ok(())
    }

    /// Get the column names.
    pub fn columns(&self) -> &[String] {
        self.schema.columns()
    }

    /// Get the number of columns.
    pub fn num_columns(&self) -> usize {
        self.schema.len()
    }

    /// Get the number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Check if the table is empty (has no rows).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Get the column index for a column name. O(1) lookup.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.schema_index.get(name).copied()
    }

    /// Add a row to the table.
    ///
    /// # Errors
    /// Returns an error if the number of values doesn't match the schema.
    pub fn push_row(&mut self, values: Vec<Value>) -> Result<(), TableDataError> {
        if values.len() != self.schema.len() {
            return Err(TableDataError::ColumnCountMismatch {
                expected: self.schema.len(),
                got: values.len(),
                row_index: self.rows.len(),
            });
        }
        self.rows.push(values);
        Ok(())
    }

    /// Get a row by index, converting it to a Record.
    pub fn get_row(&self, index: usize) -> Option<Record> {
        self.rows.get(index).map(|row| {
            self.schema
                .columns()
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .collect()
        })
    }

    /// Get a row by index as a slice of values (without column names).
    pub fn get_row_values(&self, index: usize) -> Option<&[Value]> {
        self.rows.get(index).map(Vec::as_slice)
    }

    /// Get a column by name as a Vec of references to values.
    pub fn get_column(&self, name: &str) -> Option<Vec<&Value>> {
        let idx = self.column_index(name)?;
        Some(self.rows.iter().map(|row| &row[idx]).collect())
    }

    /// Get a column by name as a Vec of cloned values.
    pub fn get_column_cloned(&self, name: &str) -> Option<Vec<Value>> {
        let idx = self.column_index(name)?;
        Some(self.rows.iter().map(|row| row[idx].clone()).collect())
    }

    /// Get a single cell value by row index and column name.
    pub fn get(&self, row: usize, column: &str) -> Option<&Value> {
        let col_idx = self.column_index(column)?;
        self.rows.get(row).and_then(|r| r.get(col_idx))
    }

    /// Get a mutable reference to a cell value.
    pub fn get_mut(&mut self, row: usize, column: &str) -> Option<&mut Value> {
        let col_idx = self.column_index(column)?;
        self.rows.get_mut(row).and_then(|r| r.get_mut(col_idx))
    }

    /// Get the raw rows as a slice.
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    /// Convert the table to a list of records (Value::List<Record>).
    pub fn to_list(&self, span: Span) -> Value {
        let records: Vec<Value> = self
            .rows
            .iter()
            .map(|row| {
                let record: Record = self
                    .schema
                    .columns()
                    .iter()
                    .cloned()
                    .zip(row.iter().cloned())
                    .collect();
                Value::record(record, span)
            })
            .collect();
        Value::list(records, span)
    }

    /// Create a table from a list of records.
    ///
    /// Returns None if:
    /// - The list is empty
    /// - The list contains non-record values
    /// - Records have inconsistent column sets
    pub fn from_list(list: &[Value]) -> Option<Self> {
        if list.is_empty() {
            return None;
        }

        // Get columns from the first record
        let first_record = list.first()?.as_record().ok()?;
        let columns: Vec<String> = first_record.columns().cloned().collect();
        let schema = TableSchema::new(columns.clone());

        let mut rows = Vec::with_capacity(list.len());

        for val in list {
            let record = val.as_record().ok()?;

            // Check that all records have the same columns
            if record.len() != columns.len() {
                return None;
            }

            let mut row = Vec::with_capacity(columns.len());
            for col in &columns {
                let cell = record.get(col)?;
                row.push(cell.clone());
            }
            rows.push(row);
        }

        Some(Self {
            schema_index: Self::build_schema_index(&schema),
            schema,
            rows,
        })
    }

    /// Get the type of this table.
    pub fn get_type(&self) -> Type {
        if self.rows.is_empty() {
            // Empty table - columns have unknown types
            Type::Table(
                self.schema
                    .columns()
                    .iter()
                    .map(|c| (c.clone(), Type::Any))
                    .collect(),
            )
        } else {
            // Infer column types from values
            Type::Table(
                self.schema
                    .columns()
                    .iter()
                    .enumerate()
                    .map(|(idx, col)| {
                        let col_type = Type::supertype_of(self.rows.iter().map(|row| row[idx].get_type()))
                            .unwrap_or(Type::Any);
                        (col.clone(), col_type)
                    })
                    .collect(),
            )
        }
    }

    /// Returns an estimate of the memory size used by this TableData in bytes.
    pub fn memory_size(&self) -> usize {
        let schema_size = self
            .schema
            .columns()
            .iter()
            .map(|s| std::mem::size_of::<String>() + s.capacity())
            .sum::<usize>();

        let index_size = self.schema_index.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        let rows_size: usize = self
            .rows
            .iter()
            .map(|row| {
                std::mem::size_of::<Vec<Value>>()
                    + row.iter().map(|v| v.memory_size()).sum::<usize>()
            })
            .sum();

        std::mem::size_of::<Self>() + schema_size + index_size + rows_size
    }

    /// Iterate over rows as Records.
    pub fn iter(&self) -> impl Iterator<Item = Record> + '_ {
        self.rows.iter().map(|row| {
            self.schema
                .columns()
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .collect()
        })
    }

    /// Iterate over row indices.
    pub fn row_indices(&self) -> impl Iterator<Item = usize> {
        0..self.rows.len()
    }

    /// Consume the table and iterate over rows as Records.
    pub fn into_iter(self) -> impl Iterator<Item = Record> {
        let columns = self.schema.into_columns();
        self.rows.into_iter().map(move |row| {
            columns
                .iter()
                .cloned()
                .zip(row.into_iter())
                .collect()
        })
    }

    /// Select specific columns, returning a new table.
    ///
    /// Columns that don't exist are silently ignored.
    pub fn select_columns(&self, columns: &[&str]) -> Self {
        // Filter to existing columns and get their indices in one pass
        let selected: Vec<(String, usize)> = columns
            .iter()
            .filter_map(|c| self.column_index(c).map(|idx| ((*c).to_owned(), idx)))
            .collect();

        let new_columns: Vec<String> = selected.iter().map(|(name, _)| name.clone()).collect();
        let indices: Vec<usize> = selected.into_iter().map(|(_, idx)| idx).collect();

        let new_schema = TableSchema::new(new_columns);
        let new_rows: Vec<Vec<Value>> = self
            .rows
            .iter()
            .map(|row| indices.iter().map(|&i| row[i].clone()).collect())
            .collect();

        Self {
            schema_index: Self::build_schema_index(&new_schema),
            schema: new_schema,
            rows: new_rows,
        }
    }

    /// Filter rows by a predicate that receives the row as a Record.
    pub fn filter<F>(&self, mut predicate: F) -> Self
    where
        F: FnMut(&Record) -> bool,
    {
        let new_rows: Vec<Vec<Value>> = self
            .rows
            .iter()
            .filter(|row| {
                let record: Record = self
                    .schema
                    .columns()
                    .iter()
                    .cloned()
                    .zip(row.iter().cloned())
                    .collect();
                predicate(&record)
            })
            .cloned()
            .collect();

        Self {
            schema: self.schema.clone(),
            schema_index: self.schema_index.clone(),
            rows: new_rows,
        }
    }

    /// Take the first n rows.
    pub fn take(&self, n: usize) -> Self {
        Self {
            schema: self.schema.clone(),
            schema_index: self.schema_index.clone(),
            rows: self.rows.iter().take(n).cloned().collect(),
        }
    }

    /// Skip the first n rows.
    pub fn skip(&self, n: usize) -> Self {
        Self {
            schema: self.schema.clone(),
            schema_index: self.schema_index.clone(),
            rows: self.rows.iter().skip(n).cloned().collect(),
        }
    }

    /// Take the last n rows.
    pub fn last(&self, n: usize) -> Self {
        let start = self.rows.len().saturating_sub(n);
        Self {
            schema: self.schema.clone(),
            schema_index: self.schema_index.clone(),
            rows: self.rows.iter().skip(start).cloned().collect(),
        }
    }

    /// Reverse the row order.
    pub fn reverse(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            schema_index: self.schema_index.clone(),
            rows: self.rows.iter().rev().cloned().collect(),
        }
    }

    /// Remove a row at the given index.
    pub fn remove_row(&mut self, index: usize) -> Vec<Value> {
        self.rows.remove(index)
    }

    /// Compare this table with a list of records without allocating.
    /// Returns None if the list contains non-record values.
    pub fn cmp_list(&self, list: &[Value]) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Compare lengths first
        match self.rows.len().cmp(&list.len()) {
            Ordering::Equal => {}
            ord => return Some(ord),
        }

        // Compare row by row
        for (row_values, list_val) in self.rows.iter().zip(list.iter()) {
            let record = match list_val.as_record() {
                Ok(r) => r,
                Err(_) => return None, // Non-record in list
            };

            // Compare schemas: table columns vs record columns
            // First check column count
            if self.schema.columns().len() != record.len() {
                return self.schema.columns().len().partial_cmp(&record.len());
            }

            // Compare columns and values
            for (col, table_val) in self.schema.columns().iter().zip(row_values.iter()) {
                // Check if record has this column
                match record.get(col) {
                    Some(record_val) => {
                        match table_val.partial_cmp(record_val) {
                            Some(Ordering::Equal) => continue,
                            other => return other,
                        }
                    }
                    None => {
                        // Record missing column - compare column names lexicographically
                        // Table has column that record doesn't, so need to compare column sets
                        let mut table_cols: Vec<_> = self.schema.columns().iter().collect();
                        let mut record_cols: Vec<_> = record.columns().collect();
                        table_cols.sort();
                        record_cols.sort();
                        return table_cols.partial_cmp(&record_cols);
                    }
                }
            }
        }

        Some(Ordering::Equal)
    }
}

impl PartialEq for TableData {
    fn eq(&self, other: &Self) -> bool {
        // Compare schemas (column names must match in order)
        if self.schema.columns() != other.schema.columns() {
            return false;
        }
        // Compare row data
        self.rows == other.rows
    }
}

impl Eq for TableData {}

impl PartialOrd for TableData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // First compare schemas lexicographically
        match self.schema.columns().partial_cmp(other.schema.columns()) {
            Some(std::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        // Then compare rows
        self.rows.partial_cmp(&other.rows)
    }
}

// Custom serialization to match List<Record> format for backward compatibility
impl Serialize for TableData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as a struct with schema and rows
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TableData", 2)?;
        state.serialize_field("schema", &self.schema)?;
        state.serialize_field("rows", &self.rows)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TableData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TableDataHelper {
            schema: TableSchema,
            rows: Vec<Vec<Value>>,
        }

        let helper = TableDataHelper::deserialize(deserializer)?;
        let schema_index = Self::build_schema_index(&helper.schema);

        Ok(Self {
            schema: helper.schema,
            schema_index,
            rows: helper.rows,
        })
    }
}

/// Errors that can occur when working with TableData.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableDataError {
    /// The number of values in a row doesn't match the schema.
    ColumnCountMismatch {
        expected: usize,
        got: usize,
        row_index: usize,
    },
    /// Cannot set schema on a table that already has rows.
    CannotSetSchemaWithRows { row_count: usize },
}

impl std::fmt::Display for TableDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TableDataError::ColumnCountMismatch {
                expected,
                got,
                row_index,
            } => {
                write!(
                    f,
                    "row {} has {} values, but schema has {} columns",
                    row_index, got, expected
                )
            }
            TableDataError::CannotSetSchemaWithRows { row_count } => {
                write!(
                    f,
                    "cannot set schema on table with {} existing rows",
                    row_count
                )
            }
        }
    }
}

impl std::error::Error for TableDataError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_schema() -> TableSchema {
        TableSchema::new(vec!["a".into(), "b".into(), "c".into()])
    }

    #[test]
    fn test_table_creation() {
        let schema = test_schema();
        let table = TableData::new(schema.clone());

        assert_eq!(table.num_columns(), 3);
        assert_eq!(table.len(), 0);
        assert!(table.is_empty());
        assert_eq!(table.columns(), &["a", "b", "c"]);
    }

    #[test]
    fn test_push_row() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        table
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();

        assert_eq!(table.len(), 1);
        assert!(!table.is_empty());
    }

    #[test]
    fn test_push_row_wrong_size() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        let result = table.push_row(vec![Value::test_int(1), Value::test_int(2)]);
        assert!(result.is_err());

        match result {
            Err(TableDataError::ColumnCountMismatch {
                expected,
                got,
                row_index,
            }) => {
                assert_eq!(expected, 3);
                assert_eq!(got, 2);
                assert_eq!(row_index, 0);
            }
            _ => panic!("Expected ColumnCountMismatch error"),
        }
    }

    #[test]
    fn test_column_index() {
        let schema = test_schema();
        let table = TableData::new(schema);

        assert_eq!(table.column_index("a"), Some(0));
        assert_eq!(table.column_index("b"), Some(1));
        assert_eq!(table.column_index("c"), Some(2));
        assert_eq!(table.column_index("d"), None);
    }

    #[test]
    fn test_get_row() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        table
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();
        table
            .push_row(vec![
                Value::test_int(4),
                Value::test_int(5),
                Value::test_int(6),
            ])
            .unwrap();

        let row = table.get_row(0).unwrap();
        assert_eq!(row.get("a"), Some(&Value::test_int(1)));
        assert_eq!(row.get("b"), Some(&Value::test_int(2)));
        assert_eq!(row.get("c"), Some(&Value::test_int(3)));

        let row = table.get_row(1).unwrap();
        assert_eq!(row.get("a"), Some(&Value::test_int(4)));

        assert!(table.get_row(2).is_none());
    }

    #[test]
    fn test_get_column() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        table
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();
        table
            .push_row(vec![
                Value::test_int(4),
                Value::test_int(5),
                Value::test_int(6),
            ])
            .unwrap();

        let col = table.get_column("a").unwrap();
        assert_eq!(col.len(), 2);
        assert_eq!(col[0], &Value::test_int(1));
        assert_eq!(col[1], &Value::test_int(4));

        assert!(table.get_column("nonexistent").is_none());
    }

    #[test]
    fn test_to_list() {
        let schema = TableSchema::new(vec!["x".into(), "y".into()]);
        let mut table = TableData::new(schema);

        table
            .push_row(vec![Value::test_int(1), Value::test_int(2)])
            .unwrap();

        let list = table.to_list(Span::test_data());
        if let Value::List { vals, .. } = list {
            assert_eq!(vals.len(), 1);
            if let Value::Record { val, .. } = &vals[0] {
                assert_eq!(val.get("x"), Some(&Value::test_int(1)));
                assert_eq!(val.get("y"), Some(&Value::test_int(2)));
            } else {
                panic!("Expected Record value");
            }
        } else {
            panic!("Expected List value");
        }
    }

    #[test]
    fn test_from_list() {
        let records = vec![
            Value::test_record(Record::from_iter([
                ("a".to_string(), Value::test_int(1)),
                ("b".to_string(), Value::test_int(2)),
            ])),
            Value::test_record(Record::from_iter([
                ("a".to_string(), Value::test_int(3)),
                ("b".to_string(), Value::test_int(4)),
            ])),
        ];

        let table = TableData::from_list(&records).unwrap();
        assert_eq!(table.num_columns(), 2);
        assert_eq!(table.len(), 2);

        let row = table.get_row(0).unwrap();
        assert_eq!(row.get("a"), Some(&Value::test_int(1)));
        assert_eq!(row.get("b"), Some(&Value::test_int(2)));
    }

    #[test]
    fn test_from_list_empty() {
        let records: Vec<Value> = vec![];
        assert!(TableData::from_list(&records).is_none());
    }

    #[test]
    fn test_from_list_inconsistent_columns() {
        let records = vec![
            Value::test_record(Record::from_iter([
                ("a".to_string(), Value::test_int(1)),
                ("b".to_string(), Value::test_int(2)),
            ])),
            Value::test_record(Record::from_iter([
                ("a".to_string(), Value::test_int(3)),
                ("c".to_string(), Value::test_int(4)), // Different column
            ])),
        ];

        assert!(TableData::from_list(&records).is_none());
    }

    #[test]
    fn test_select_columns() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        table
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();

        let selected = table.select_columns(&["a", "c"]);
        assert_eq!(selected.num_columns(), 2);
        assert_eq!(selected.columns(), &["a", "c"]);

        let row = selected.get_row(0).unwrap();
        assert_eq!(row.get("a"), Some(&Value::test_int(1)));
        assert_eq!(row.get("c"), Some(&Value::test_int(3)));
    }

    #[test]
    fn test_take_skip() {
        let schema = TableSchema::new(vec!["x".into()]);
        let mut table = TableData::new(schema);

        for i in 0..5 {
            table.push_row(vec![Value::test_int(i)]).unwrap();
        }

        let taken = table.take(3);
        assert_eq!(taken.len(), 3);

        let skipped = table.skip(2);
        assert_eq!(skipped.len(), 3);
    }

    #[test]
    fn test_reverse() {
        let schema = TableSchema::new(vec!["x".into()]);
        let mut table = TableData::new(schema);

        table.push_row(vec![Value::test_int(1)]).unwrap();
        table.push_row(vec![Value::test_int(2)]).unwrap();
        table.push_row(vec![Value::test_int(3)]).unwrap();

        let reversed = table.reverse();
        assert_eq!(reversed.get_row(0).unwrap().get("x"), Some(&Value::test_int(3)));
        assert_eq!(reversed.get_row(2).unwrap().get("x"), Some(&Value::test_int(1)));
    }

    #[test]
    fn test_equality() {
        let schema = test_schema();
        let mut table1 = TableData::new(schema.clone());
        let mut table2 = TableData::new(schema);

        table1
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();
        table2
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();

        assert_eq!(table1, table2);
    }

    #[test]
    fn test_get_type() {
        let schema = TableSchema::new(vec!["a".into(), "b".into()]);
        let mut table = TableData::new(schema);

        table
            .push_row(vec![Value::test_int(1), Value::test_string("hello")])
            .unwrap();

        let ty = table.get_type();
        if let Type::Table(cols) = ty {
            assert_eq!(cols.len(), 2);
            assert_eq!(cols[0], ("a".to_string(), Type::Int));
            assert_eq!(cols[1], ("b".to_string(), Type::String));
        } else {
            panic!("Expected Type::Table");
        }
    }

    #[test]
    fn test_serde_roundtrip() {
        let schema = test_schema();
        let mut table = TableData::new(schema);

        table
            .push_row(vec![
                Value::test_int(1),
                Value::test_int(2),
                Value::test_int(3),
            ])
            .unwrap();

        let serialized = serde_json::to_string(&table).unwrap();
        let deserialized: TableData = serde_json::from_str(&serialized).unwrap();

        assert_eq!(table, deserialized);
        // Verify schema_index was rebuilt
        assert_eq!(deserialized.column_index("a"), Some(0));
        assert_eq!(deserialized.column_index("b"), Some(1));
    }
}
