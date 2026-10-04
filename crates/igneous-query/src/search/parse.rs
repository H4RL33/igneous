//! Parsing searches.
//!
//! The grammar, loosest binding first:
//!
//! ```text
//! or      = and ("OR" and)*
//! and     = unary*
//! unary   = "-" unary | primary
//! primary = "(" or ")" | phrase | regex | property | operator ":" operand | word
//! ```
//!
//! Parsing is forgiving, like Obsidian's: unclosed groups, phrases and
//! properties end at the end of the search, and anything that isn't syntax is
//! a word.

use super::{Comparison, Operator, Query};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ParseError {
    pub message: String,
    /// Byte offset in the search.
    pub offset: usize,
}

pub fn parse(input: &str) -> Result<Query, ParseError> {
    let mut parser = Parser {
        src: input,
        pos: 0,
        parens: 0,
        brackets: 0,
        in_value: false,
    };
    // At the top level nothing closes, so this reads the whole search: a `)`
    // or `]` with nothing to close is part of a word.
    parser.or()
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    /// Open `(` groups.
    parens: usize,
    /// Open `[` properties.
    brackets: usize,
    /// Inside a property value, where `null` and `<5` mean something.
    in_value: bool,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// Whether `c` closes an open group or property.
    fn is_closer(&self, c: char) -> bool {
        (c == ')' && self.parens > 0) || (c == ']' && self.brackets > 0)
    }

    fn at_or(&self) -> bool {
        let rest = self.rest();
        rest.starts_with("OR")
            && rest[2..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '(')
    }

    fn or(&mut self) -> Result<Query, ParseError> {
        let mut alternatives = vec![self.and()?];
        loop {
            self.skip_ws();
            if !self.at_or() {
                break;
            }
            self.pos += 2;
            alternatives.push(self.and()?);
        }
        // `a OR` and `OR a` mean `a`.
        if alternatives.len() > 1 {
            alternatives.retain(|q| *q != Query::Empty);
        }
        Ok(match alternatives.len() {
            0 => Query::Empty,
            1 => alternatives.pop().unwrap(),
            _ => Query::Or(alternatives),
        })
    }

    fn and(&mut self) -> Result<Query, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(c) if self.is_closer(c) => break,
                Some(_) if self.at_or() => break,
                Some(_) => items.push(self.unary()?),
            }
        }
        Ok(match items.len() {
            0 => Query::Empty,
            1 => items.pop().unwrap(),
            _ => Query::And(items),
        })
    }

    fn unary(&mut self) -> Result<Query, ParseError> {
        if self.peek() == Some('-')
            && self
                .peek_second()
                .is_some_and(|c| !c.is_whitespace() && !self.is_closer(c))
        {
            self.pos += 1;
            return Ok(Query::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Query, ParseError> {
        match self.peek() {
            Some('(') => self.group(),
            Some('"') => Ok(Query::Phrase(self.phrase())),
            Some('/') => match self.regex() {
                Some(pattern) => Ok(Query::Regex(pattern)),
                None => Ok(self.word()),
            },
            Some('[') => self.property(),
            _ => self.word_or_operator(),
        }
    }

    fn group(&mut self) -> Result<Query, ParseError> {
        self.pos += 1;
        self.parens += 1;
        let query = self.or();
        self.parens -= 1;
        if self.peek() == Some(')') {
            self.pos += 1;
        }
        query
    }

    fn phrase(&mut self) -> String {
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        let mut end = self.rest().len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some((_, next @ ('"' | '\\'))) => out.push(next),
                    Some((_, next)) => {
                        out.push('\\');
                        out.push(next);
                    }
                    None => out.push('\\'),
                },
                '"' => {
                    end = i + 1;
                    break;
                }
                c => out.push(c),
            }
        }
        self.pos += end;
        out
    }

    /// A `/pattern/`, if the slash starts one.
    fn regex(&mut self) -> Option<String> {
        let body = &self.rest()[1..];
        let mut in_class = false;
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => {
                    if i == 0 {
                        return None;
                    }
                    let pattern = body[..i].to_owned();
                    self.pos += i + 2;
                    return Some(pattern);
                }
                _ => {}
            }
        }
        None
    }

    fn property(&mut self) -> Result<Query, ParseError> {
        let start = self.pos;
        let body = &self.rest()[1..];
        let mut quote = false;
        let mut key_end = None;
        for (i, c) in body.char_indices() {
            match c {
                '"' => quote = !quote,
                ':' | ']' if !quote => {
                    key_end = Some((i, c));
                    break;
                }
                _ => {}
            }
        }
        let Some((key_len, delimiter)) = key_end else {
            return Ok(self.word());
        };
        let raw_key = body[..key_len].trim();
        let key = raw_key
            .strip_prefix('"')
            .and_then(|k| k.strip_suffix('"'))
            .unwrap_or(raw_key)
            .to_owned();
        if key.is_empty() {
            return Err(ParseError {
                message: "a property search needs a property name, as in [status]".into(),
                offset: start,
            });
        }
        self.pos += 1 + key_len + 1;
        if delimiter == ']' {
            return Ok(Query::Property { key, value: None });
        }

        let was_in_value = std::mem::replace(&mut self.in_value, true);
        self.brackets += 1;
        let value = self.or();
        self.brackets -= 1;
        self.in_value = was_in_value;
        let value = value?;
        if self.peek() == Some(']') {
            self.pos += 1;
        }
        Ok(Query::Property {
            key,
            value: (value != Query::Empty).then(|| Box::new(value)),
        })
    }

    fn word_or_operator(&mut self) -> Result<Query, ParseError> {
        let rest = self.rest();
        let operator = rest.find(':').and_then(|colon| {
            let name = &rest[..colon];
            let is_name =
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == '-');
            is_name
                .then(|| Operator::from_name(name))
                .flatten()
                .map(|op| (op, colon + 1))
        });
        let Some((operator, len)) = operator else {
            return Ok(self.word());
        };
        self.pos += len;
        let operand = match self.peek() {
            None => Query::Empty,
            Some(c) if c.is_whitespace() || self.is_closer(c) => Query::Empty,
            Some('(' | '"' | '/' | '[') => self.primary()?,
            Some(_) => self.word(),
        };
        Ok(Query::Op(operator, Box::new(operand)))
    }

    fn word(&mut self) -> Query {
        let start = self.pos;
        let mut end = self.src.len();
        for (i, c) in self.rest().char_indices() {
            // The first character is always part of the word, so a stray
            // `[` or `/` can't stop parsing.
            if i > 0 && (c.is_whitespace() || self.is_closer(c)) {
                end = start + i;
                break;
            }
        }
        self.pos = end;
        let word = &self.src[start..end];
        if self.in_value {
            if word.eq_ignore_ascii_case("null") {
                return Query::Null;
            }
            for (prefix, comparison) in [
                ("<=", Comparison::Le),
                (">=", Comparison::Ge),
                ("<", Comparison::Lt),
                (">", Comparison::Gt),
                ("=", Comparison::Eq),
            ] {
                if let Some(value) = word.strip_prefix(prefix) {
                    return Query::Compare(comparison, value.to_owned());
                }
            }
        }
        Query::Word(word.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Query {
        Query::Word(s.into())
    }

    fn op(o: Operator, q: Query) -> Query {
        Query::Op(o, Box::new(q))
    }

    fn not(q: Query) -> Query {
        Query::Not(Box::new(q))
    }

    #[test]
    fn words_and_or() {
        assert_eq!(
            parse("meeting work"),
            Ok(Query::And(vec![w("meeting"), w("work")]))
        );
        assert_eq!(
            parse("meeting OR work"),
            Ok(Query::Or(vec![w("meeting"), w("work")]))
        );
        assert_eq!(
            parse("meeting work OR meetup personal"),
            Ok(Query::Or(vec![
                Query::And(vec![w("meeting"), w("work")]),
                Query::And(vec![w("meetup"), w("personal")]),
            ]))
        );
        assert_eq!(
            parse("meeting (work OR meetup) personal"),
            Ok(Query::And(vec![
                w("meeting"),
                Query::Or(vec![w("work"), w("meetup")]),
                w("personal"),
            ]))
        );
        // Only upper-case OR is an operator.
        assert_eq!(
            parse("cats or dogs"),
            Ok(Query::And(vec![w("cats"), w("or"), w("dogs")]))
        );
        assert_eq!(parse("ORANGE"), Ok(w("ORANGE")));
        assert_eq!(parse(""), Ok(Query::Empty));
        assert_eq!(parse("   "), Ok(Query::Empty));
    }

    #[test]
    fn negation() {
        assert_eq!(
            parse("meeting -work"),
            Ok(Query::And(vec![w("meeting"), not(w("work"))]))
        );
        assert_eq!(
            parse("meeting -work -meetup"),
            Ok(Query::And(vec![
                w("meeting"),
                not(w("work")),
                not(w("meetup"))
            ]))
        );
        assert_eq!(
            parse("meeting -(work meetup)"),
            Ok(Query::And(vec![
                w("meeting"),
                not(Query::And(vec![w("work"), w("meetup")]))
            ]))
        );
        assert_eq!(parse("- dash"), Ok(Query::And(vec![w("-"), w("dash")])));
        assert_eq!(parse("-line:x"), Ok(not(op(Operator::Line, w("x")))));
    }

    #[test]
    fn phrases_and_regexes() {
        assert_eq!(
            parse("\"star wars\""),
            Ok(Query::Phrase("star wars".into()))
        );
        assert_eq!(
            parse(r#""they said \"hello\" to each other""#),
            Ok(Query::Phrase(r#"they said "hello" to each other"#.into()))
        );
        assert_eq!(
            parse(r"/\d{4}-\d{2}-\d{2}/"),
            Ok(Query::Regex(r"\d{4}-\d{2}-\d{2}".into()))
        );
        assert_eq!(parse(r"/a[/]b/"), Ok(Query::Regex("a[/]b".into())));
        assert_eq!(parse("/unclosed"), Ok(w("/unclosed")));
        assert_eq!(
            parse("\"unclosed phrase"),
            Ok(Query::Phrase("unclosed phrase".into()))
        );
    }

    #[test]
    fn operators() {
        assert_eq!(parse("file:.jpg"), Ok(op(Operator::File, w(".jpg"))));
        assert_eq!(parse("file:202209"), Ok(op(Operator::File, w("202209"))));
        assert_eq!(
            parse("path:\"Daily notes/2022-07\""),
            Ok(op(
                Operator::Path,
                Query::Phrase("Daily notes/2022-07".into())
            ))
        );
        assert_eq!(
            parse("content:\"happy cat\""),
            Ok(op(Operator::Content, Query::Phrase("happy cat".into())))
        );
        assert_eq!(
            parse("match-case:HappyCat"),
            Ok(op(Operator::MatchCase, w("HappyCat")))
        );
        assert_eq!(
            parse("ignore-case:ikea"),
            Ok(op(Operator::IgnoreCase, w("ikea")))
        );
        assert_eq!(parse("tag:#work"), Ok(op(Operator::Tag, w("#work"))));
        assert_eq!(
            parse("line:(mix flour)"),
            Ok(op(Operator::Line, Query::And(vec![w("mix"), w("flour")])))
        );
        assert_eq!(
            parse("block:(dog cat)"),
            Ok(op(Operator::Block, Query::And(vec![w("dog"), w("cat")])))
        );
        assert_eq!(
            parse("section:(dog cat)"),
            Ok(op(Operator::Section, Query::And(vec![w("dog"), w("cat")])))
        );
        assert_eq!(parse("task:call"), Ok(op(Operator::Task, w("call"))));
        assert_eq!(
            parse("task-todo:call"),
            Ok(op(Operator::TaskTodo, w("call")))
        );
        assert_eq!(
            parse("task-done:call"),
            Ok(op(Operator::TaskDone, w("call")))
        );
        assert_eq!(
            parse("task:(call OR email)"),
            Ok(op(Operator::Task, Query::Or(vec![w("call"), w("email")])))
        );
        assert_eq!(
            parse(r"path:/\d{4}-\d{2}-\d{2}/"),
            Ok(op(
                Operator::Path,
                Query::Regex(r"\d{4}-\d{2}-\d{2}".into())
            ))
        );
        assert_eq!(parse("FILE:x"), Ok(op(Operator::File, w("x"))));
        // Not an operator: an ordinary word with a colon.
        assert_eq!(parse("time:12:30"), Ok(w("time:12:30")));
        assert_eq!(parse("file:"), Ok(op(Operator::File, Query::Empty)));
        assert_eq!(
            parse("(file:a) b"),
            Ok(Query::And(vec![op(Operator::File, w("a")), w("b")]))
        );
    }

    #[test]
    fn properties() {
        let prop = |key: &str, value: Option<Query>| Query::Property {
            key: key.into(),
            value: value.map(Box::new),
        };
        assert_eq!(parse("[aliases]"), Ok(prop("aliases", None)));
        assert_eq!(
            parse("[aliases:Name]"),
            Ok(prop("aliases", Some(w("Name"))))
        );
        assert_eq!(
            parse("[aliases:null]"),
            Ok(prop("aliases", Some(Query::Null)))
        );
        assert_eq!(
            parse("[status:Draft OR Published]"),
            Ok(prop(
                "status",
                Some(Query::Or(vec![w("Draft"), w("Published")]))
            ))
        );
        assert_eq!(
            parse("meeting [duration:<5]"),
            Ok(Query::And(vec![
                w("meeting"),
                prop("duration", Some(Query::Compare(Comparison::Lt, "5".into())))
            ]))
        );
        assert_eq!(
            parse("[duration:>5]"),
            Ok(prop(
                "duration",
                Some(Query::Compare(Comparison::Gt, "5".into()))
            ))
        );
        assert_eq!(
            parse("[\"due date\":\"next week\"]"),
            Ok(prop("due date", Some(Query::Phrase("next week".into()))))
        );
        assert_eq!(
            parse("[status:(a OR b)] c"),
            Ok(Query::And(vec![
                prop("status", Some(Query::Or(vec![w("a"), w("b")]))),
                w("c"),
            ]))
        );
        // `null` and comparisons only mean something inside a value.
        assert_eq!(parse("null <5"), Ok(Query::And(vec![w("null"), w("<5")])));
        assert!(parse("[]").is_err());
        assert_eq!(parse("[unclosed"), Ok(w("[unclosed")));
    }

    #[test]
    fn stray_closers_are_text() {
        assert_eq!(parse("a) b"), Ok(Query::And(vec![w("a)"), w("b")])));
        assert_eq!(parse("(a b"), Ok(Query::And(vec![w("a"), w("b")])));
        assert_eq!(
            parse("embed OR search"),
            Ok(Query::Or(vec![w("embed"), w("search")]))
        );
    }
}
