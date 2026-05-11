use std::borrow::Cow;

use anyhow::{anyhow, Result};
use lora_analyzer::symbols::VarId;
use lora_ast::YieldItem;
use lora_executor::{LoraValue, Row};

#[derive(Debug, Clone)]
pub(crate) struct NamedColumn<'a> {
    name: Cow<'a, str>,
    value: LoraValue,
}

impl<'a> NamedColumn<'a> {
    pub(crate) fn new(name: impl Into<Cow<'a, str>>, value: LoraValue) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ColumnLookupContext {
    ProcedureYield,
    ShowPipeline,
}

impl ColumnLookupContext {
    const fn description(self) -> &'static str {
        match self {
            ColumnLookupContext::ProcedureYield => "procedure YIELD",
            ColumnLookupContext::ShowPipeline => "SHOW pipeline",
        }
    }
}

pub(crate) fn row_from_columns<'a>(columns: impl IntoIterator<Item = NamedColumn<'a>>) -> Row {
    let mut row = Row::new();
    for (idx, column) in columns.into_iter().enumerate() {
        row.insert_named(VarId(idx as u32), column.name.into_owned(), column.value);
    }
    row
}

pub(crate) fn lookup_column(
    row: &Row,
    name: &str,
    context: ColumnLookupContext,
) -> Result<LoraValue> {
    for (_, column_name, value) in row.iter_named() {
        if column_name.as_ref() == name {
            return Ok(value.clone());
        }
    }
    Err(anyhow!(
        "unknown column `{name}` in {}",
        context.description()
    ))
}

pub(crate) fn project_yield_items(
    rows: Vec<Row>,
    items: &[YieldItem],
    yield_all: bool,
    context: ColumnLookupContext,
) -> Result<Vec<Row>> {
    if yield_all || items.is_empty() {
        return Ok(rows);
    }

    rows.into_iter()
        .map(|row| {
            items
                .iter()
                .map(|item| {
                    let source = item.field.as_deref().unwrap_or(&item.alias.name);
                    lookup_column(&row, source, context)
                        .map(|value| NamedColumn::new(item.alias.name.clone(), value))
                })
                .collect::<Result<Vec<_>>>()
                .map(row_from_columns)
        })
        .collect()
}
