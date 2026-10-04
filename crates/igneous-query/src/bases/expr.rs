//! The Bases expression language used by filters, formulas and summaries.
//!
//! It's a small subset of JavaScript: literals (`"text"`, `'text'`, `12.5`,
//! `true`, `null`, `[1, 2]`, `{"a": 1}`, `/regex/g`), property access
//! (`price`, `note.price`, `note["unit price"]`, `file.name`, `formula.ppu`,
//! `this.file.folder`), indexing (`list[0]`), calls (`today()`,
//! `file.hasTag("book")`, `name.lower()`), and the operators below, tightest
//! last:
//!
//! | Operators | |
//! |---|---|
//! | `\|\|` | or |
//! | `&&` | and |
//! | `==` `!=` | equality |
//! | `<` `<=` `>` `>=` | comparison |
//! | `+` `-` | addition, string joining, date ± duration |
//! | `*` `/` `%` | multiplication |
//! | `!` `-` | not, negation |

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Regex {
        pattern: String,
        flags: String,
    },
    List(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    /// A bare name: a note property, or `file`, `note`, `formula`, `this`,
    /// or a variable such as `value` inside `filter()`.
    Ident(String),
    /// `receiver.name`
    Member(Box<Expr>, String),
    /// `receiver[index]`
    Index(Box<Expr>, Box<Expr>),
    /// A global function: `name(args)`.
    Call(String, Vec<Expr>),
    /// A method: `receiver.name(args)`.
    Method(Box<Expr>, String, Vec<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl BinaryOp {
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne => 3,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => 4,
            Self::Add | Self::Sub => 5,
            Self::Mul | Self::Div | Self::Rem => 6,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Or => "||",
            Self::And => "&&",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExprError {
    pub message: String,
    /// Byte offset in the expression.
    pub offset: usize,
}

pub fn parse(source: &str) -> Result<Expr, ExprError> {
    let mut parser = Parser {
        src: source,
        pos: 0,
    };
    let expr = parser.expr(0)?;
    parser.skip_ws();
    if parser.pos < source.len() {
        return Err(parser.error(format!("unexpected `{}`", parser.rest_preview())));
    }
    Ok(expr)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

const BINARY: [(&str, BinaryOp); 13] = [
    ("||", BinaryOp::Or),
    ("&&", BinaryOp::And),
    ("==", BinaryOp::Eq),
    ("!=", BinaryOp::Ne),
    ("<=", BinaryOp::Le),
    (">=", BinaryOp::Ge),
    ("<", BinaryOp::Lt),
    (">", BinaryOp::Gt),
    ("+", BinaryOp::Add),
    ("-", BinaryOp::Sub),
    ("*", BinaryOp::Mul),
    ("/", BinaryOp::Div),
    ("%", BinaryOp::Rem),
];

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn rest_preview(&self) -> String {
        self.rest().chars().take(12).collect()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn error(&self, message: impl Into<String>) -> ExprError {
        ExprError {
            message: message.into(),
            offset: self.pos,
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_ws();
        if self.rest().starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), ExprError> {
        if self.eat(token) {
            Ok(())
        } else if self.pos >= self.src.len() {
            Err(self.error(format!("expected `{token}` at the end")))
        } else {
            Err(self.error(format!(
                "expected `{token}` before `{}`",
                self.rest_preview()
            )))
        }
    }

    fn peek_binary(&mut self) -> Option<BinaryOp> {
        self.skip_ws();
        let rest = self.rest();
        BINARY
            .iter()
            .find(|(symbol, _)| rest.starts_with(symbol))
            .map(|(_, op)| *op)
    }

    fn expr(&mut self, min: u8) -> Result<Expr, ExprError> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.peek_binary() {
            if op.precedence() < min {
                break;
            }
            self.pos += op.symbol().len();
            let rhs = self.expr(op.precedence() + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        self.skip_ws();
        if self.rest().starts_with('!') && !self.rest().starts_with("!=") {
            self.pos += 1;
            return Ok(Expr::Unary(UnaryOp::Not, Box::new(self.unary()?)));
        }
        if self.rest().starts_with('-') {
            self.pos += 1;
            return Ok(Expr::Unary(UnaryOp::Neg, Box::new(self.unary()?)));
        }
        let primary = self.primary()?;
        self.postfix(primary)
    }

    fn postfix(&mut self, mut expr: Expr) -> Result<Expr, ExprError> {
        loop {
            self.skip_ws();
            if self.rest().starts_with('.') {
                self.pos += 1;
                self.skip_ws();
                let name = self
                    .ident()
                    .ok_or_else(|| self.error("expected a name after `.`"))?;
                if self.eat("(") {
                    let args = self.args(")")?;
                    expr = Expr::Method(Box::new(expr), name, args);
                } else {
                    expr = Expr::Member(Box::new(expr), name);
                }
            } else if self.rest().starts_with('[') {
                self.pos += 1;
                let index = self.expr(0)?;
                self.expect("]")?;
                expr = Expr::Index(Box::new(expr), Box::new(index));
            } else {
                return Ok(expr);
            }
        }
    }

    /// Comma-separated expressions up to `close`, which is consumed.
    fn args(&mut self, close: &str) -> Result<Vec<Expr>, ExprError> {
        let mut args = Vec::new();
        if self.eat(close) {
            return Ok(args);
        }
        loop {
            args.push(self.expr(0)?);
            if self.eat(close) {
                return Ok(args);
            }
            self.expect(",")?;
            // A trailing comma.
            if self.eat(close) {
                return Ok(args);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, ExprError> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return Err(self.error("the expression ends too early"));
        };
        match c {
            '(' => {
                self.pos += 1;
                let inner = self.expr(0)?;
                self.expect(")")?;
                Ok(inner)
            }
            '[' => {
                self.pos += 1;
                Ok(Expr::List(self.args("]")?))
            }
            '{' => {
                self.pos += 1;
                self.object()
            }
            '"' | '\'' => Ok(Expr::String(self.string(c)?)),
            '/' => self.regex(),
            c if c.is_ascii_digit()
                || (c == '.' && self.rest()[1..].starts_with(|d: char| d.is_ascii_digit())) =>
            {
                self.number()
            }
            _ => {
                let start = self.pos;
                let Some(name) = self.ident() else {
                    return Err(self.error(format!("unexpected `{}`", self.rest_preview())));
                };
                match name.as_str() {
                    "true" => return Ok(Expr::Bool(true)),
                    "false" => return Ok(Expr::Bool(false)),
                    "null" => return Ok(Expr::Null),
                    _ => {}
                }
                if self.eat("(") {
                    let args = self.args(")")?;
                    return Ok(Expr::Call(name, args));
                }
                debug_assert!(self.pos > start);
                Ok(Expr::Ident(name))
            }
        }
    }

    fn ident(&mut self) -> Option<String> {
        let rest = self.rest();
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            let ok = if i == 0 {
                c.is_alphabetic() || c == '_' || c == '$'
            } else {
                c.is_alphanumeric() || c == '_' || c == '$'
            };
            if !ok {
                break;
            }
            end = i + c.len_utf8();
        }
        if end == 0 {
            return None;
        }
        let name = rest[..end].to_owned();
        self.pos += end;
        Some(name)
    }

    fn number(&mut self) -> Result<Expr, ExprError> {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let mut end = 0;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        // A fraction only if a digit follows the dot: `5.isEmpty()` calls a
        // method on 5.
        if end < bytes.len()
            && bytes[end] == b'.'
            && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
        {
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        if end < bytes.len() && matches!(bytes[end], b'e' | b'E') {
            let mut exp = end + 1;
            if exp < bytes.len() && matches!(bytes[exp], b'+' | b'-') {
                exp += 1;
            }
            if exp < bytes.len() && bytes[exp].is_ascii_digit() {
                end = exp;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
            }
        }
        let value = rest[..end]
            .parse()
            .map_err(|_| self.error(format!("`{}` isn't a number", &rest[..end])))?;
        self.pos += end;
        Ok(Expr::Number(value))
    }

    fn string(&mut self, quote: char) -> Result<String, ExprError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    let Some((_, escaped)) = chars.next() else {
                        break;
                    };
                    match escaped {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '0' => out.push('\0'),
                        'u' => {
                            let hex: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                            let code = u32::from_str_radix(&hex, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| self.error(format!("bad escape `\\u{hex}`")))?;
                            out.push(code);
                        }
                        other => out.push(other),
                    }
                }
                c if c == quote => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                c => out.push(c),
            }
        }
        self.pos = start;
        Err(self.error("this text has no closing quote"))
    }

    fn regex(&mut self) -> Result<Expr, ExprError> {
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
                    let pattern = body[..i].to_owned();
                    let flags_len = body[i + 1..]
                        .find(|c: char| !c.is_ascii_alphabetic())
                        .unwrap_or(body.len() - i - 1);
                    let flags = body[i + 1..i + 1 + flags_len].to_owned();
                    self.pos += 1 + i + 1 + flags_len;
                    return Ok(Expr::Regex { pattern, flags });
                }
                _ => {}
            }
        }
        Err(self.error("this regular expression has no closing `/`"))
    }

    fn object(&mut self) -> Result<Expr, ExprError> {
        let mut entries = Vec::new();
        if self.eat("}") {
            return Ok(Expr::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = match self.peek() {
                Some(q @ ('"' | '\'')) => self.string(q)?,
                _ => self
                    .ident()
                    .ok_or_else(|| self.error("expected a key in the object"))?,
            };
            self.expect(":")?;
            entries.push((key, self.expr(0)?));
            if self.eat("}") {
                return Ok(Expr::Object(entries));
            }
            self.expect(",")?;
            if self.eat("}") {
                return Ok(Expr::Object(entries));
            }
        }
    }
}

/// Writes the expression back as source, for error messages and tests.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list(f: &mut fmt::Formatter<'_>, items: &[Expr]) -> fmt::Result {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{item}")?;
            }
            Ok(())
        }
        match self {
            Expr::Null => f.write_str("null"),
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::Number(n) => f.write_str(&super::value::format_number(*n)),
            Expr::String(s) => write!(f, "{s:?}"),
            Expr::Regex { pattern, flags } => write!(f, "/{pattern}/{flags}"),
            Expr::List(items) => {
                f.write_str("[")?;
                list(f, items)?;
                f.write_str("]")
            }
            Expr::Object(entries) => {
                f.write_str("{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k:?}: {v}")?;
                }
                f.write_str("}")
            }
            Expr::Ident(name) => f.write_str(name),
            Expr::Member(receiver, name) => write!(f, "{receiver}.{name}"),
            Expr::Index(receiver, index) => write!(f, "{receiver}[{index}]"),
            Expr::Call(name, args) => {
                write!(f, "{name}(")?;
                list(f, args)?;
                f.write_str(")")
            }
            Expr::Method(receiver, name, args) => {
                write!(f, "{receiver}.{name}(")?;
                list(f, args)?;
                f.write_str(")")
            }
            Expr::Unary(UnaryOp::Not, inner) => write!(f, "!{inner}"),
            Expr::Unary(UnaryOp::Neg, inner) => write!(f, "-{inner}"),
            Expr::Binary(op, lhs, rhs) => write!(f, "({lhs} {} {rhs})", op.symbol()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(source: &str) -> String {
        parse(source)
            .map(|e| e.to_string())
            .unwrap_or_else(|e| format!("error: {e}"))
    }

    #[test]
    fn precedence() {
        assert_eq!(round_trip("1 + 2 * 3"), "(1 + (2 * 3))");
        assert_eq!(round_trip("(1 + 2) * 3"), "((1 + 2) * 3)");
        assert_eq!(round_trip("a || b && c"), "(a || (b && c))");
        assert_eq!(round_trip("a == b < c"), "(a == (b < c))");
        assert_eq!(round_trip("1 - 2 - 3"), "((1 - 2) - 3)");
        assert_eq!(round_trip("!a && -b"), "(!a && -b)");
        assert_eq!(round_trip("a != b"), "(a != b)");
    }

    #[test]
    fn documented_examples_parse() {
        for source in [
            r#"if(price, price.toFixed(2) + " dollars")"#,
            "(price / age).toFixed(2)",
            "values.mean().round(3)",
            r#"status != "done""#,
            "formula.ppu > 5",
            "price > 2.1",
            r#"file.hasTag("tag")"#,
            r#"file.hasLink("Textbook")"#,
            r#"file.inFolder("Required Reading")"#,
            r#"file.ext == "md""#,
            r#"now() + "1 day""#,
            r#"file.mtime > now() - "1 week""#,
            r#"date("2024-12-01") + "1M" + "4h" + "3m""#,
            "now() - file.ctime",
            r#"datetime.format("YYYY-MM-DD")"#,
            "radius * (2 * 3.14)",
            r#"now() + (duration('1d') * 2)"#,
            "property[0]",
            r#"property["subprop"]"#,
            "property.subprop",
            r#"link("filename", icon("plus"))"#,
            "link(file.ctime.date().toString())",
            "author == this",
            "authors.contains(this)",
            "file.hasLink(this.file)",
            "this.file.folder",
            r#"[1,2,3,4].filter(value > 2)"#,
            r#"[1,2,3].reduce(acc + value, 0)"#,
            r#"values.filter(value.isType("number")).reduce(if(acc == null || value > acc, value, acc), null)"#,
            r#""a:b:c:d".replace(/:/g, "-")"#,
            r#""John Smith".replace(/(\w+) (\w+)/, "$2, $1")"#,
            r#""a,b,c,d".split(/,/, 3)"#,
            "(-5).abs()",
            "5.isEmpty()",
            "123.toString()",
            "(2.5).round()",
            "{}.isEmpty()",
            r#"{"a": 1, b: [true, null]}.keys()"#,
            r#"/abc/.matches("abcde")"#,
            "start_date + \"2w\"",
            r#"if(due_date < now() && status != "Done", "Overdue", "")"#,
            "monthlyUses * formula.Owned.round()",
            r#"note["price"]"#,
        ] {
            assert!(parse(source).is_ok(), "{source}: {:?}", parse(source));
        }
    }

    #[test]
    fn literals() {
        assert_eq!(parse("'it\\'s'"), Ok(Expr::String("it's".into())));
        assert_eq!(parse("\"\\u00e9\""), Ok(Expr::String("é".into())));
        assert_eq!(parse("1e3"), Ok(Expr::Number(1000.0)));
        assert_eq!(parse(".5"), Ok(Expr::Number(0.5)));
        assert_eq!(
            parse("/a\\/b[/]/gi"),
            Ok(Expr::Regex {
                pattern: "a\\/b[/]".into(),
                flags: "gi".into()
            })
        );
        assert_eq!(round_trip("5.isEmpty()"), "5.isEmpty()");
        assert_eq!(round_trip("[1, 2,]"), "[1, 2]");
    }

    #[test]
    fn errors() {
        for bad in [
            "", "1 +", "(1", "\"open", "a.", "f(1 2)", "1 2", "/open", "@",
        ] {
            assert!(parse(bad).is_err(), "{bad} parsed");
        }
    }
}
