//! Source-neutral function-pack validation failures and TOML locators.

use std::{fmt, ops::Range};

use toml_edit::{Document, Item, Table};

use super::{ReviewedContext, ReviewedContextField, ReviewedFunction, ReviewedFunctionInput};

pub(super) type ValidationResult<T> = std::result::Result<T, ValidationError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ValidationError {
    message: String,
    location: Location,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Location {
    Pack {
        key: &'static str,
    },
    Input {
        profile: String,
        source: String,
        key: &'static str,
    },
    Function {
        profile: String,
        source: String,
        identity: String,
        key: &'static str,
    },
    Context {
        profile: String,
        source: String,
        identity: String,
        argument: u8,
        key: &'static str,
    },
    Field {
        profile: String,
        source: String,
        identity: String,
        argument: u8,
        offset: i32,
        width: u8,
        key: &'static str,
    },
}

impl ValidationError {
    pub(super) fn pack(key: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: Location::Pack { key },
        }
    }

    pub(super) fn input(
        input: &ReviewedFunctionInput,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Input {
                profile: input.profile.clone(),
                source: input.source.clone(),
                key,
            },
        }
    }

    pub(super) fn function(
        function: &ReviewedFunction,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: function_location(function, key),
        }
    }

    pub(super) fn context(
        function: &ReviewedFunction,
        context: &ReviewedContext,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Context {
                profile: function.profile.clone(),
                source: function.source.clone(),
                identity: function.identity.clone(),
                argument: context.argument,
                key,
            },
        }
    }

    pub(super) fn field(
        function: &ReviewedFunction,
        context: &ReviewedContext,
        field: &ReviewedContextField,
        key: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location: Location::Field {
                profile: function.profile.clone(),
                source: function.source.clone(),
                identity: function.identity.clone(),
                argument: context.argument,
                offset: field.offset,
                width: field.width,
                key,
            },
        }
    }

    pub(super) fn span(&self, document: &Document<String>) -> Option<Range<usize>> {
        match &self.location {
            Location::Pack { key } => item_or_container_span(document.as_item(), key),
            Location::Input {
                profile,
                source,
                key,
            } => {
                let table = document
                    .get("inputs")?
                    .as_array_of_tables()?
                    .iter()
                    .find(|table| {
                        table.get("profile").and_then(Item::as_str) == Some(profile)
                            && table.get("source").and_then(Item::as_str) == Some(source)
                    })?;
                table_item_or_span(table, key)
            }
            Location::Function {
                profile,
                source,
                identity,
                key,
            } => table_item_or_span(function_table(document, profile, source, identity)?, key),
            Location::Context {
                profile,
                source,
                identity,
                argument,
                key,
            } => {
                let function = function_table(document, profile, source, identity)?;
                let context = context_table(function, *argument)?;
                table_item_or_span(context, key)
            }
            Location::Field {
                profile,
                source,
                identity,
                argument,
                offset,
                width,
                key,
            } => {
                let function = function_table(document, profile, source, identity)?;
                let context = context_table(function, *argument)?;
                let field = context
                    .get("fields")?
                    .as_array_of_tables()?
                    .iter()
                    .find(|table| {
                        table.get("offset").and_then(Item::as_integer) == Some(i64::from(*offset))
                            && table.get("width").and_then(Item::as_integer)
                                == Some(i64::from(*width))
                    })?;
                table_item_or_span(field, key)
            }
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

fn function_location(function: &ReviewedFunction, key: &'static str) -> Location {
    Location::Function {
        profile: function.profile.clone(),
        source: function.source.clone(),
        identity: function.identity.clone(),
        key,
    }
}

fn function_table<'a>(
    document: &'a Document<String>,
    profile: &str,
    source: &str,
    identity: &str,
) -> Option<&'a Table> {
    document
        .get("functions")?
        .as_array_of_tables()?
        .iter()
        .find(|table| {
            table.get("profile").and_then(Item::as_str) == Some(profile)
                && table.get("source").and_then(Item::as_str) == Some(source)
                && table.get("identity").and_then(Item::as_str) == Some(identity)
        })
}

fn context_table(function: &Table, argument: u8) -> Option<&Table> {
    function
        .get("contexts")?
        .as_array_of_tables()?
        .iter()
        .find(|table| table.get("argument").and_then(Item::as_integer) == Some(i64::from(argument)))
}

fn item_or_container_span(item: &Item, key: &str) -> Option<Range<usize>> {
    item.get(key).and_then(Item::span).or_else(|| item.span())
}

fn table_item_or_span(table: &Table, key: &str) -> Option<Range<usize>> {
    table.get(key).and_then(Item::span).or_else(|| table.span())
}
