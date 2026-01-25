use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Record;

use super::TableSchema;

/// Metadata that is valid for the whole [`PipelineData`](crate::PipelineData)
///
/// ## Custom Metadata
///
/// The `custom` field allows commands and plugins to attach arbitrary metadata to pipeline data.
/// To avoid key collisions, it is recommended to use namespaced keys with an underscore separator:
///
/// - `"http_response"` - HTTP response metadata (status, headers, etc.)
/// - `"polars_schema"` - DataFrame schema information
/// - `"custom_plugin_field"` - Plugin-specific metadata
///
/// This convention helps ensure different commands and plugins don't overwrite each other's metadata.
///
/// ## Table Schema
///
/// The `table_schema` field stores the column names for table data (lists of records).
/// When set, this provides fast column information without scanning the data, and
/// enables memory optimization through schema sharing.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PipelineMetadata {
    pub data_source: DataSource,
    pub content_type: Option<String>,
    #[serde(default)]
    pub custom: Record,
    /// Schema for table data (column names shared across all records)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_schema: Option<TableSchema>,
}

impl PipelineMetadata {
    pub fn with_data_source(self, data_source: DataSource) -> Self {
        Self {
            data_source,
            ..self
        }
    }

    pub fn with_content_type(self, content_type: Option<String>) -> Self {
        Self {
            content_type,
            ..self
        }
    }

    /// Transform metadata for the `collect` operation.
    ///
    /// After collecting a stream into a value, `FilePath` data sources are no longer meaningful
    /// and should be converted to `None`. If all metadata fields become empty after this
    /// transformation, returns `None` to avoid carrying around empty metadata.
    pub fn for_collect(self) -> Option<Self> {
        let Self {
            data_source,
            content_type,
            custom,
            table_schema,
        } = self;

        // Transform FilePath to None after collect
        let data_source = match data_source {
            DataSource::FilePath(_) => DataSource::None,
            other => other,
        };

        // Return None if completely empty
        if matches!(data_source, DataSource::None)
            && content_type.is_none()
            && custom.is_empty()
            && table_schema.is_none()
        {
            None
        } else {
            Some(Self {
                data_source,
                content_type,
                custom,
                table_schema,
            })
        }
    }

    /// Set the table schema for this metadata.
    pub fn with_table_schema(self, schema: Option<TableSchema>) -> Self {
        Self {
            table_schema: schema,
            ..self
        }
    }
}

/// Describes where the particular [`PipelineMetadata`] originates.
///
/// This can either be a particular family of commands (useful so downstream commands can adjust
/// the presentation e.g. `Ls`) or the opened file to protect against overwrite-attempts properly.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub enum DataSource {
    Ls,
    HtmlThemes,
    FilePath(PathBuf),
    #[default]
    None,
}
