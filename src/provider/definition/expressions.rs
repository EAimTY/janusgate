//! Concrete structural ownership of descriptor expressions, shared by validation and references.

use super::template::{ControlBody, ControlRequest, ParamRow, StringExpr, TemplateNode, ValueExpr};

/// One borrowed descriptor surface whose structure is inspected without evaluating it.
#[derive(Clone, Copy)]
pub(super) enum ExpressionSurface<'a> {
    /// A concatenated string expression.
    String(&'a StringExpr),
    /// Literal HTTP header names and their expression values.
    Headers(&'a [ParamRow]),
    /// Percent-encoded parameter names and their expression values.
    Parameters(&'a [ParamRow]),
    /// A JSON template; literal JSON stays opaque.
    Template(&'a TemplateNode),
    /// All expression-bearing members of one control request.
    Control(&'a ControlRequest),
}

impl<'a> ExpressionSurface<'a> {
    /// Enumerates structural members in descriptor order, preserving rejection precedence.
    pub(super) fn members(self) -> ExpressionMembers<'a> {
        ExpressionMembers {
            pending: vec![Pending::Surface(self)],
        }
    }

    /// Whether a value appears directly or among a coalesce's direct arguments.
    pub(super) fn references(self, sought: &ValueExpr) -> bool {
        self.members()
            .any(|member| matches!(member, ExpressionMember::Value(value) if value == sought))
    }
}

/// The structural facts consumers inspect; stage and syntax rules belong to validation.
#[derive(Clone, Copy)]
pub(super) enum ExpressionMember<'a> {
    /// A value part, including coalesce arguments.
    Value(&'a ValueExpr),
    /// A literal HTTP header-row name.
    HeaderName(&'a str),
    /// A percent-encoded parameter-row name.
    ParameterName(&'a str),
    /// A literal JSON-template object member name.
    ObjectMemberName(&'a str),
    /// A codec body placeholder.
    Body,
    /// A coalesce has no arguments.
    EmptyCoalesce,
    /// A coalesce argument is itself a coalesce.
    NestedCoalesce,
}

/// Concrete expansion work for the borrowed descriptor iterator.
enum Pending<'a> {
    /// A descriptor surface awaiting expansion.
    Surface(ExpressionSurface<'a>),
    /// A value and whether it is a direct argument of a coalesce.
    Value(&'a ValueExpr, bool),
    /// A structural fact ready for inspection.
    Member(ExpressionMember<'a>),
}

/// A depth-first descriptor iterator; it owns only borrowed traversal work.
pub(super) struct ExpressionMembers<'a> {
    /// Remaining work in reverse inspection order.
    pending: Vec<Pending<'a>>,
}

impl<'a> Iterator for ExpressionMembers<'a> {
    type Item = ExpressionMember<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(pending) = self.pending.pop() {
            match pending {
                Pending::Member(member) => return Some(member),
                Pending::Value(value, is_coalesce_argument) => {
                    if let ValueExpr::Coalesce { parts } = value {
                        if is_coalesce_argument {
                            return Some(ExpressionMember::NestedCoalesce);
                        }
                        if parts.is_empty() {
                            self.pending
                                .push(Pending::Member(ExpressionMember::EmptyCoalesce));
                        } else {
                            self.pending
                                .extend(parts.iter().rev().map(|part| Pending::Value(part, true)));
                        }
                    }
                    return Some(ExpressionMember::Value(value));
                }
                Pending::Surface(surface) => match surface {
                    ExpressionSurface::String(expr) => self.pending.extend(
                        expr.0
                            .iter()
                            .rev()
                            .map(|value| Pending::Value(value, false)),
                    ),
                    ExpressionSurface::Headers(rows) | ExpressionSurface::Parameters(rows) => {
                        for row in rows.iter().rev() {
                            self.pending
                                .push(Pending::Surface(ExpressionSurface::String(&row.value)));
                            self.pending.push(Pending::Member(match surface {
                                ExpressionSurface::Headers(_) => {
                                    ExpressionMember::HeaderName(&row.name)
                                }
                                _ => ExpressionMember::ParameterName(&row.name),
                            }));
                        }
                    }
                    ExpressionSurface::Template(node) => match node {
                        TemplateNode::Json { .. } => {}
                        TemplateNode::Body => return Some(ExpressionMember::Body),
                        TemplateNode::String { expr } => self
                            .pending
                            .push(Pending::Surface(ExpressionSurface::String(expr))),
                        TemplateNode::Object { fields } => {
                            for (name, field) in fields.iter().rev() {
                                self.pending
                                    .push(Pending::Surface(ExpressionSurface::Template(field)));
                                self.pending.push(Pending::Member(
                                    ExpressionMember::ObjectMemberName(name),
                                ));
                            }
                        }
                        TemplateNode::Array { items } => self.pending.extend(
                            items
                                .iter()
                                .rev()
                                .map(|node| Pending::Surface(ExpressionSurface::Template(node))),
                        ),
                    },
                    ExpressionSurface::Control(request) => {
                        match &request.body {
                            None => {}
                            Some(ControlBody::Form { fields }) => self
                                .pending
                                .push(Pending::Surface(ExpressionSurface::Parameters(fields))),
                            Some(ControlBody::Json { node }) => self
                                .pending
                                .push(Pending::Surface(ExpressionSurface::Template(node))),
                        }
                        self.pending
                            .push(Pending::Surface(ExpressionSurface::Parameters(
                                &request.query,
                            )));
                        self.pending
                            .push(Pending::Surface(ExpressionSurface::Headers(
                                &request.headers,
                            )));
                        self.pending
                            .push(Pending::Surface(ExpressionSurface::String(&request.url)));
                    }
                },
            }
        }
        None
    }
}
