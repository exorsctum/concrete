use std::path::Path;

use crate::check::FileSpan;

use super::{
    ProgramSource,
    lexer::LexicalError,
    tokens::{self, Token},
};
use ariadne::{ColorGenerator, Label, Report, ReportKind};
use itertools::Itertools;
use lalrpop_util::ParseError;

pub type Error = ParseError<usize, Token, LexicalError>;

pub struct Diagnostic(pub Error);

impl Diagnostic {
    pub fn render(&self, source: &ProgramSource) {
        let path = source.path.display().to_string();
        let error = &self.0;

        let mut colors = ColorGenerator::new();
        let report = match error {
            ParseError::InvalidToken { location } => {
                let loc = *location;
                Report::build(ReportKind::Error, FileSpan::new(path.clone(), loc..loc))
                    .with_code("P1")
                    .with_message("Parse error.")
                    .with_label(
                        Label::new(FileSpan::new(path.clone(), loc..(loc + 1)))
                            .with_color(colors.next())
                            .with_message("invalid token"),
                    )
                    .with_label(
                        Label::new(FileSpan::new(
                            path.clone(),
                            (loc.saturating_sub(10))..(loc + 10),
                        ))
                        .with_message("There was a problem parsing part of this code."),
                    )
                    .finish()
            }
            ParseError::UnrecognizedEof { location, expected } => {
                let loc = *location;
                Report::build(ReportKind::Error, FileSpan::new(path.clone(), loc..loc))
                    .with_code("P2")
                    .with_message("Parse error.")
                    .with_label(
                        Label::new(FileSpan::new(path.clone(), loc..(loc + 1)))
                            .with_message("unrecognized eof")
                            .with_color(colors.next()),
                    )
                    .with_note(format!(
                        "expected one of the following: {}",
                        expected.iter().join(", ")
                    ))
                    .with_label(
                        Label::new(FileSpan::new(
                            path.clone(),
                            (loc.saturating_sub(10))..(loc + 10),
                        ))
                        .with_message("There was a problem parsing part of this code."),
                    )
                    .finish()
            }
            ParseError::UnrecognizedToken { token, expected } => Report::build(
                ReportKind::Error,
                FileSpan::new(path.clone(), token.0..token.2),
            )
            .with_code(3)
            .with_message("Parse error.")
            .with_label(
                Label::new(FileSpan::new(path.clone(), token.0..token.2))
                    .with_message(format!("unrecognized token '{:?}'", token.1))
                    .with_color(colors.next()),
            )
            .with_note(format!(
                "expected one of the following: {}",
                expected.iter().join(", ")
            ))
            .with_label(
                Label::new(FileSpan::new(
                    path.clone(),
                    (token.0.saturating_sub(10))..(token.2 + 10),
                ))
                .with_message("There was a problem parsing part of this code."),
            )
            .finish(),
            ParseError::ExtraToken { token } => Report::build(
                ReportKind::Error,
                FileSpan::new(path.clone(), token.0..token.2),
            )
            .with_code("P3")
            .with_message("Parse error.")
            .with_label(
                Label::new(FileSpan::new(path.clone(), token.0..token.2))
                    .with_message(format!("unexpected extra token {:?}", token.1)),
            )
            .finish(),
            ParseError::User { error } => match error {
                LexicalError::InvalidToken(err, range) => match err {
                    tokens::LexingError::NumberParseError => Report::build(
                        ReportKind::Error,
                        FileSpan::new(path.clone(), range.clone()),
                    )
                    .with_code(4)
                    .with_message("Error parsing literal number")
                    .with_label(
                        Label::new(FileSpan::new(path.clone(), range.clone()))
                            .with_message("error parsing literal number")
                            .with_color(colors.next()),
                    )
                    .finish(),
                    tokens::LexingError::Other => Report::build(
                        ReportKind::Error,
                        FileSpan::new(path.clone(), range.clone()),
                    )
                    .with_code(4)
                    .with_message("Other error")
                    .with_label(
                        Label::new(FileSpan::new(path.clone(), range.clone()))
                            .with_message("other error")
                            .with_color(colors.next()),
                    )
                    .finish(),
                },
            },
        };

        report
            .eprint(ariadne::FnCache::new(|x: &String| {
                std::fs::read_to_string(Path::new(x.as_str()))
            }))
            .expect("failed to print to stderr");
    }
}
