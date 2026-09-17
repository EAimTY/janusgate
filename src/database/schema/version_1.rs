//! Schema version 1: the initial database layout.
//!
//! Version 1 creates every current object — the `providers`, `models`, and `oauth_states` tables
//! and the `idx_oauth_states_expires_at` index — so its [`Schema::migration`] is empty: there is no
//! previous version to migrate from. A fresh database receives this version's DDL directly, and
//! every later version's migration transforms it.

use super::Schema;

/// The schema of version 1.
///
/// The DDL is constructed at compile time by `turso-ddl-builder` inside a `crabtime` expression:
/// the annotated function is evaluated during compilation and its output — the SQL text — becomes
/// the string literal stored here. The builders lower into `turso_parser`'s AST and the SQL text
/// always comes from `turso_parser` itself, so the generated statements are guaranteed to parse
/// back into themselves. The `crabtime` project takes its dependencies — the DDL builder and the
/// parser — from the `[build-dependencies]` section of `Cargo.toml`.
pub(super) const SCHEMA: Schema = Schema {
    version: 1,

    schema: {
        #[crabtime::expression]
        fn ddl() -> String {
            use turso_ddl_builder::{
                Column, ColumnConstraint, ColumnType, Expr, ForeignKeyClause, Index, IndexedColumn,
                Literal, Name, Operator, Table,
            };
            use turso_parser::ast::{RefAct, RefArg};

            /// Builds an identifier expression referencing a column by name.
            fn column_reference(name: &str) -> Expr {
                Expr::Id(Name::from_string(name))
            }

            /// Builds a string literal expression; `turso_parser` represents the literal payload as
            /// the SQL-quoted text itself.
            fn string_literal(value: &str) -> Expr {
                Expr::Literal(Literal::String(format!("'{value}'")))
            }

            /// Builds a numeric literal expression.
            fn numeric_literal(value: i64) -> Expr {
                Expr::Literal(Literal::Numeric(value.to_string()))
            }

            /// Builds a `column IN ('value', ...)` check expression over string values.
            fn in_strings(column: &str, values: &[&str]) -> Expr {
                Expr::InList {
                    lhs: Box::new(column_reference(column)),
                    not: false,
                    rhs: values
                        .iter()
                        .map(|value| Box::new(string_literal(value)))
                        .collect(),
                }
            }

            /// Builds a `column IN (value, ...)` check expression over numeric values.
            fn in_numbers(column: &str, values: &[i64]) -> Expr {
                Expr::InList {
                    lhs: Box::new(column_reference(column)),
                    not: false,
                    rhs: values
                        .iter()
                        .map(|value| Box::new(numeric_literal(*value)))
                        .collect(),
                }
            }

            /// Builds the explicit `NULL` column constraint, the storage-level spelling of
            /// nullability this schema declares on every nullable column.
            fn nullable() -> ColumnConstraint {
                ColumnConstraint::NotNull {
                    nullable: true,
                    conflict_clause: None,
                }
            }

            /// Builds the `references providers(id) on delete cascade` clause of `models`.
            fn provider_reference() -> ForeignKeyClause {
                ForeignKeyClause {
                    tbl_name: Name::from_string("providers"),
                    columns: vec![IndexedColumn {
                        col_name: Name::from_string("id"),
                        collation_name: None,
                        order: None,
                    }],
                    args: vec![RefArg::OnDelete(RefAct::Cascade)],
                }
            }

            let providers = Table::new("providers")
                .strict()
                .column(
                    Column::new("id", ColumnType::Integer)
                        .primary_key()
                        .not_null(),
                )
                .column(
                    Column::new("name", ColumnType::Text)
                        .not_null()
                        .unique()
                        .check(Expr::Binary(
                            Box::new(column_reference("name")),
                            Operator::NotEquals,
                            Box::new(string_literal("")),
                        )),
                )
                .column(
                    Column::new("is_enabled", ColumnType::Integer)
                        .not_null()
                        .default(numeric_literal(1))
                        .check(in_numbers("is_enabled", &[0, 1])),
                )
                .column(Column::new("model_prefix", ColumnType::Text).not_null())
                .column(
                    Column::new("type", ColumnType::Text)
                        .not_null()
                        .check(in_strings(
                            "type",
                            &["openai_oauth", "antigravity_oauth", "openai_compatible"],
                        )),
                )
                .column(
                    Column::new("protocol", ColumnType::Text)
                        .constraint(nullable())
                        .check(Expr::Binary(
                            Box::new(Expr::IsNull(Box::new(column_reference("protocol")))),
                            Operator::Or,
                            Box::new(in_strings(
                                "protocol",
                                &[
                                    "openai_response",
                                    "openai_chat_completion",
                                    "gemini_generate_content",
                                ],
                            )),
                        )),
                )
                .column(Column::new("base_url", ColumnType::Text).constraint(nullable()))
                .column(Column::new("credentials", ColumnType::Text).not_null())
                .column(Column::new("expires_at", ColumnType::Integer).constraint(nullable()))
                .column(
                    Column::new("refresh_started_at", ColumnType::Integer).constraint(nullable()),
                )
                .build();

            let models = Table::new("models")
                .strict()
                .column(
                    Column::new("provider_id", ColumnType::Integer)
                        .not_null()
                        .references(provider_reference()),
                )
                .column(Column::new("upstream_id", ColumnType::Text).not_null())
                .column(
                    Column::new("is_enabled", ColumnType::Integer)
                        .not_null()
                        .default(numeric_literal(1))
                        .check(in_numbers("is_enabled", &[0, 1])),
                )
                .column(Column::new("id", ColumnType::Text).not_null().unique())
                .column(Column::new("id_override", ColumnType::Text).constraint(nullable()))
                .column(
                    Column::new("thinking_levels_override", ColumnType::Text)
                        .constraint(nullable()),
                )
                .primary_key(["provider_id", "upstream_id"])
                .build();

            let oauth_states = Table::new("oauth_states")
                .strict()
                .column(
                    Column::new("id", ColumnType::Integer)
                        .primary_key()
                        .not_null(),
                )
                .column(
                    Column::new("type", ColumnType::Text)
                        .not_null()
                        .check(in_strings("type", &["openai_oauth", "antigravity_oauth"])),
                )
                .column(Column::new("state", ColumnType::Text).not_null().unique())
                .column(Column::new("verifier", ColumnType::Text).not_null())
                .column(Column::new("expires_at", ColumnType::Integer).not_null())
                .build();

            let oauth_states_expires_at_index =
                Index::new("idx_oauth_states_expires_at", "oauth_states")
                    .column("expires_at")
                    .build();

            // Every statement is terminated by ";\n" so schema texts concatenate safely into the
            // bootstrap batch, which appends the user_version write before COMMIT.
            let mut ddl = String::new();
            for statement in [
                providers,
                models,
                oauth_states,
                oauth_states_expires_at_index,
            ] {
                ddl.push_str(&statement);
                ddl.push_str(";\n");
            }

            // Debug-formatting the SQL quotes and escapes it into a Rust string literal, which is
            // what this compile-time expression must output.
            format!("{ddl:?}")
        }

        ddl!()
    },

    migration: "",
};
