use csv::{ReaderBuilder, Trim};
use nu_protocol::{
    ByteStream, IntoPipelineData, ListStream, PipelineData, PipelineMetadata, ShellError, Signals,
    Span, TableData, TableSchema, Value,
};

fn from_csv_error(err: csv::Error, span: Span) -> ShellError {
    ShellError::DelimiterError {
        msg: err.to_string(),
        span,
    }
}

/// Result of parsing delimited data: the stream and optional schema
struct DelimitedResult {
    stream: ListStream,
    schema: Option<TableSchema>,
}

/// Collect a ListStream into a Value::Table when schema is known
fn collect_into_table(
    stream: ListStream,
    schema: TableSchema,
    span: Span,
) -> Result<Value, ShellError> {
    let mut table = TableData::new(schema);
    let num_columns = table.num_columns();

    for value in stream {
        // Extract values from the record in column order
        if let Value::Record { val: record, .. } = value {
            // Optimization: CSV parser creates records with values in header order,
            // so if column counts match, we can take values directly (O(n) vs O(n²))
            let row_values: Vec<Value> = if record.len() == num_columns {
                // Fast path: take values in order (assumes same column order as schema)
                record.into_owned().into_iter().map(|(_, v)| v).collect()
            } else {
                // Slow path: lookup by column name (handles missing/extra columns)
                table
                    .columns()
                    .iter()
                    .map(|col| {
                        record
                            .get(col)
                            .cloned()
                            .unwrap_or_else(|| Value::nothing(span))
                    })
                    .collect()
            };
            table
                .push_row(row_values)
                .map_err(|e| ShellError::GenericError {
                    error: "Failed to build table".into(),
                    msg: e.to_string(),
                    span: Some(span),
                    help: None,
                    inner: vec![],
                })?;
        } else if let Value::Error { error, .. } = value {
            return Err(error.as_ref().clone());
        }
    }
    Ok(Value::table(table, span))
}

fn from_delimited_stream(
    DelimitedReaderConfig {
        separator,
        comment,
        quote,
        escape,
        noheaders,
        flexible,
        no_infer,
        trim,
    }: DelimitedReaderConfig,
    input: ByteStream,
    span: Span,
) -> Result<DelimitedResult, ShellError> {
    let input_reader = if let Some(stream) = input.reader() {
        stream
    } else {
        return Ok(DelimitedResult {
            stream: ListStream::new(std::iter::empty(), span, Signals::empty()),
            schema: None,
        });
    };

    let mut reader = ReaderBuilder::new()
        .has_headers(!noheaders)
        .flexible(flexible)
        .delimiter(separator as u8)
        .comment(comment.map(|c| c as u8))
        .quote(quote as u8)
        .escape(escape.map(|c| c as u8))
        .trim(trim)
        .from_reader(input_reader);

    let headers: Vec<String> = if noheaders {
        vec![]
    } else {
        reader
            .headers()
            .map_err(|err| from_csv_error(err, span))?
            .iter()
            .map(String::from)
            .collect()
    };

    // Create schema from headers if present
    let schema = if headers.is_empty() {
        None
    } else {
        Some(TableSchema::new(headers.clone()))
    };

    let n = headers.len();
    let columns = headers
        .into_iter()
        .chain((n..).map(|i| format!("column{i}")));
    let iter = reader.into_records().map(move |row| {
        let row = match row {
            Ok(row) => row,
            Err(err) => return Value::error(from_csv_error(err, span), span),
        };
        let columns = columns.clone();
        let values = row.into_iter().map(|s| {
            if no_infer {
                Value::string(s, span)
            } else if let Ok(i) = s.parse() {
                Value::int(i, span)
            } else if let Ok(f) = s.parse() {
                Value::float(f, span)
            } else {
                Value::string(s, span)
            }
        });

        Value::record(columns.zip(values).collect(), span)
    });

    Ok(DelimitedResult {
        stream: ListStream::new(iter, span, Signals::empty()),
        schema,
    })
}

pub(super) struct DelimitedReaderConfig {
    pub separator: char,
    pub comment: Option<char>,
    pub quote: char,
    pub escape: Option<char>,
    pub noheaders: bool,
    pub flexible: bool,
    pub no_infer: bool,
    pub trim: Trim,
}

pub(super) fn from_delimited_data(
    config: DelimitedReaderConfig,
    input: PipelineData,
    name: Span,
) -> Result<PipelineData, ShellError> {
    let base_metadata = input.metadata().map(|md| md.with_content_type(None));

    // Determine if we can produce a Table (has headers and not flexible)
    let can_produce_table = !config.noheaders && !config.flexible;

    // Helper to merge schema into metadata
    let with_schema = |schema: Option<TableSchema>| -> Option<PipelineMetadata> {
        match (base_metadata.clone(), schema) {
            (Some(md), schema) => Some(md.with_table_schema(schema)),
            (None, Some(schema)) => {
                Some(PipelineMetadata::default().with_table_schema(Some(schema)))
            }
            (None, None) => None,
        }
    };

    match input {
        PipelineData::Empty => Ok(PipelineData::empty()),
        PipelineData::Value(value, ..) => {
            let string = value.into_string()?;
            let byte_stream = ByteStream::read_string(string, name, Signals::empty());
            let result = from_delimited_stream(config, byte_stream, name)?;

            // For Value inputs (typically small data), collect into Table when possible
            if can_produce_table {
                if let Some(schema) = result.schema {
                    let table = collect_into_table(result.stream, schema, name)?;
                    return Ok(table.into_pipeline_data_with_metadata(base_metadata));
                }
            }

            Ok(PipelineData::list_stream(
                result.stream,
                with_schema(result.schema),
            ))
        }
        PipelineData::ListStream(list_stream, _) => Err(ShellError::OnlySupportsThisInputType {
            exp_input_type: "string".into(),
            wrong_type: "list".into(),
            dst_span: name,
            src_span: list_stream.span(),
        }),
        PipelineData::ByteStream(byte_stream, ..) => {
            let result = from_delimited_stream(config, byte_stream, name)?;
            // For ByteStream inputs (potentially large files), keep streaming
            // but set schema in metadata for downstream commands
            Ok(PipelineData::list_stream(
                result.stream,
                with_schema(result.schema),
            ))
        }
    }
}

pub fn trim_from_str(trim: Option<Value>) -> Result<Trim, ShellError> {
    match trim {
        Some(v) => {
            let span = v.span();
            match v {
                Value::String {val: item, ..} => match item.as_str() {

            "all" => Ok(Trim::All),
            "headers" => Ok(Trim::Headers),
            "fields" => Ok(Trim::Fields),
            "none" => Ok(Trim::None),
            _ => Err(ShellError::TypeMismatch {
                err_message:
                    "the only possible values for trim are 'all', 'headers', 'fields' and 'none'"
                        .into(),
                span,
            }),
                }
                _ => Ok(Trim::None),
            }
        }
        _ => Ok(Trim::None),
    }
}
