use super::LogicalOperator;
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::iter::Peekable;
use thiserror::Error;

/// A representation of an hierarchy of version constraints e.g. `1.3.4,>=5.0.1|(1.2.4,>=3.0.1)`.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum VersionTree<'a> {
    Term(&'a str),
    Group(LogicalOperator, Vec<VersionTree<'a>>),
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseVersionTreeError {
    #[error("missing '()")]
    MissingClosingParenthesis,

    #[error("unexpected token")]
    UnexpectedOperator,

    #[error("unexpected eof")]
    UnexpectedEndOfString,
}

impl<'a> TryFrom<&'a str> for VersionTree<'a> {
    type Error = ParseVersionTreeError;

    fn try_from(input: &'a str) -> Result<Self, Self::Error> {
        let version_spec_tokens = crate::utils::regex!("\\s*[()|,]|[^()|,]+");
        let tokens = version_spec_tokens
            .find_iter(input)
            .map(|m| match m.as_str() {
                "," => VersionTreeToken::And,
                "|" => VersionTreeToken::Or,
                "(" => VersionTreeToken::ParenOpen,
                ")" => VersionTreeToken::ParenClose,
                token => VersionTreeToken::Term(token.trim()),
            });

        #[derive(Eq, PartialEq)]
        enum VersionTreeToken<'a> {
            Term(&'a str),
            And,
            Or,
            ParenOpen,
            ParenClose,
        }

        impl<'a> Display for VersionTreeToken<'a> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                match self {
                    VersionTreeToken::Term(str) => write!(f, "{}", str),
                    VersionTreeToken::And => write!(f, ","),
                    VersionTreeToken::Or => write!(f, "|"),
                    VersionTreeToken::ParenOpen => write!(f, "("),
                    VersionTreeToken::ParenClose => write!(f, ")"),
                }
            }
        }

        /// Modifies the specified version tree to become a Group of the specified operator type and
        /// returns a reference to the container of the group.
        fn make_group<'a, 'b>(
            term: &'b mut VersionTree<'a>,
            op: LogicalOperator,
        ) -> &'b mut Vec<VersionTree<'a>> {
            if match term {
                VersionTree::Term(_) => true,
                VersionTree::Group(group_op, _) if *group_op != op => true,
                _ => false,
            } {
                let previous_term = std::mem::replace(term, VersionTree::Group(op, Vec::new()));
                let vec = match term {
                    VersionTree::Group(_, vec) => vec,
                    _ => unreachable!(),
                };
                vec.push(previous_term);
                vec
            } else {
                match term {
                    VersionTree::Group(_, vec) => vec,
                    _ => unreachable!(),
                }
            }
        }

        /// Parses an atomic term, e.g. (`>=1.2.3` or a group surrounded by parenthesis)
        fn parse_term<'a, I: Iterator<Item = VersionTreeToken<'a>>>(
            tokens: &mut Peekable<I>,
        ) -> Result<VersionTree<'a>, ParseVersionTreeError> {
            let token = tokens
                .next()
                .ok_or(ParseVersionTreeError::UnexpectedEndOfString)?;
            match token {
                VersionTreeToken::ParenOpen => {
                    let group = parse_group(tokens, 2)?;
                    if tokens.next() != Some(VersionTreeToken::ParenClose) {
                        return Err(ParseVersionTreeError::MissingClosingParenthesis);
                    }
                    Ok(group)
                }
                VersionTreeToken::Term(term) => Ok(VersionTree::Term(term)),
                _ => Err(ParseVersionTreeError::UnexpectedOperator),
            }
        }

        /// Returns the operator precedence to ensure correct ordering.
        fn op_precedence(op: LogicalOperator) -> u8 {
            match op {
                LogicalOperator::And => 1,
                LogicalOperator::Or => 2,
            }
        }

        /// Parses a group of constraints seperated by `|` and/or `,`.
        fn parse_group<'a, I: Iterator<Item = VersionTreeToken<'a>>>(
            tokens: &mut Peekable<I>,
            max_precedence: u8,
        ) -> Result<VersionTree<'a>, ParseVersionTreeError> {
            let mut result = parse_term(tokens)?;
            loop {
                let op = match tokens.peek() {
                    Some(VersionTreeToken::Or) => LogicalOperator::Or,
                    Some(VersionTreeToken::And) => LogicalOperator::And,
                    _ => break,
                };
                let precedence = op_precedence(op);
                if precedence > max_precedence {
                    break;
                }
                let _ = tokens.next();
                let next_term = parse_group(tokens, precedence - 1)?;
                let terms = make_group(&mut result, op);

                match next_term {
                    VersionTree::Group(other_op, mut others) if other_op == op => {
                        terms.append(&mut others)
                    }
                    term => terms.push(term),
                }
            }
            Ok(result)
        }

        parse_group(&mut tokens.peekable(), 2)
    }
}

#[cfg(test)]
mod tests {
    use super::{LogicalOperator, VersionTree};
    use std::convert::TryFrom;

    #[test]
    fn test_treeify() {
        use LogicalOperator::*;
        use VersionTree::*;

        assert_eq!(VersionTree::try_from("1.2.3").unwrap(), Term("1.2.3"));

        assert_eq!(
            VersionTree::try_from("1.2.3,(4.5.6),<=7.8.9").unwrap(),
            Group(And, vec![Term("1.2.3"), Term("4.5.6"), Term("<=7.8.9")])
        );
        assert_eq!(
            VersionTree::try_from("((1.2.3)|(4.5.6))|<=7.8.9").unwrap(),
            Group(Or, vec![Term("1.2.3"), Term("4.5.6"), Term("<=7.8.9")])
        );

        assert_eq!(
            VersionTree::try_from("1.2.3,4.5.6|<=7.8.9").unwrap(),
            Group(
                Or,
                vec![
                    Group(And, vec![Term("1.2.3"), Term("4.5.6")]),
                    Term("<=7.8.9")
                ]
            )
        );
        assert_eq!(
            VersionTree::try_from("((1.5|((1.6|1.7), 1.8), 1.9 |2.0))|2.1").unwrap(),
            Group(
                Or,
                vec![
                    Term("1.5"),
                    Group(
                        And,
                        vec![
                            Group(Or, vec![Term("1.6"), Term("1.7")]),
                            Term("1.8"),
                            Term("1.9")
                        ]
                    ),
                    Term("2.0"),
                    Term("2.1")
                ]
            )
        );
    }
}
