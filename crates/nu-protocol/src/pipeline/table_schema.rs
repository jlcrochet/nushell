//! Table schema for sharing column names across records
//!
//! When processing tables (lists of records with the same columns), each record
//! traditionally stores its own copy of column names. `TableSchema` allows storing
//! column names once and sharing them, reducing memory overhead and providing
//! fast column information for display.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Record, Span, Value};

/// A shared table schema containing column names.
///
/// This type uses `Arc` internally, making cloning extremely cheap (O(1) atomic increment).
/// Use this when creating tables where all rows share the same column structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    columns: Arc<[String]>,
}

impl TableSchema {
    /// Create a new schema from column names.
    pub fn new(columns: Vec<String>) -> Self {
        Self {
            columns: columns.into(),
        }
    }

    /// Create a schema from an iterator of column names.
    pub fn from_columns(columns: impl IntoIterator<Item = String>) -> Self {
        Self {
            columns: columns.into_iter().collect(),
        }
    }

    /// Get the column names as a slice.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Consume the schema and return the column names.
    pub fn into_columns(self) -> Arc<[String]> {
        self.columns
    }

    /// Get the number of columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Check if the schema has no columns.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Create a record from values using this schema.
    ///
    /// The values are paired with column names positionally. If there are more
    /// values than columns, extra values are dropped. If there are fewer values
    /// than columns, missing values become Nothing.
    pub fn make_record(&self, values: impl IntoIterator<Item = Value>, span: Span) -> Value {
        let mut vals: Vec<Value> = values.into_iter().collect();
        let n_cols = self.columns.len();

        // Pad with Nothing if we have fewer values than columns
        vals.resize_with(n_cols, || Value::nothing(span));
        // Truncate if we have more values than columns
        vals.truncate(n_cols);

        let record: Record = self
            .columns
            .iter()
            .cloned()
            .zip(vals)
            .collect();

        Value::record(record, span)
    }

    /// Create an iterator that yields (column_name, index) pairs.
    pub fn iter_indexed(&self) -> impl Iterator<Item = (&str, usize)> {
        self.columns.iter().map(|s| s.as_str()).zip(0..)
    }
}

impl Default for TableSchema {
    fn default() -> Self {
        Self {
            columns: Arc::new([]),
        }
    }
}

impl From<Vec<String>> for TableSchema {
    fn from(columns: Vec<String>) -> Self {
        Self::new(columns)
    }
}

impl From<&[String]> for TableSchema {
    fn from(columns: &[String]) -> Self {
        Self {
            columns: columns.into(),
        }
    }
}

impl From<&[&str]> for TableSchema {
    fn from(columns: &[&str]) -> Self {
        Self {
            columns: columns.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        let schema = TableSchema::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(schema.len(), 3);
        assert_eq!(schema.columns(), &["a", "b", "c"]);
    }

    #[test]
    fn test_schema_clone_is_cheap() {
        let schema1 = TableSchema::new(vec!["col1".into(), "col2".into()]);
        let schema2 = schema1.clone();

        // Both should point to the same underlying data (same Arc pointer)
        assert!(std::ptr::eq(
            schema1.columns.as_ptr(),
            schema2.columns.as_ptr()
        ));
    }

    #[test]
    fn test_make_record() {
        let schema = TableSchema::new(vec!["x".into(), "y".into()]);
        let span = Span::test_data();
        let record = schema.make_record(vec![Value::test_int(1), Value::test_int(2)], span);

        if let Value::Record { val, .. } = record {
            assert_eq!(val.len(), 2);
            assert_eq!(val.get("x"), Some(&Value::test_int(1)));
            assert_eq!(val.get("y"), Some(&Value::test_int(2)));
        } else {
            panic!("Expected Record value");
        }
    }

    #[test]
    fn test_make_record_pads_missing() {
        let schema = TableSchema::new(vec!["a".into(), "b".into(), "c".into()]);
        let span = Span::test_data();
        let record = schema.make_record(vec![Value::test_int(1)], span);

        if let Value::Record { val, .. } = record {
            assert_eq!(val.len(), 3);
            assert_eq!(val.get("a"), Some(&Value::test_int(1)));
            assert!(val.get("b").unwrap().is_nothing());
            assert!(val.get("c").unwrap().is_nothing());
        } else {
            panic!("Expected Record value");
        }
    }
}
